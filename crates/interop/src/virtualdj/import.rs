//! Turns VirtualDJ's songs and playlists into an [`ImportPlan`] against the library.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use library::{HotcuePoint, Reader};

use super::{InteropError, VdjPlaylist, VdjPoi, VdjSong};
use crate::import::{ImportPlan, PlaylistAction, PlaylistImport, TrackImport};

/// Plans importing `songs`' hotcues and loops and `playlists` into the library `reader`
/// reads, writing nothing.
///
/// Each song is matched to a library track by exact path, else by the audio fingerprint
/// of the file at its path when that file exists, else by being the only track with the
/// same file name and size. Slots 1-8 keep their number; hotcues and loops numbered above
/// 8 fill the free slots in position order. Positions convert at the track's sample rate.
/// A track matched by two songs takes the one from a drive's own database, else the one
/// with hotcues or loops, else the first; the other's hotcues and loops count as dropped.
///
/// # Errors
///
/// [`InteropError::Library`] when the library cannot be read.
pub fn plan_virtualdj(
    reader: &Reader,
    songs: &[VdjSong],
    playlists: &[VdjPlaylist],
) -> Result<ImportPlan, InteropError> {
    let mut matcher = Matcher {
        reader,
        sizes: songs.iter().map(|s| (s.path.as_path(), s.size)).collect(),
        found: HashMap::new(),
    };
    let mut plan = ImportPlan {
        tracks: Vec::new(),
        playlists: Vec::new(),
        unmatched: Vec::new(),
        dropped: 0,
        other_markers: 0,
    };
    let mut unmatched = HashSet::new();
    let mut not_found = |path: &Path, plan: &mut ImportPlan| {
        if unmatched.insert(path.to_path_buf()) {
            plan.unmatched.push(path.to_path_buf());
        }
    };
    // Per planned track, the entry it comes from.
    let mut kept: Vec<Kept> = Vec::new();
    for song in songs {
        let Some((track, rate)) = matcher.find(&song.path)? else {
            plan.other_markers += song.other_markers;
            not_found(&song.path, &mut plan);
            continue;
        };
        let (incoming, dropped) = slots(song, rate);
        let entry = Kept {
            from_drive: song.from_drive,
            has_points: incoming.iter().any(Option::is_some),
            points: song.pois.len(),
            dropped,
            other_markers: song.other_markers,
        };
        // The same file listed twice (say, in two databases) is imported once, from the
        // better entry; the other's hotcues and loops are dropped.
        if let Some(i) = plan.tracks.iter().position(|t| t.track == track) {
            if entry.rank() > kept[i].rank() {
                plan.dropped += kept[i].points;
                kept[i] = entry;
                plan.tracks[i].path = song.path.clone();
                plan.tracks[i].incoming = incoming;
            } else {
                plan.dropped += entry.points;
            }
            continue;
        }
        kept.push(entry);
        plan.tracks.push(TrackImport {
            track,
            path: song.path.clone(),
            incoming,
            existing: reader.hotcue_points(track)?,
        });
    }
    for entry in &kept {
        plan.dropped += entry.dropped;
        plan.other_markers += entry.other_markers;
    }

    let mut taken = existing_playlists(reader)?;
    for list in playlists {
        let mut tracks = Vec::new();
        for path in &list.paths {
            match matcher.find(path)? {
                Some((track, _)) => tracks.push(track),
                None => not_found(path, &mut plan),
            }
        }
        let (name, action) = place_playlist(&list.name, &tracks, &taken);
        taken.push((name.clone(), tracks.clone()));
        plan.playlists.push(PlaylistImport {
            name,
            tracks,
            action,
        });
    }
    Ok(plan)
}

/// The VirtualDJ entry a planned track comes from.
struct Kept {
    from_drive: bool,
    /// It gives the track a hotcue or loop.
    has_points: bool,
    /// Its hotcues and loops, imported or not.
    points: usize,
    /// Its hotcues and loops that do not fit or are invalid.
    dropped: usize,
    other_markers: usize,
}

impl Kept {
    /// Which of two entries for one track wins: the drive's own database's, else one with
    /// hotcues or loops, else the first.
    fn rank(&self) -> (bool, bool) {
        (self.from_drive, self.has_points)
    }
}

/// Finds library tracks for VirtualDJ paths, remembering each answer.
struct Matcher<'a> {
    reader: &'a Reader,
    /// The file sizes the database gives, for the name-and-size match.
    sizes: HashMap<&'a Path, Option<u64>>,
    /// Track id and sample rate per path already looked up.
    found: HashMap<PathBuf, Option<(i64, u32)>>,
}

impl Matcher<'_> {
    /// The library track for `path` and its sample rate, or `None` when there is none or
    /// its sample rate is unknown.
    fn find(&mut self, path: &Path) -> Result<Option<(i64, u32)>, InteropError> {
        if let Some(found) = self.found.get(path) {
            return Ok(*found);
        }
        let found = match self.track_id(path)? {
            Some(id) => self
                .reader
                .track(id)?
                .and_then(|t| t.row.sample_rate)
                .map(|rate| (id, rate)),
            None => None,
        };
        self.found.insert(path.to_path_buf(), found);
        Ok(found)
    }

    fn track_id(&self, path: &Path) -> Result<Option<i64>, InteropError> {
        // A path that is not UTF-8 cannot be stored in the library, so it never matches.
        if path.to_str().is_some()
            && let Some(id) = self.reader.track_id_by_path(path)?
        {
            return Ok(Some(id));
        }
        if path.is_file()
            && let Ok(fingerprint) = library::fingerprint::of_file(path)
            && let [id] = self.reader.track_ids_by_fingerprint(&fingerprint)?[..]
        {
            return Ok(Some(id));
        }
        let size = self.sizes.get(path).copied().flatten().or_else(|| {
            path.metadata()
                .ok()
                .filter(std::fs::Metadata::is_file)
                .map(|m| m.len())
        });
        if let (Some(name), Some(size)) = (path.file_name().and_then(|n| n.to_str()), size)
            && let [id] = self.reader.track_ids_by_name_and_size(name, size)?[..]
        {
            return Ok(Some(id));
        }
        Ok(None)
    }
}

/// A song's hotcues and loops by slot at `rate`, and how many were dropped.
fn slots(song: &VdjSong, rate: u32) -> ([Option<HotcuePoint>; 8], usize) {
    let rate = f64::from(rate);
    let mut points: Vec<(u32, HotcuePoint)> = Vec::new();
    let mut dropped = 0;
    for poi in &song.pois {
        let (num, seconds, beats, name, color) = match poi {
            VdjPoi::Hotcue {
                num,
                seconds,
                name,
                color,
            } => (*num, *seconds, None, name, color),
            VdjPoi::Loop {
                num,
                seconds,
                beats,
                name,
                color,
            } => (*num, *seconds, Some(*beats), name, color),
        };
        let frames = seconds * rate;
        if !frames.is_finite() || frames < 0.0 {
            dropped += 1;
            continue;
        }
        // Without VirtualDJ's grid the loop's length is unknown: it comes in as a hotcue.
        let loop_frames = beats
            .zip(song.seconds_per_beat)
            .map(|(beats, spb)| beats * spb * rate)
            .filter(|len| len.is_finite() && *len > 0.0);
        points.push((
            num,
            HotcuePoint {
                frames,
                loop_frames,
                color: *color,
                name: name.clone(),
            },
        ));
    }
    points.sort_by(|a, b| a.1.frames.total_cmp(&b.1.frames));

    let mut out = [const { None }; 8];
    let mut overflow = Vec::new();
    for (num, point) in points {
        match usize::try_from(num).ok().filter(|n| (1..=8).contains(n)) {
            Some(n) if out[n - 1].is_none() => out[n - 1] = Some(point),
            Some(_) => dropped += 1,
            None => overflow.push(point),
        }
    }
    for point in overflow {
        match out.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(point),
            None => dropped += 1,
        }
    }
    (out, dropped)
}

/// The library's playlists, by name with their tracks.
fn existing_playlists(reader: &Reader) -> Result<Vec<(String, Vec<i64>)>, InteropError> {
    let mut out = Vec::new();
    for p in reader.playlists()? {
        let tracks = reader
            .playlist_tracks(p.id, 0, usize::MAX)?
            .iter()
            .map(|t| t.id)
            .collect();
        out.push((p.name, tracks));
    }
    Ok(out)
}

/// The name an imported playlist gets: its own, else the first free `Name (n)`, unless
/// a playlist of that name already holds exactly these tracks.
fn place_playlist(
    name: &str,
    tracks: &[i64],
    taken: &[(String, Vec<i64>)],
) -> (String, PlaylistAction) {
    for n in 1usize.. {
        let candidate = if n == 1 {
            name.to_owned()
        } else {
            format!("{name} ({n})")
        };
        let mut same_name = taken.iter().filter(|(t, _)| *t == candidate).peekable();
        if same_name.peek().is_none() {
            return (candidate, PlaylistAction::Create);
        }
        if same_name.any(|(_, t)| t == tracks) {
            return (candidate, PlaylistAction::SkipSame);
        }
    }
    unreachable!("some suffix is always free")
}

//! The stems cache: separated stems kept on disk within a budget, so a reload is instant.
//!
//! One folder per entry, `<stems dir>/<track id>-<fingerprint hex>/`, holding the three
//! stored stems as Ogg Vorbis (VBR around 192 kbit/s, at the track's native rate and frame
//! count) and a `stems.toml` naming the format, rate, frame count and model identity (see
//! `docs/stems-backend.md`, "Cache"). An entry is written to a temporary folder beside the
//! entries and renamed into place; its row in the `stems` table records its size.
//!
//! Anything wrong with an entry (a missing, truncated or unreadable file, another model,
//! rate or frame count, a changed fingerprint) makes it a miss, and the entry is removed so
//! the stems are separated again.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::num::{NonZeroU8, NonZeroU32};
use std::path::{Path, PathBuf};

use engine::{Stem, StemSet};
use rusqlite::OptionalExtension;
use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

use crate::writer::{StemsRow, WriteOp, unix_now};
use crate::{Library, LibraryError, Reader, Writer};

/// The entry format written to `stems.toml`; any other is a miss.
const FORMAT: i64 = 1;

/// Loaded within this many seconds (30 days) counts as recently played: kept longer.
pub const RECENT_SECONDS: i64 = 30 * 24 * 60 * 60;

/// The stored stems' files, in [`StemSet::new`] order.
const FILES: [(&str, Stem); 3] = [
    ("vocals.ogg", Stem::Vocals),
    ("bass.ogg", Stem::Bass),
    ("rhythm.ogg", Stem::Rhythm),
];

const MANIFEST: &str = "stems.toml";

/// Temporary entry folders start with this; any left by a crash are swept by
/// [`StemCache::new`] once older than [`TEMP_MAX_AGE`].
const TEMP_PREFIX: &str = ".tmp-";

/// A temporary folder older than this (1 hour) is a crash's leftover, not a write in
/// progress in another process.
const TEMP_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

const STEREO: NonZeroU8 = NonZeroU8::new(2).unwrap();
const TARGET_BITRATE: NonZeroU32 = NonZeroU32::new(192_000).unwrap();

/// The encoder quality where libvorbis has no bitrate table for the rate.
const FALLBACK_QUALITY: f32 = 0.6;

/// Frames handed to the encoder at a time.
const BLOCK_FRAMES: usize = 4096;

/// A fixed Ogg stream serial: each file holds one stream, so it need not be random.
const STREAM_SERIAL: i32 = 0x4f4a_0001;

/// The scale of a 16-bit sample, as [`StemSet`] stores them: `i16::MIN` is −1.0.
const I16_SCALE: f32 = 1.0 / 32_768.0;

/// Which stems an entry holds: a library track, its fingerprint, and the loaded track's
/// native rate and frame count, which a cached entry must match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StemKey<'a> {
    pub track: i64,
    /// The track's audio fingerprint (`tracks.fingerprint`).
    pub fingerprint: &'a [u8],
    pub sample_rate: u32,
    pub frames: usize,
}

/// What [`StemCache::evict`] (or [`StemCache::put`]) did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Eviction {
    /// The tracks whose entries were removed, in eviction order.
    pub removed: Vec<i64>,
    /// Entries that could not be removed (their files stay, and so does their row, so a
    /// later pass retries), or 1 if eviction itself failed after a [`StemCache::put`].
    pub failed: usize,
    /// The cache's size afterwards.
    pub bytes: u64,
    /// Still over budget: everything left is held by a loaded deck.
    pub over_budget: bool,
    /// A message to log: set the first time the cache stays over budget (until it is back
    /// under), and whenever an eviction after a [`StemCache::put`] failed.
    pub notice: Option<String>,
}

/// The stems cache. Reads and writes go through the library's reader and writer; the
/// files live under `dir`.
#[derive(Debug)]
pub struct StemCache {
    dir: PathBuf,
    budget: u64,
    writer: Writer,
    reader: Reader,
    /// The over-budget notice was given and the cache has not been under budget since.
    over_budget_noted: bool,
    /// Where the decoded sets split kicks from rhythm (kicks are not stored).
    kicks_hz: f32,
}

/// An entry's row.
struct Row {
    fingerprint: Vec<u8>,
    path: String,
}

impl StemCache {
    /// Opens the cache in `dir` (created if needed) with a budget of `budget_bytes`.
    ///
    /// Leftovers of a crash are swept, best effort: temporary folders over an hour old,
    /// entry folders without a row (a crash between the rename and the row) and rows whose
    /// folder is gone. Other files and folders in `dir` (such as the CoreML compile cache)
    /// are left alone, and so is anything that cannot be read or removed.
    ///
    /// # Errors
    ///
    /// Fails if `dir` cannot be created, or on a database error.
    pub fn new(
        library: &Library,
        dir: impl Into<PathBuf>,
        budget_bytes: u64,
    ) -> Result<Self, LibraryError> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let cache = Self {
            dir,
            budget: budget_bytes,
            writer: library.writer(),
            reader: library.reader()?,
            over_budget_noted: false,
            kicks_hz: engine::DEFAULT_KICKS_HZ,
        };
        cache.sweep()?;
        Ok(cache)
    }

    /// Splits the kicks of the sets [`StemCache::get`] decodes at `hz` instead of
    /// [`engine::DEFAULT_KICKS_HZ`].
    #[must_use]
    pub fn with_kicks_hz(mut self, hz: f32) -> Self {
        self.kicks_hz = hz;
        self
    }

    /// The folder holding the entries.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The stems for `key`, decoded, or `None` when they are not cached. An entry that
    /// cannot be used (see the module docs) is removed. Never panics on bad files.
    #[must_use]
    pub fn get(&self, key: &StemKey<'_>) -> Option<StemSet> {
        let row = self.row(key.track).ok()??;
        match self
            .check(key, &row)
            .and_then(|dir| decode_entry(&dir, key, self.kicks_hz))
        {
            Some(set) => Some(set),
            None => {
                let _ = self.remove(key.track, &row.path);
                None
            }
        }
    }

    /// Whether `key` has an entry that looks usable (its row, `stems.toml` and files),
    /// without decoding it; an unusable one is removed. [`StemCache::get`] still checks
    /// the audio.
    #[must_use]
    pub fn contains(&self, key: &StemKey<'_>) -> bool {
        let Ok(Some(row)) = self.row(key.track) else {
            return false;
        };
        let usable = self.check(key, &row).is_some_and(|dir| {
            FILES.iter().all(|(file, _)| {
                fs::metadata(dir.join(file)).is_ok_and(|m| m.is_file() && m.len() > 0)
            })
        });
        if !usable {
            let _ = self.remove(key.track, &row.path);
        }
        usable
    }

    /// Stores `set` as `key`'s entry (replacing any entry of the track), then evicts down
    /// to the budget, never touching `key.track` or the tracks in `loaded`. `prepared`
    /// marks stems separated ahead with `library stems`; it is kept once set.
    ///
    /// # Errors
    ///
    /// Fails if the stems cannot be encoded or written (nothing is stored then), or on a
    /// database error.
    pub fn put(
        &mut self,
        key: &StemKey<'_>,
        set: &StemSet,
        prepared: bool,
        loaded: &[i64],
    ) -> Result<Eviction, LibraryError> {
        if set.frames() != key.frames {
            return Err(LibraryError::StemEncode(format!(
                "the stems have {} frames, the track {}",
                set.frames(),
                key.frames
            )));
        }
        let name = entry_name(key.track, key.fingerprint);
        let temp = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .tempdir_in(&self.dir)?;
        write_entry(temp.path(), set, key.sample_rate)?;
        let bytes = folder_bytes(temp.path())?;

        if let Some(old) = self.row(key.track)?
            && old.path != name
            && is_entry_name(&old.path)
        {
            // Best effort: a folder left behind has no row and is swept later.
            let _ = remove_folder(&self.dir.join(&old.path));
        }
        let target = self.dir.join(&name);
        // Windows cannot rename a folder onto an existing one (Unix can only onto an empty
        // one), so any previous entry under this name goes first: a crash in between leaves
        // a row without its folder, which is a miss.
        remove_folder(&target)?;
        // Every file in `temp` was closed by `write_entry`; the guard holds no handle.
        let temp = temp.keep();
        if let Err(e) = rename_folder(&temp, &target) {
            let _ = fs::remove_dir_all(&temp);
            return Err(e.into());
        }
        self.writer
            .submit_wait(WriteOp::PutStems(Box::new(StemsRow {
                track_id: key.track,
                fingerprint: key.fingerprint.to_vec(),
                path: name,
                bytes,
                prepared,
            })))?;
        let mut protected = loaded.to_vec();
        protected.push(key.track);
        // The entry is stored: an eviction failure is reported, not an error.
        Ok(self.evict(&protected).unwrap_or_else(|e| {
            let bytes = self.bytes().unwrap_or(0);
            Eviction {
                removed: Vec::new(),
                failed: 1,
                bytes,
                over_budget: bytes > self.budget,
                notice: Some(format!("stems cache: eviction failed: {e}")),
            }
        }))
    }

    /// Marks track `track`'s entry as prepared ahead (no-op without one).
    ///
    /// # Errors
    ///
    /// On a database error.
    pub fn mark_prepared(&self, track: i64) -> Result<(), LibraryError> {
        self.writer.submit_wait(WriteOp::MarkStemsPrepared(track))?;
        Ok(())
    }

    /// Removes entries until the cache is within its budget, never one of the tracks in
    /// `loaded`. First tracks in no playlist, not prepared (no cue above 0, no hotcue, not
    /// prepared with `library stems`) and not loaded in the last 30 days, oldest load first
    /// (never loaded first); then the rest, oldest load first. If only loaded tracks are
    /// left over budget, [`Eviction::notice`] says so, once.
    ///
    /// # Errors
    ///
    /// Fails if an entry's folder cannot be removed, or on a database error.
    pub fn evict(&mut self, loaded: &[i64]) -> Result<Eviction, LibraryError> {
        let mut bytes = self.bytes()?;
        let mut removed = Vec::new();
        let mut failed = 0;
        if bytes > self.budget {
            for (track, path, size) in self.eviction_order()? {
                if bytes <= self.budget {
                    break;
                }
                if loaded.contains(&track) {
                    continue;
                }
                // Best effort per entry: one that cannot be removed now is retried later.
                match self.remove_files(&path) {
                    Ok(()) => {
                        self.writer.submit_wait(WriteOp::RemoveStems(track))?;
                        bytes = bytes.saturating_sub(size);
                        removed.push(track);
                    }
                    Err(_) => failed += 1,
                }
            }
        }
        let over_budget = bytes > self.budget;
        let notice = (over_budget && !self.over_budget_noted).then(|| {
            format!(
                "stems cache: {:.1} GB is over its {:.1} GB budget, but every entry left is \
                 in use (loaded in a deck or just stored) or could not be removed; it \
                 shrinks once they can go",
                bytes as f64 / 1e9,
                self.budget as f64 / 1e9
            )
        });
        self.over_budget_noted = over_budget;
        Ok(Eviction {
            removed,
            failed,
            bytes,
            over_budget,
            notice,
        })
    }

    /// The cache's size: the recorded size of every entry.
    ///
    /// # Errors
    ///
    /// On a database error.
    pub fn bytes(&self) -> Result<u64, LibraryError> {
        let total: i64 = self.reader.connection().query_row(
            "SELECT coalesce(sum(bytes), 0) FROM stems",
            [],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(total).unwrap_or(0))
    }

    /// Track `track`'s fingerprint, the key's other half.
    ///
    /// # Errors
    ///
    /// On a database error.
    pub fn fingerprint(&self, track: i64) -> Result<Option<Vec<u8>>, LibraryError> {
        Ok(self
            .reader
            .connection()
            .prepare_cached("SELECT fingerprint FROM tracks WHERE id = ?1")?
            .query_row([track], |r| r.get(0))
            .optional()?)
    }

    fn row(&self, track: i64) -> Result<Option<Row>, LibraryError> {
        Ok(self
            .reader
            .connection()
            .prepare_cached("SELECT fingerprint, path FROM stems WHERE track_id = ?1")?
            .query_row([track], |r| {
                Ok(Row {
                    fingerprint: r.get(0)?,
                    path: r.get(1)?,
                })
            })
            .optional()?)
    }

    /// The entry's folder if `row` is `key`'s and its `stems.toml` matches.
    fn check(&self, key: &StemKey<'_>, row: &Row) -> Option<PathBuf> {
        if row.fingerprint != key.fingerprint || row.path != entry_name(key.track, key.fingerprint)
        {
            return None;
        }
        let dir = self.dir.join(&row.path);
        let manifest = fs::read_to_string(dir.join(MANIFEST)).ok()?;
        manifest_matches(&manifest, key).then_some(dir)
    }

    /// Removes an entry's folder, then its row.
    fn remove(&self, track: i64, path: &str) -> Result<(), LibraryError> {
        self.remove_files(path)?;
        self.writer.submit_wait(WriteOp::RemoveStems(track))?;
        Ok(())
    }

    /// Removes an entry's folder (only one named like an entry).
    fn remove_files(&self, path: &str) -> std::io::Result<()> {
        if is_entry_name(path) {
            remove_folder(&self.dir.join(path))?;
        }
        Ok(())
    }

    /// Every entry `(track, path, bytes)`, in eviction order.
    fn eviction_order(&self) -> Result<Vec<(i64, String, u64)>, LibraryError> {
        let recent = unix_now().saturating_sub(RECENT_SECONDS);
        let mut stmt = self.reader.connection().prepare(
            "SELECT s.track_id, s.path, s.bytes FROM stems s \
             LEFT JOIN tracks t ON t.id = s.track_id \
             ORDER BY \
                 (s.prepared \
                  OR EXISTS (SELECT 1 FROM playlist_tracks p WHERE p.track_id = s.track_id) \
                  OR EXISTS (SELECT 1 FROM cues c \
                             WHERE c.track_id = s.track_id AND c.cue_frames > 0) \
                  OR EXISTS (SELECT 1 FROM hotcues h WHERE h.track_id = s.track_id) \
                  OR coalesce(t.last_loaded_at, 0) >= ?1), \
                 coalesce(t.last_loaded_at, 0), s.created_at, s.track_id",
        )?;
        let rows = stmt.query_map([recent], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                u64::try_from(r.get::<_, i64>(2)?).unwrap_or(0),
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Removes leftovers of a crash (see [`StemCache::new`]).
    fn sweep(&self) -> Result<(), LibraryError> {
        let rows: Vec<(i64, String)> = {
            let mut stmt = self
                .reader
                .connection()
                .prepare("SELECT track_id, path FROM stems")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        let known: HashSet<&str> = rows.iter().map(|(_, path)| path.as_str()).collect();
        // Best effort throughout: whatever cannot be read or removed now is left for a
        // later pass.
        let entries = fs::read_dir(&self.dir).into_iter().flatten().flatten();
        for entry in entries {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let stale = if name.starts_with(TEMP_PREFIX) {
                // Another process may still be writing it.
                entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > TEMP_MAX_AGE)
            } else {
                is_entry_name(name) && !known.contains(name)
            };
            if stale && entry.file_type().is_ok_and(|t| t.is_dir()) {
                let _ = remove_folder(&entry.path());
            }
        }
        for (track, path) in &rows {
            if !is_entry_name(path) || !self.dir.join(path).exists() {
                self.writer.submit_wait(WriteOp::RemoveStems(*track))?;
            }
        }
        Ok(())
    }
}

/// `<track id>-<fingerprint hex>`.
fn entry_name(track: i64, fingerprint: &[u8]) -> String {
    let mut name = format!("{track}-");
    for byte in fingerprint {
        name.push_str(&format!("{byte:02x}"));
    }
    name
}

/// Whether `name` has the form of [`entry_name`], so it is safe to delete under the stems
/// folder (no separators, no `..`).
fn is_entry_name(name: &str) -> bool {
    let Some((track, hex)) = name.split_once('-') else {
        return false;
    };
    !track.is_empty()
        && track.bytes().all(|b| b.is_ascii_digit())
        && !hex.is_empty()
        && hex.len() % 2 == 0
        && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Removes a folder and everything in it; one that is not there is fine. A stray file in
/// its place is removed too, so it cannot leave a row that never goes.
fn remove_folder(path: &Path) -> std::io::Result<()> {
    let removed = match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_dir() => fs::remove_file(path),
        _ => fs::remove_dir_all(path),
    };
    match removed {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Renames folder `from` to `to` (which must not exist).
///
/// Windows: right after files are written, a virus scanner or the search indexer may
/// briefly hold one of them open, and the folder's rename then fails with "Access is
/// denied" (os 5); it is retried a few times over about a second.
fn rename_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    const ATTEMPTS: u32 = 6;
    let mut attempt = 1;
    loop {
        match fs::rename(from, to) {
            Err(e)
                if cfg!(windows)
                    && e.kind() == std::io::ErrorKind::PermissionDenied
                    && attempt < ATTEMPTS =>
            {
                std::thread::sleep(std::time::Duration::from_millis(50 * u64::from(attempt)));
                attempt += 1;
            }
            result => return result,
        }
    }
}

/// The total size of the files in `dir`.
fn folder_bytes(dir: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(dir)? {
        total += entry?.metadata()?.len();
    }
    Ok(total)
}

fn manifest_text(sample_rate: u32, frames: usize) -> String {
    format!(
        "format = {FORMAT}\n\
         sample_rate = {sample_rate}\n\
         frames = {frames}\n\
         model = \"{}\"\n\
         weights_sha256 = \"{}\"\n\
         graph_sha256 = \"{}\"\n",
        stems::MODEL_ID,
        stems::WEIGHTS_SHA256,
        stems::GRAPH_SHA256
    )
}

fn manifest_matches(text: &str, key: &StemKey<'_>) -> bool {
    let Ok(table) = text.parse::<toml::Table>() else {
        return false;
    };
    let int = |k: &str| table.get(k).and_then(toml::Value::as_integer);
    let string = |k: &str| table.get(k).and_then(toml::Value::as_str);
    int("format") == Some(FORMAT)
        && int("sample_rate") == Some(i64::from(key.sample_rate))
        && int("frames") == i64::try_from(key.frames).ok()
        && string("model") == Some(stems::MODEL_ID)
        && string("weights_sha256") == Some(stems::WEIGHTS_SHA256)
        && string("graph_sha256") == Some(stems::GRAPH_SHA256)
}

/// Encodes the three stems into `dir` in parallel, then writes `stems.toml`.
fn write_entry(dir: &Path, set: &StemSet, sample_rate: u32) -> Result<(), LibraryError> {
    std::thread::scope(|scope| {
        let jobs: Vec<_> = FILES
            .iter()
            .map(|&(file, stem)| {
                let path = dir.join(file);
                let samples = set.stored(stem).unwrap_or_default();
                scope.spawn(move || encode(&path, samples, sample_rate))
            })
            .collect();
        jobs.into_iter().try_for_each(|job| {
            job.join()
                .unwrap_or_else(|_| Err(LibraryError::StemEncode("the encoder panicked".into())))
        })
    })?;
    // Written and flushed through one writable handle, dropped before the folder is
    // renamed. Windows: `sync_all` is `FlushFileBuffers`, which needs a handle with write
    // access; on a read-only `File::open` handle it fails with "Access is denied" (os 5).
    let mut manifest = File::create(dir.join(MANIFEST))?;
    manifest.write_all(manifest_text(sample_rate, set.frames()).as_bytes())?;
    manifest.sync_all()?;
    Ok(())
}

/// Encodes interleaved stereo `samples` as Ogg Vorbis at `path`.
fn encode(path: &Path, samples: &[i16], sample_rate: u32) -> Result<(), LibraryError> {
    let vorbis = |e: vorbis_rs::VorbisError| LibraryError::StemEncode(e.to_string());
    let rate = NonZeroU32::new(sample_rate)
        .ok_or_else(|| LibraryError::StemEncode("a sample rate of 0".into()))?;
    let build = |strategy| {
        let sink = BufWriter::new(File::create(path)?);
        VorbisEncoderBuilder::new_with_serial(rate, STEREO, sink, STREAM_SERIAL)
            .bitrate_management_strategy(strategy)
            .build()
            .map_err(vorbis)
    };
    // libvorbis has bitrate tables only for some rates (not 22.05, 88.2 or 96 kHz, for
    // example); elsewhere it takes a quality, 0.6 being about 192 kbit/s at 44.1 kHz.
    let mut encoder = build(VorbisBitrateManagementStrategy::Vbr {
        target_bitrate: TARGET_BITRATE,
    })
    .or_else(|_| {
        build(VorbisBitrateManagementStrategy::QualityVbr {
            target_quality: FALLBACK_QUALITY,
        })
    })?;
    let mut planar = [
        Vec::with_capacity(BLOCK_FRAMES),
        Vec::with_capacity(BLOCK_FRAMES),
    ];
    for block in samples.chunks(BLOCK_FRAMES * 2) {
        for channel in &mut planar {
            channel.clear();
        }
        for &[left, right] in block.as_chunks::<2>().0 {
            planar[0].push(f32::from(left) * I16_SCALE);
            planar[1].push(f32::from(right) * I16_SCALE);
        }
        encoder.encode_audio_block(&planar).map_err(vorbis)?;
    }
    let sink = encoder.finish().map_err(vorbis)?;
    let file = sink
        .into_inner()
        .map_err(std::io::IntoInnerError::into_error)?;
    file.sync_all()?;
    Ok(())
}

/// Decodes an entry's three stems one after another, so only one stem's decoded float
/// buffer is held at a time; `None` if any is unusable.
fn decode_entry(dir: &Path, key: &StemKey<'_>, kicks_hz: f32) -> Option<StemSet> {
    let vocals = decode_stem(&dir.join(FILES[0].0), key)?;
    let bass = decode_stem(&dir.join(FILES[1].0), key)?;
    let rhythm = decode_stem(&dir.join(FILES[2].0), key)?;
    StemSet::with_split(vocals, bass, rhythm, key.frames, key.sample_rate, kicks_hz).ok()
}

/// One stem at `key`'s rate and frame count, as 16-bit samples.
fn decode_stem(path: &Path, key: &StemKey<'_>) -> Option<Box<[i16]>> {
    let decoded = decode::decode_file(path).ok()?;
    let track = decoded.track;
    if decoded.truncated || track.sample_rate() != key.sample_rate || track.frames() != key.frames {
        return None;
    }
    let mut samples = Vec::with_capacity(key.frames * 2);
    for i in 0..key.frames {
        samples.extend(track.frame(i).map(to_i16));
    }
    Some(samples.into())
}

/// A decoded sample as 16-bit (clamped, rounded), the inverse of [`I16_SCALE`].
#[allow(clippy::cast_possible_truncation)]
fn to_i16(value: f32) -> i16 {
    // `as` saturates, and maps NaN to 0.
    (value * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_names_are_safe_to_delete() {
        let name = entry_name(12, &[0xab, 0x01]);
        assert_eq!(name, "12-ab01");
        assert!(is_entry_name(&name));
        for bad in [
            "coreml-cache",
            "12-",
            "-ab",
            "12-abc",
            "12-AB",
            "../1-ab",
            "1-ab/..",
            "",
        ] {
            assert!(!is_entry_name(bad), "{bad}");
        }
    }
}

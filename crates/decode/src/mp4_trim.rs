//! Gapless trim for MP4/M4A audio.
//!
//! symphonia 0.6 does not apply MP4 edit lists, so AAC encoder delay and padding are read from
//! the track's first real `elst` entry here. All parsing is bounds-checked and never panics.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Upper bound on the `moov` box we are willing to load.
const MAX_MOOV: u64 = 64 * 1024 * 1024;

/// Frames to drop at the start, and the total playable length, at the output sample rate.
#[derive(Debug, PartialEq, Eq)]
pub struct Trim {
    pub delay: usize,
    pub length: usize,
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        b.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// Iterates the ISO base media boxes in `data` as `(type, payload)`; stops at the first
/// malformed box.
fn boxes(data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let size = be32(data, at)? as usize;
        let kind = data.get(at.checked_add(4)?..at.checked_add(8)?)?;
        let (header, size) = match size {
            0 => (8, data.len() - at),
            1 => (16, usize::try_from(be64(data, at.checked_add(8)?)?).ok()?),
            n => (8, n),
        };
        if size < header {
            return None;
        }
        let end = at.checked_add(size)?;
        let payload = data.get(at + header..end)?;
        at = end;
        Some((kind, payload))
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _)| *k == kind).map(|(_, p)| p)
}

/// Reads the trim for `track_id` from an MP4 file, or `None` if the file is not MP4 or has no
/// usable edit list. Only the `moov` box is loaded, found by seeking over the other boxes.
pub fn read_trim(path: &Path, track_id: u32, rate: u32) -> Option<Trim> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut head = [0u8; 8];
    file.read_exact(&mut head).ok()?;
    if &head[4..8] != b"ftyp" {
        return None;
    }
    let mut at = 0u64;
    while at.checked_add(8)? <= len {
        file.seek(SeekFrom::Start(at)).ok()?;
        file.read_exact(&mut head).ok()?;
        let mut size = u64::from(u32::from_be_bytes(head[..4].try_into().ok()?));
        let mut header = 8u64;
        if size == 1 {
            let mut ext = [0u8; 8];
            file.read_exact(&mut ext).ok()?;
            size = u64::from_be_bytes(ext);
            header = 16;
        } else if size == 0 {
            size = len - at;
        }
        if size < header {
            return None;
        }
        if &head[4..8] == b"moov" {
            let payload = size - header;
            if payload > MAX_MOOV || at.checked_add(size)? > len {
                return None;
            }
            let mut moov = vec![0u8; usize::try_from(payload).ok()?];
            file.read_exact(&mut moov).ok()?;
            return parse_moov(&moov, track_id, rate);
        }
        at = at.checked_add(size)?;
    }
    None
}

/// Finds the trim for `track_id` in a `moov` payload.
pub fn parse_moov(moov: &[u8], track_id: u32, rate: u32) -> Option<Trim> {
    let mvhd = child(moov, b"mvhd")?;
    let movie_scale = u64::from(if *mvhd.first()? == 1 {
        be32(mvhd, 20)?
    } else {
        be32(mvhd, 12)?
    });
    let trak = boxes(moov)
        .filter(|(k, _)| *k == b"trak")
        .map(|(_, p)| p)
        .find(|trak| {
            child(trak, b"tkhd").is_some_and(|tkhd| {
                let id = if tkhd.first() == Some(&1) {
                    be32(tkhd, 20)
                } else {
                    be32(tkhd, 12)
                };
                id == Some(track_id)
            })
        })?;
    let mdhd = child(child(trak, b"mdia")?, b"mdhd")?;
    let media_scale = u64::from(if *mdhd.first()? == 1 {
        be32(mdhd, 20)?
    } else {
        be32(mdhd, 12)?
    });
    if movie_scale == 0 || media_scale == 0 {
        return None;
    }
    let elst = child(child(trak, b"edts")?, b"elst")?;
    let v1 = *elst.first()? == 1;
    let (mut at, count) = (8usize, be32(elst, 4)?);
    for _ in 0..count {
        let (duration, media_time, next) = if v1 {
            (
                be64(elst, at)?,
                be64(elst, at.checked_add(8)?)? as i64,
                at.checked_add(20)?,
            )
        } else {
            (
                u64::from(be32(elst, at)?),
                i64::from(be32(elst, at.checked_add(4)?)? as i32),
                at.checked_add(12)?,
            )
        };
        at = next;
        // A negative media time is an empty edit (silence); skip to the first real one.
        if let Ok(media_time) = u64::try_from(media_time) {
            let rate = u64::from(rate);
            let delay = media_time.checked_mul(rate)? / media_scale;
            let length = duration.checked_mul(rate)? / movie_scale;
            return Some(Trim {
                delay: usize::try_from(delay).ok()?,
                length: usize::try_from(length).ok()?,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    fn hdr(version: u8, fields: &[u32]) -> Vec<u8> {
        let mut v = vec![version, 0, 0, 0];
        v.extend(fields.iter().flat_map(|f| f.to_be_bytes()));
        v
    }

    /// v0 mvhd/mdhd: creation, modification, timescale, duration.
    fn mvhd(scale: u32) -> Vec<u8> {
        mk(b"mvhd", &hdr(0, &[0, 0, scale, 0]))
    }

    fn mdhd(scale: u32) -> Vec<u8> {
        mk(b"mdhd", &hdr(0, &[0, 0, scale, 0]))
    }

    fn tkhd(id: u32) -> Vec<u8> {
        mk(b"tkhd", &hdr(0, &[0, 0, id, 0]))
    }

    fn elst_v0(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut p = hdr(0, &[entries.len() as u32]);
        for &(d, t) in entries {
            p.extend(d.to_be_bytes());
            p.extend(t.to_be_bytes());
            p.extend(0x0001_0000u32.to_be_bytes());
        }
        mk(b"elst", &p)
    }

    fn trak(id: u32, media_scale: u32, elst: Option<Vec<u8>>) -> Vec<u8> {
        let mut p = tkhd(id);
        if let Some(e) = elst {
            p.extend(mk(b"edts", &e));
        }
        p.extend(mk(b"mdia", &mdhd(media_scale)));
        mk(b"trak", &p)
    }

    fn moov(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.iter().flatten().copied().collect()
    }

    #[test]
    fn reads_delay_and_length() {
        let m = moov(&[mvhd(1000), trak(1, 44100, Some(elst_v0(&[(1000, 1024)])))]);
        assert_eq!(
            parse_moov(&m, 1, 44100),
            Some(Trim {
                delay: 1024,
                length: 44100
            })
        );
    }

    #[test]
    fn skips_empty_edit_before_real_edit() {
        let m = moov(&[
            mvhd(1000),
            trak(1, 44100, Some(elst_v0(&[(500, -1), (2000, 2048)]))),
        ]);
        assert_eq!(
            parse_moov(&m, 1, 44100),
            Some(Trim {
                delay: 2048,
                length: 88200
            })
        );
    }

    #[test]
    fn reads_version_1_entries() {
        let mut p = vec![1, 0, 0, 0];
        p.extend(1u32.to_be_bytes());
        p.extend(1000u64.to_be_bytes());
        p.extend(1024i64.to_be_bytes());
        p.extend(0x0001_0000u32.to_be_bytes());
        let m = moov(&[mvhd(1000), trak(1, 44100, Some(mk(b"elst", &p)))]);
        assert_eq!(
            parse_moov(&m, 1, 44100),
            Some(Trim {
                delay: 1024,
                length: 44100
            })
        );
    }

    #[test]
    fn selects_trak_by_track_id() {
        let m = moov(&[
            mvhd(1000),
            trak(1, 90000, Some(elst_v0(&[(1000, 7)]))),
            trak(2, 44100, Some(elst_v0(&[(1000, 1024)]))),
        ]);
        assert_eq!(
            parse_moov(&m, 2, 44100),
            Some(Trim {
                delay: 1024,
                length: 44100
            })
        );
        assert_eq!(parse_moov(&m, 3, 44100), None);
    }

    #[test]
    fn converts_media_time_from_media_timescale() {
        // HE-AAC style: media timescale 22050, output 44100.
        let m = moov(&[mvhd(1000), trak(1, 22050, Some(elst_v0(&[(1000, 1024)])))]);
        assert_eq!(
            parse_moov(&m, 1, 44100),
            Some(Trim {
                delay: 2048,
                length: 44100
            })
        );
    }

    #[test]
    fn no_edit_list_is_none() {
        let m = moov(&[mvhd(1000), trak(1, 44100, None)]);
        assert_eq!(parse_moov(&m, 1, 44100), None);
    }

    #[test]
    fn truncated_boxes_are_none() {
        let m = moov(&[mvhd(1000), trak(1, 44100, Some(elst_v0(&[(1000, 1024)])))]);
        for cut in 0..m.len() {
            let _ = parse_moov(&m[..cut], 1, 44100);
        }
        assert_eq!(parse_moov(&m[..m.len() - 3], 1, 44100), None);
    }

    #[test]
    fn odd_box_sizes_do_not_panic() {
        for size in [0u32, 1, 4, 7, 8, u32::MAX] {
            let mut d = size.to_be_bytes().to_vec();
            d.extend_from_slice(b"mvhd");
            d.extend([0u8; 12]);
            let _ = boxes(&d).count();
            let _ = parse_moov(&d, 1, 44100);
        }
        // Size 1 with a 64-bit size of 0 or huge.
        for big in [0u64, 15, u64::MAX] {
            let mut d = 1u32.to_be_bytes().to_vec();
            d.extend_from_slice(b"mvhd");
            d.extend(big.to_be_bytes());
            let _ = parse_moov(&d, 1, 44100);
        }
    }

    #[test]
    fn random_bytes_do_not_panic() {
        let mut x = 0x1234_5678u32;
        for _ in 0..2000 {
            let data: Vec<u8> = (0..200)
                .map(|_| {
                    x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (x >> 24) as u8
                })
                .collect();
            let _ = parse_moov(&data, 1, 44100);
        }
    }
}

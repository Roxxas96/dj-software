//! Serato's tag-length-value container, used by crate files and `database V2`.

use super::super::ExportTrack;

/// One record: a 4-byte ASCII tag and a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub tag: [u8; 4],
    pub body: Body,
}

/// What a record holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Records nested inside, for tags that start with `o`.
    Records(Vec<Record>),
    /// Raw bytes, for every other tag.
    Bytes(Vec<u8>),
}

fn tag(s: &str) -> [u8; 4] {
    let mut t = [b' '; 4];
    for (d, b) in t.iter_mut().zip(s.bytes()) {
        *d = b;
    }
    t
}

/// A record holding `value` as UTF-16BE without a terminator.
#[must_use]
pub fn text(tag_name: &str, value: &str) -> Record {
    let bytes = value.encode_utf16().flat_map(u16::to_be_bytes).collect();
    Record {
        tag: tag(tag_name),
        body: Body::Bytes(bytes),
    }
}

/// A record holding `value` big-endian.
#[must_use]
pub fn u32_rec(tag_name: &str, value: u32) -> Record {
    Record {
        tag: tag(tag_name),
        body: Body::Bytes(value.to_be_bytes().to_vec()),
    }
}

fn container(tag_name: &str, records: Vec<Record>) -> Record {
    Record {
        tag: tag(tag_name),
        body: Body::Records(records),
    }
}

/// Serialises `records`: tag, `u32` big-endian length, body.
#[must_use]
pub fn encode(records: &[Record]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in records {
        let body = match &r.body {
            Body::Records(inner) => encode(inner),
            Body::Bytes(b) => b.clone(),
        };
        out.extend_from_slice(&r.tag);
        // Serato's records are small; a body over 4 GiB cannot occur from our inputs.
        out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
        out.extend_from_slice(&body);
    }
    out
}

/// Parses `bytes` into records, descending into tags that start with `o`.
///
/// # Errors
/// When a record's header or body runs past the end.
pub fn decode(bytes: &[u8]) -> Result<Vec<Record>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let header = bytes
            .get(at..at + 8)
            .ok_or_else(|| format!("a record header is cut short at byte {at}"))?;
        let t: [u8; 4] = [header[0], header[1], header[2], header[3]];
        let len = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let start = at + 8;
        let body = start
            .checked_add(len)
            .and_then(|end| bytes.get(start..end))
            .ok_or_else(|| format!("a record runs past the end at byte {at}"))?;
        let body = if t[0] == b'o' {
            Body::Records(decode(body)?)
        } else {
            Body::Bytes(body.to_vec())
        };
        out.push(Record { tag: t, body });
        at = start + len;
    }
    Ok(out)
}

/// A crate file listing `stick_paths` (relative to the stick root, no leading `/`).
#[must_use]
pub fn crate_file(stick_paths: &[&str]) -> Vec<u8> {
    let mut records = vec![text("vrsn", "1.0/Serato ScratchLive Crate")];
    for name in ["song", "artist", "bpm", "key"] {
        records.push(container(
            "ovct",
            vec![text("tvcn", name), text("tvcw", "0")],
        ));
    }
    for p in stick_paths {
        records.push(container("otrk", vec![text("ptrk", p)]));
    }
    encode(&records)
}

/// The `database V2` entry for `t` stored at `stick_path`; `added` is Unix seconds.
#[must_use]
pub fn database_record(t: &ExportTrack, stick_path: &str, added: u32) -> Record {
    let ext = match &t.kind {
        super::super::FileKind::Mp3 => "mp3".to_string(),
        super::super::FileKind::Wav => "wav".to_string(),
        super::super::FileKind::Other(e) => e.clone(),
    };
    let mut fields = vec![text("ttyp", &ext), text("pfil", stick_path)];
    fields.push(text("tsng", &t.title));
    if let Some(v) = &t.artist {
        fields.push(text("tart", v));
    }
    if let Some(v) = &t.album {
        fields.push(text("talb", v));
    }
    if let Some(v) = &t.genre {
        fields.push(text("tgen", v));
    }
    if let Some(bpm) = t.bpm {
        fields.push(text("tbpm", &format!("{bpm:.2}")));
    }
    if let Some(v) = &t.key {
        fields.push(text("tkey", v));
    }
    fields.push(u32_rec("uadd", added));
    container("otrk", fields)
}

fn utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
}

fn is_ours(r: &Record) -> bool {
    if &r.tag != b"otrk" {
        return false;
    }
    let Body::Records(fields) = &r.body else {
        return false;
    };
    fields.iter().any(|f| {
        &f.tag == b"pfil"
            && matches!(&f.body, Body::Bytes(b) if utf16be(b).starts_with("Open Jockey/"))
    })
}

/// Merges our records into an existing `database V2`: foreign records stay in order, any of
/// our earlier `otrk` records (path under `Open Jockey/`) go, and `ours` is appended.
///
/// # Errors
/// When `existing` is not a well-formed record stream.
pub fn merge_database(existing: Option<&[u8]>, ours: Vec<Record>) -> Result<Vec<u8>, String> {
    let mut out = vec![text("vrsn", "2.0/Serato Scratch LIVE Database")];
    if let Some(bytes) = existing {
        for r in decode(bytes)? {
            if &r.tag == b"vrsn" || is_ours(&r) {
                continue;
            }
            out.push(r);
        }
    }
    out.extend(ours);
    Ok(encode(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn otrk(path: &str) -> Record {
        container("otrk", vec![text("pfil", path)])
    }

    #[test]
    fn crate_bytes() {
        let bytes = crate_file(&["Open Jockey/Music/A/x.mp3"]);
        assert_eq!(bytes[..8], [0x76, 0x72, 0x73, 0x6E, 0, 0, 0, 0x38]);
        let recs = decode(&bytes).unwrap();
        assert_eq!(recs.len(), 6);
        assert_eq!(&recs[0].tag, b"vrsn");
        assert!(recs[1..5].iter().all(|r| &r.tag == b"ovct"));
        assert_eq!(&recs[5].tag, b"otrk");
        assert_eq!(
            recs[5],
            container("otrk", vec![text("ptrk", "Open Jockey/Music/A/x.mp3")])
        );
        let Body::Records(cols) = &recs[1].body else {
            panic!("ovct holds records");
        };
        assert_eq!(cols, &vec![text("tvcn", "song"), text("tvcw", "0")]);
    }

    #[test]
    fn merge_keeps_foreign_records() {
        let foreign = otrk("Music/friend.mp3");
        let old = otrk("Open Jockey/Music/A/x.mp3");
        let existing = encode(&[
            text("vrsn", "2.0/Serato Scratch LIVE Database"),
            foreign.clone(),
            old,
        ]);
        let new = otrk("Open Jockey/Music/B/y.mp3");
        let merged = merge_database(Some(&existing), vec![new.clone()]).unwrap();
        let want = encode(&[
            text("vrsn", "2.0/Serato Scratch LIVE Database"),
            foreign,
            new,
        ]);
        assert_eq!(merged, want);
        assert_eq!(merge_database(None, vec![]).unwrap()[..4], *b"vrsn");
    }

    #[test]
    fn merge_refuses_garbage() {
        assert!(merge_database(Some(b"\x00\x01"), vec![]).is_err());
    }

    #[test]
    fn decode_refuses_overlong_record() {
        assert!(decode(b"abcd\x00\x00\x00\x09xx").is_err());
    }
}

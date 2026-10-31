//! Playlists and the paged track queries behind the library and set-prep views.
//!
//! Playlist ops run inside the writer's transaction and keep positions dense (0..n). A
//! reorder loads the playlist's entries, rearranges them in memory and rewrites the rows,
//! so the composite primary key `(playlist_id, position)` never sees a transient clash.

use std::path::{MAIN_SEPARATOR, PathBuf};

use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Row, Transaction, params_from_iter};

use crate::writer::{WriteOp, WriteOutcome};
use crate::{LibraryError, Reader};

/// A playlist in the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistSummary {
    pub id: i64,
    pub name: String,
    /// Entries, counting a track once per time it was added.
    pub track_count: u32,
    /// Entries whose track has neither a cue nor any hot cue.
    pub unprepared_count: u32,
}

/// One row of a track list.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRow {
    pub id: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub bpm: Option<f64>,
    /// The raw stored key, 0..=23.
    pub key: Option<u8>,
    pub duration_frames: Option<u64>,
    pub sample_rate: Option<u32>,
    /// The track has a cue point or at least one hot cue.
    pub has_cues: bool,
    pub missing: bool,
}

/// One track with its file, as [`Reader::track`] returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackDetails {
    pub row: TrackRow,
    pub path: PathBuf,
}

/// Which tracks [`Reader::tracks`] returns. Every set field must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackFilter {
    /// Only tracks whose file lives under this folder.
    pub folder: Option<PathBuf>,
    /// Case-insensitive (ASCII) substring of the title or artist; `%` and `_` are literal.
    pub text: Option<String>,
    /// `Some(true)` only missing tracks, `Some(false)` only present ones.
    pub missing: Option<bool>,
}

/// The column a track list is ordered by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TrackSortKey {
    Title,
    #[default]
    Artist,
    Bpm,
    Key,
    Added,
}

/// Ordering of a track list. Ties break by track id, so paging is stable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrackSort {
    pub key: TrackSortKey,
    pub descending: bool,
}

const TRACK_COLUMNS: &str = "t.id, t.title, t.artist, a.bpm, a.key, t.duration_frames, \
     t.sample_rate, \
     (EXISTS (SELECT 1 FROM cues c WHERE c.track_id = t.id) \
      OR EXISTS (SELECT 1 FROM hotcues h WHERE h.track_id = t.id)), \
     t.missing";

const UNPREPARED: &str = "NOT EXISTS (SELECT 1 FROM cues c WHERE c.track_id = t.id) \
     AND NOT EXISTS (SELECT 1 FROM hotcues h WHERE h.track_id = t.id)";

fn track_row(row: &Row<'_>) -> rusqlite::Result<TrackRow> {
    Ok(TrackRow {
        id: row.get(0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        bpm: row.get(3)?,
        key: row
            .get::<_, Option<i64>>(4)?
            .and_then(|k| u8::try_from(k).ok()),
        duration_frames: row
            .get::<_, Option<i64>>(5)?
            .and_then(|f| u64::try_from(f).ok()),
        sample_rate: row
            .get::<_, Option<i64>>(6)?
            .and_then(|r| u32::try_from(r).ok()),
        has_cues: row.get(7)?,
        missing: row.get(8)?,
    })
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Escapes `%`, `_` and the escape character itself for a `LIKE ... ESCAPE '\'` pattern.
fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Reader {
    /// Every playlist in list order, with its entry counts.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn playlists(&self) -> Result<Vec<PlaylistSummary>, LibraryError> {
        let sql = format!(
            "SELECT p.id, p.name, count(t.id), coalesce(sum(t.id IS NOT NULL AND {UNPREPARED}), 0) \
             FROM playlists p \
             LEFT JOIN playlist_tracks pt ON pt.playlist_id = p.id \
             LEFT JOIN tracks t ON t.id = pt.track_id \
             GROUP BY p.id ORDER BY p.position, p.id"
        );
        let mut stmt = self.connection().prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok(PlaylistSummary {
                id: r.get(0)?,
                name: r.get(1)?,
                track_count: r.get(2)?,
                unprepared_count: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Track `id` with its file path, or `None` if there is no such track.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn track(&self, id: i64) -> Result<Option<TrackDetails>, LibraryError> {
        let sql = format!(
            "SELECT {TRACK_COLUMNS}, t.path FROM tracks t \
             LEFT JOIN analysis a ON a.track_id = t.id WHERE t.id = ?1"
        );
        Ok(self
            .connection()
            .prepare_cached(&sql)?
            .query_row([id], |r| {
                Ok(TrackDetails {
                    row: track_row(r)?,
                    path: PathBuf::from(r.get::<_, String>(9)?),
                })
            })
            .optional()?)
    }

    /// A page of a playlist's entries in playlist order.
    ///
    /// # Errors
    ///
    /// Any database error.
    pub fn playlist_tracks(
        &self,
        id: i64,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TrackRow>, LibraryError> {
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM playlist_tracks pt \
             JOIN tracks t ON t.id = pt.track_id \
             LEFT JOIN analysis a ON a.track_id = t.id \
             WHERE pt.playlist_id = ?1 ORDER BY pt.position LIMIT ?2 OFFSET ?3"
        );
        let mut stmt = self.connection().prepare_cached(&sql)?;
        let rows = stmt.query_map((id, to_i64(limit), to_i64(offset)), track_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// A page of library tracks matching `filter`, ordered by `sort` (ties by id).
    ///
    /// # Errors
    ///
    /// Any database error, or [`LibraryError::NonUtf8Path`] for a folder filter that is
    /// not valid UTF-8.
    pub fn tracks(
        &self,
        filter: &TrackFilter,
        sort: TrackSort,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<TrackRow>, LibraryError> {
        let (sql, params) = tracks_query(filter, sort, offset, limit)?;
        let mut stmt = self.connection().prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(&params), track_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Where track `id` is in the list [`Reader::tracks`] returns for `filter` and `sort`
    /// (from 0), or `None` if it does not match `filter`. One count query, served by the
    /// sort column's index where there is one.
    ///
    /// # Errors
    ///
    /// Any database error, or [`LibraryError::NonUtf8Path`] for a folder filter that is
    /// not valid UTF-8.
    pub fn track_position(
        &self,
        filter: &TrackFilter,
        sort: TrackSort,
        id: i64,
    ) -> Result<Option<usize>, LibraryError> {
        let (where_clause, params) = filter_clause(filter)?;
        let and = if where_clause.is_empty() {
            "WHERE".to_owned()
        } else {
            format!("{where_clause} AND")
        };
        let member_sql = format!("SELECT count(*) FROM tracks t {and} t.id = ?");
        let mut member_params = params.clone();
        member_params.push(Value::Integer(id));
        let member: i64 = self
            .connection()
            .prepare_cached(&member_sql)?
            .query_row(params_from_iter(&member_params), |r| r.get(0))?;
        if member == 0 {
            return Ok(None);
        }
        let (column, value) = match sort.key {
            TrackSortKey::Title => ("t.title COLLATE NOCASE", "t.title"),
            TrackSortKey::Artist => ("t.artist COLLATE NOCASE", "t.artist"),
            TrackSortKey::Bpm => ("a.bpm", "a.bpm"),
            TrackSortKey::Key => ("a.key", "a.key"),
            TrackSortKey::Added => ("t.added_at", "t.added_at"),
        };
        // Rows before the target in `ORDER BY column dir, id dir`, where SQLite puts NULLs
        // first ascending and last descending.
        let before = if sort.descending {
            format!(
                "(({column}) IS NOT NULL AND x.v IS NULL) OR ({column}) > x.v \
                 OR (({column}) IS x.v AND t.id > x.id)"
            )
        } else {
            format!(
                "(({column}) IS NULL AND x.v IS NOT NULL) OR ({column}) < x.v \
                 OR (({column}) IS x.v AND t.id < x.id)"
            )
        };
        let sql = format!(
            "SELECT count(*) FROM tracks t LEFT JOIN analysis a ON a.track_id = t.id, \
                 (SELECT {value} AS v, t.id AS id FROM tracks t \
                  LEFT JOIN analysis a ON a.track_id = t.id WHERE t.id = ?) x \
             {and} ({before})"
        );
        let mut all_params = vec![Value::Integer(id)];
        all_params.extend(params);
        let n: i64 = self
            .connection()
            .prepare_cached(&sql)?
            .query_row(params_from_iter(&all_params), |r| r.get(0))?;
        Ok(Some(usize::try_from(n).unwrap_or(0)))
    }

    /// How many library tracks match `filter`, without loading them.
    ///
    /// # Errors
    ///
    /// Any database error, or [`LibraryError::NonUtf8Path`] for a folder filter that is
    /// not valid UTF-8.
    pub fn track_count(&self, filter: &TrackFilter) -> Result<usize, LibraryError> {
        let (where_clause, params) = filter_clause(filter)?;
        let sql = format!("SELECT count(*) FROM tracks t {where_clause}");
        let mut stmt = self.connection().prepare_cached(&sql)?;
        let n: i64 = stmt.query_row(params_from_iter(&params), |r| r.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// The `EXPLAIN QUERY PLAN` detail lines of the exact statement [`Reader::tracks`]
    /// runs, to test that a sort is served by an index.
    ///
    /// # Errors
    ///
    /// Any database error.
    #[doc(hidden)]
    pub fn tracks_query_plan(
        &self,
        filter: &TrackFilter,
        sort: TrackSort,
    ) -> Result<Vec<String>, LibraryError> {
        let (sql, params) = tracks_query(filter, sort, 0, 100)?;
        let mut stmt = self
            .connection()
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        let rows = stmt.query_map(params_from_iter(&params), |r| r.get::<_, String>(3))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The `WHERE` clause (empty when nothing is filtered) and its parameters selecting the
/// tracks (aliased `t`) that match `filter`.
fn filter_clause(filter: &TrackFilter) -> Result<(String, Vec<Value>), LibraryError> {
    let mut conds: Vec<&str> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    if let Some(folder) = &filter.folder {
        let folder = folder
            .to_str()
            .ok_or_else(|| LibraryError::NonUtf8Path(folder.clone()))?;
        let prefix = format!(
            "{}{MAIN_SEPARATOR}",
            folder.trim_end_matches(MAIN_SEPARATOR)
        );
        // Range on the unique path index: everything from "dir/" up to "dir0" ('/' + 1).
        let upper = format!("{}{}", prefix.trim_end_matches(MAIN_SEPARATOR), {
            char::from(u8::try_from(MAIN_SEPARATOR).unwrap_or(b'/') + 1)
        });
        conds.push("t.path >= ? AND t.path < ?");
        params.push(Value::Text(prefix));
        params.push(Value::Text(upper));
    }
    if let Some(text) = filter.text.as_deref().filter(|t| !t.is_empty()) {
        let pattern = Value::Text(format!("%{}%", escape_like(text)));
        conds.push("(t.title LIKE ? ESCAPE '\\' OR t.artist LIKE ? ESCAPE '\\')");
        params.push(pattern.clone());
        params.push(pattern);
    }
    if let Some(missing) = filter.missing {
        conds.push("t.missing = ?");
        params.push(Value::Integer(i64::from(missing)));
    }
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conds.join(" AND "))
    };
    Ok((where_clause, params))
}

/// The SQL and parameters for one page of [`Reader::tracks`].
fn tracks_query(
    filter: &TrackFilter,
    sort: TrackSort,
    offset: usize,
    limit: usize,
) -> Result<(String, Vec<Value>), LibraryError> {
    let (where_clause, mut params) = filter_clause(filter)?;
    let column = match sort.key {
        TrackSortKey::Title => "t.title COLLATE NOCASE",
        TrackSortKey::Artist => "t.artist COLLATE NOCASE",
        TrackSortKey::Bpm => "a.bpm",
        TrackSortKey::Key => "a.key",
        TrackSortKey::Added => "t.added_at",
    };
    let dir = if sort.descending { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT p.id, p.title, p.artist, p.bpm, p.key, p.duration_frames, p.sample_rate, \
                 (EXISTS (SELECT 1 FROM cues c WHERE c.track_id = p.id) \
                  OR EXISTS (SELECT 1 FROM hotcues h WHERE h.track_id = p.id)), \
                 p.missing \
             FROM (SELECT t.id, t.title, t.artist, a.bpm, a.key, t.duration_frames, \
                          t.sample_rate, t.missing \
                   FROM tracks t LEFT JOIN analysis a ON a.track_id = t.id \
                   {where_clause} ORDER BY {column} {dir}, t.id {dir} LIMIT ? OFFSET ?) p"
    );
    params.push(Value::Integer(to_i64(limit)));
    params.push(Value::Integer(to_i64(offset)));
    Ok((sql, params))
}

/// Applies a playlist op inside the writer's transaction.
pub(crate) fn apply(tx: &Transaction<'_>, op: &WriteOp) -> Result<WriteOutcome, LibraryError> {
    match op {
        WriteOp::CreatePlaylist { name } => {
            let next: i64 = tx.query_row(
                "SELECT coalesce(max(position) + 1, 0) FROM playlists",
                [],
                |r| r.get(0),
            )?;
            tx.execute(
                "INSERT INTO playlists(name, position) VALUES (?1, ?2)",
                (name, next),
            )?;
            Ok(WriteOutcome::Created {
                id: tx.last_insert_rowid(),
            })
        }
        WriteOp::RenamePlaylist { id, name } => {
            let changed = tx.execute("UPDATE playlists SET name = ?2 WHERE id = ?1", (id, name))?;
            require_playlist(changed, *id)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::DeletePlaylist(id) => {
            tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id])?;
            let changed = tx.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
            require_playlist(changed, *id)?;
            renumber_playlists(tx, &playlist_ids(tx)?)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::MovePlaylist { id, to_position } => {
            let mut ids = playlist_ids(tx)?;
            let from = ids
                .iter()
                .position(|p| p == id)
                .ok_or(LibraryError::NoSuchPlaylist(*id))?;
            let moved = ids.remove(from);
            ids.insert((*to_position).min(ids.len()), moved);
            renumber_playlists(tx, &ids)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::AddToPlaylist {
            playlist,
            track,
            at,
        } => {
            let mut entries = entries(tx, *playlist)?;
            let known: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM tracks WHERE id = ?1)",
                [track],
                |r| r.get(0),
            )?;
            if !known {
                return Err(LibraryError::NoSuchTrack(*track));
            }
            entries.insert(at.unwrap_or(usize::MAX).min(entries.len()), *track);
            rewrite(tx, *playlist, &entries)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::RemoveFromPlaylist { playlist, position } => {
            let mut entries = entries(tx, *playlist)?;
            check_position(*position, entries.len())?;
            entries.remove(*position);
            rewrite(tx, *playlist, &entries)?;
            Ok(WriteOutcome::Done)
        }
        WriteOp::MoveInPlaylist { playlist, from, to } => {
            let mut entries = entries(tx, *playlist)?;
            check_position(*from, entries.len())?;
            let moved = entries.remove(*from);
            entries.insert((*to).min(entries.len()), moved);
            rewrite(tx, *playlist, &entries)?;
            Ok(WriteOutcome::Done)
        }
        _ => unreachable!("only playlist ops are routed here"),
    }
}

fn require_playlist(changed: usize, id: i64) -> Result<(), LibraryError> {
    if changed == 0 {
        Err(LibraryError::NoSuchPlaylist(id))
    } else {
        Ok(())
    }
}

fn check_position(position: usize, len: usize) -> Result<(), LibraryError> {
    if position >= len {
        Err(LibraryError::PositionOutOfRange { position, len })
    } else {
        Ok(())
    }
}

fn playlist_ids(tx: &Transaction<'_>) -> Result<Vec<i64>, LibraryError> {
    let mut stmt = tx.prepare_cached("SELECT id FROM playlists ORDER BY position, id")?;
    let ids = stmt.query_map([], |r| r.get(0))?;
    Ok(ids.collect::<Result<_, _>>()?)
}

fn renumber_playlists(tx: &Transaction<'_>, ids: &[i64]) -> Result<(), LibraryError> {
    let mut stmt = tx.prepare_cached("UPDATE playlists SET position = ?2 WHERE id = ?1")?;
    for (position, id) in ids.iter().enumerate() {
        stmt.execute((id, to_i64(position)))?;
    }
    Ok(())
}

/// A playlist's track ids in order; errors if the playlist does not exist.
fn entries(tx: &Transaction<'_>, playlist: i64) -> Result<Vec<i64>, LibraryError> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM playlists WHERE id = ?1)",
        [playlist],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(LibraryError::NoSuchPlaylist(playlist));
    }
    let mut stmt = tx.prepare_cached(
        "SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
    )?;
    let ids = stmt.query_map([playlist], |r| r.get(0))?;
    Ok(ids.collect::<Result<_, _>>()?)
}

/// Replaces a playlist's rows with `tracks` at positions 0..n.
fn rewrite(tx: &Transaction<'_>, playlist: i64, tracks: &[i64]) -> Result<(), LibraryError> {
    tx.execute(
        "DELETE FROM playlist_tracks WHERE playlist_id = ?1",
        [playlist],
    )?;
    let mut stmt = tx.prepare_cached(
        "INSERT INTO playlist_tracks(playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
    )?;
    for (position, track) in tracks.iter().enumerate() {
        stmt.execute((playlist, track, to_i64(position)))?;
    }
    Ok(())
}

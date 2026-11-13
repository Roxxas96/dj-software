//! The browser's folder tree: the drives and the folders opened below them, read from the
//! disk, then the playlists.
//!
//! The tree is held as its visible rows. Opening a folder reads its subfolders and inserts
//! them below it; closing it removes every row below it, so a closed branch is read again
//! when it next opens.

use std::fs;
use std::path::{Path, PathBuf};

use super::Drive;
use crate::PlaylistSummary;
use crate::scan::is_hidden;

/// What a tree row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeKind {
    Drive,
    Folder,
    /// The node the playlists sit under.
    Playlists,
    Playlist,
}

/// One visible row of the folder tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub label: String,
    /// 0 for the drives and the Playlists node.
    pub depth: u8,
    pub kind: TreeKind,
    /// Open, showing its children (the Playlists node always is).
    pub expanded: bool,
    /// Why an open folder shows no children, such as "cannot read".
    pub note: Option<&'static str>,
    /// The library id of a playlist's row; `None` for every other row.
    pub playlist: Option<i64>,
    /// A folder row's folder on disk, as the tree read it; `None` for every other row.
    pub path: Option<PathBuf>,
}

/// What a tree row's list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    /// A drive's or folder's own audio files.
    Dir(PathBuf),
    /// Nothing: the Playlists node.
    Playlists,
    /// A playlist and its entry count.
    Playlist { id: i64, len: usize },
}

impl Target {
    /// Whether `self` and `other` are the same entry, even if its contents changed.
    pub(super) fn same_entry(&self, other: &Target) -> bool {
        match (self, other) {
            (Target::Playlist { id: a, .. }, Target::Playlist { id: b, .. }) => a == b,
            _ => self == other,
        }
    }
}

#[derive(Debug, Clone)]
struct Node {
    row: TreeRow,
    target: Target,
}

/// The visible tree rows. Never empty: the Playlists node is always there.
#[derive(Debug, Clone)]
pub(super) struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    /// The drives (closed), then the Playlists node, open, with `playlists` below it.
    pub(super) fn new(roots: Vec<Drive>, playlists: &[PlaylistSummary]) -> Self {
        let mut nodes: Vec<Node> = roots
            .into_iter()
            .map(|d| Node {
                row: row(d.name, 0, TreeKind::Drive),
                target: Target::Dir(d.path),
            })
            .collect();
        let mut playlists_node = row("Playlists".to_owned(), 0, TreeKind::Playlists);
        playlists_node.expanded = true;
        nodes.push(Node {
            row: playlists_node,
            target: Target::Playlists,
        });
        let mut tree = Self { nodes };
        tree.set_playlists(playlists);
        tree
    }

    /// Replaces the drives with `roots`, in their order. A drive still there (at the same
    /// path) keeps its row and the folders open below it; a new one comes in closed.
    pub(super) fn set_drives(&mut self, roots: Vec<Drive>) {
        let at = self
            .nodes
            .iter()
            .position(|n| n.target == Target::Playlists)
            .unwrap_or(self.nodes.len() - 1);
        let old: Vec<Node> = self.nodes.drain(..at).collect();
        let mut drives = Vec::with_capacity(old.len().max(roots.len()));
        for drive in roots {
            let target = Target::Dir(drive.path);
            match old
                .iter()
                .position(|n| n.row.depth == 0 && n.target == target)
            {
                Some(i) => {
                    let start = drives.len();
                    drives.extend(old[i..subtree_end(&old, i)].iter().cloned());
                    drives[start].row.label = drive.name;
                }
                None => drives.push(Node {
                    row: row(drive.name, 0, TreeKind::Drive),
                    target,
                }),
            }
        }
        self.nodes.splice(0..0, drives);
    }

    pub(super) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(super) fn target(&self, index: usize) -> &Target {
        &self.nodes[index].target
    }

    /// Where `target` is in the tree, if it is visible.
    pub(super) fn position(&self, target: &Target) -> Option<usize> {
        self.nodes.iter().position(|n| n.target.same_entry(target))
    }

    pub(super) fn rows(&self) -> Vec<TreeRow> {
        self.nodes.iter().map(|n| n.row.clone()).collect()
    }

    /// The name of the drive `path` is on: the drive whose folder holds it most closely.
    pub(super) fn drive_of(&self, path: &Path) -> Option<&str> {
        self.nodes
            .iter()
            .filter(|n| n.row.kind == TreeKind::Drive)
            .filter_map(|n| match &n.target {
                Target::Dir(dir) if path.starts_with(dir) => Some((dir, &n.row.label)),
                _ => None,
            })
            .max_by_key(|(dir, _)| dir.components().count())
            .map(|(_, label)| label.as_str())
    }

    /// Takes out the folders that are no longer on disk (a folder moved to the Trash), with
    /// the rows below them. Drives come and go with [`Tree::set_drives`] instead.
    pub(super) fn drop_gone_folders(&mut self) {
        let mut at = 0;
        while at < self.nodes.len() {
            let node = &self.nodes[at];
            let gone = node.row.kind == TreeKind::Folder
                && matches!(&node.target, Target::Dir(dir) if is_gone(dir));
            if gone {
                let end = self.subtree_end(at);
                self.nodes.drain(at..end);
            } else {
                at += 1;
            }
        }
    }

    /// Row `index`'s label after its ancestors', such as `"Music ▸ House"`.
    pub(super) fn path(&self, index: usize) -> String {
        let mut labels = vec![self.nodes[index].row.label.as_str()];
        let mut depth = self.nodes[index].row.depth;
        for node in self.nodes[..index].iter().rev() {
            if depth == 0 {
                break;
            }
            if node.row.depth < depth {
                depth = node.row.depth;
                labels.push(&node.row.label);
            }
        }
        labels.reverse();
        labels.join(" ▸ ")
    }

    /// Opens or closes the drive or folder at `index`; anything else is left alone.
    ///
    /// Opening reads the folder's subfolders: hidden ones are skipped and the rest sorted
    /// by name, ignoring case. A folder that can't be read opens with no children and the
    /// note "cannot read".
    pub(super) fn toggle(&mut self, index: usize) {
        let Target::Dir(dir) = &self.nodes[index].target else {
            return;
        };
        if self.nodes[index].row.expanded {
            let end = self.subtree_end(index);
            self.nodes.drain(index + 1..end);
            let closed = &mut self.nodes[index].row;
            closed.expanded = false;
            closed.note = None;
            return;
        }
        let read = subfolders(dir);
        let opened = &mut self.nodes[index].row;
        opened.expanded = true;
        let depth = opened.depth.saturating_add(1);
        match read {
            Ok(children) => {
                let children = children.into_iter().map(|(name, path)| Node {
                    row: TreeRow {
                        path: Some(path.clone()),
                        ..row(name, depth, TreeKind::Folder)
                    },
                    target: Target::Dir(path),
                });
                self.nodes.splice(index + 1..index + 1, children);
            }
            Err(_) => opened.note = Some("cannot read"),
        }
    }

    /// Replaces the rows under the Playlists node with `playlists`, in their order.
    pub(super) fn set_playlists(&mut self, playlists: &[PlaylistSummary]) {
        let at = self
            .nodes
            .iter()
            .position(|n| n.target == Target::Playlists)
            .unwrap_or(self.nodes.len() - 1);
        let end = self.subtree_end(at);
        let rows = playlists.iter().map(|p| Node {
            row: TreeRow {
                playlist: Some(p.id),
                ..row(p.name.clone(), 1, TreeKind::Playlist)
            },
            target: Target::Playlist {
                id: p.id,
                len: usize::try_from(p.track_count).unwrap_or(usize::MAX),
            },
        });
        self.nodes.splice(at + 1..end, rows);
    }

    /// The index just past the rows below row `index`.
    pub(super) fn subtree_end(&self, index: usize) -> usize {
        subtree_end(&self.nodes, index)
    }
}

/// The index just past the rows below `nodes[index]`.
fn subtree_end(nodes: &[Node], index: usize) -> usize {
    let depth = nodes[index].row.depth;
    nodes[index + 1..]
        .iter()
        .position(|n| n.row.depth <= depth)
        .map_or(nodes.len(), |i| index + 1 + i)
}

fn row(label: String, depth: u8, kind: TreeKind) -> TreeRow {
    TreeRow {
        label,
        depth,
        kind,
        expanded: false,
        note: None,
        playlist: None,
        path: None,
    }
}

/// True only when `dir` is known not to exist (not when it cannot be checked).
fn is_gone(dir: &Path) -> bool {
    matches!(fs::symlink_metadata(dir), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// `dir`'s visible subfolders (links to folders included) as `(name, path)`, sorted by
/// name ignoring case.
fn subfolders(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut folders = Vec::new();
    for entry in fs::read_dir(dir)?.flatten() {
        let name = entry.file_name();
        if is_hidden(&name) {
            continue;
        }
        let path = entry.path();
        let is_dir = match entry.file_type() {
            Ok(t) if t.is_symlink() => path.is_dir(),
            Ok(t) => t.is_dir(),
            Err(_) => false,
        };
        if is_dir {
            folders.push((name.to_string_lossy().into_owned(), path));
        }
    }
    folders.sort_by_cached_key(|(name, _)| name.to_lowercase());
    Ok(folders)
}

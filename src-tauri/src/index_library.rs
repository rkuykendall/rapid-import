//! Builds/refreshes the SQLite index (`db.rs`) from a destination library
//! that already has content on disk — as opposed to `scan.rs`, which only
//! ever *reads* that index to check a source file against it. Once a
//! destination has been indexed here, `scan::is_already_imported` and
//! `commit.rs`'s `DuplicatePolicy::Skip` already do the right thing with no
//! further changes; this module's only job is populating/refreshing that
//! index cheaply, without rehashing content that hasn't actually changed.
//!
//! Deliberately takes a `db_path` rather than a live `&Connection`: a
//! full-library backfill can run for a long time, and holding the app's one
//! shared connection for that whole run would block every other command.
//! Instead this opens short-lived connections at each phase boundary —
//! which also keeps `bin/reindex_cli.rs` trivial (just needs a path).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use chrono::Utc;
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::db;
use crate::dedup;
use crate::formats::is_supported_media_file;

/// Files are walked and committed in chunks rather than one
/// walk-everything-then-hash-everything-then-write-everything pass, so a
/// crash or quit mid-run loses at most one chunk's worth of work, not the
/// whole backfill.
const CHUNK_SIZE: usize = 2000;

pub struct IndexOptions<'a> {
    pub destination_root: &'a Path,
    /// Must be `destination_root` itself or a descendant of it — validated
    /// up front. `None` reindexes the whole library.
    pub scope_root: Option<&'a Path>,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct IndexSummary {
    pub batch_id: i64,
    /// Path matched an indexed row with the same size+mtime — no bytes read.
    pub unchanged: usize,
    /// Matched an indexed row by file identity at a different path, same
    /// size — path updated, no rehash.
    pub moved: usize,
    /// Matched an indexed row (by path or file identity) but size/mtime
    /// differ — rehashed.
    pub content_changed: usize,
    /// No matching row at all — hashed and inserted.
    pub new: usize,
    /// Previously-indexed rows under the scope that no longer correspond to
    /// any file found in this walk.
    pub removed: usize,
}

pub fn index_library(db_path: &Path, options: &IndexOptions) -> Result<IndexSummary> {
    index_library_with_progress(db_path, options, |_| {})
}

pub fn index_library_with_progress(
    db_path: &Path,
    options: &IndexOptions,
    mut on_item: impl FnMut(usize),
) -> Result<IndexSummary> {
    let effective_root = resolve_scope_root(options)?;

    let (all_rows, batch_id) = {
        let conn = db::open(db_path)?;
        let all_rows = db::all_indexed_files(&conn)?;
        let batch_id = db::insert_batch(
            &conn,
            &db::NewBatch {
                started_at: Utc::now().to_rfc3339(),
                profile_id: None,
                kind: "index".to_string(),
                // An index run never touches a file on disk, so there's
                // nothing to undo and nothing to log.
                undo_log_path: String::new(),
            },
        )?;
        (all_rows, batch_id)
    };

    // `by_path` only holds rows under the scope being walked (what this
    // walk needs to reconcile against). `by_file_id` holds every indexed
    // row regardless of scope — a move can land *into* the current scope
    // from somewhere outside it.
    let mut by_path: HashMap<PathBuf, db::IndexedFileMeta> = HashMap::new();
    let mut by_file_id: HashMap<String, db::IndexedFileMeta> = HashMap::new();
    for row in all_rows {
        if let Some(file_id) = row.file_id.clone() {
            by_file_id.entry(file_id).or_insert_with(|| row.clone());
        }
        let path = PathBuf::from(&row.current_path);
        if path.starts_with(&effective_root) {
            by_path.insert(path, row);
        }
    }

    let walked_paths: Vec<PathBuf> = WalkDir::new(&effective_root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| is_supported_media_file(entry.path()))
        .map(|entry| entry.path().to_path_buf())
        .collect();

    let mut summary = IndexSummary { batch_id, ..Default::default() };
    let mut seen_ids: HashSet<i64> = HashSet::new();
    let mut walked = 0usize;

    for chunk in walked_paths.chunks(CHUNK_SIZE) {
        let mut moves: Vec<PendingMove> = Vec::new();
        let mut to_hash: Vec<PendingItem> = Vec::new();

        for path in chunk {
            walked += 1;
            let Ok(meta) = fs::metadata(path) else {
                // Vanished between the walk listing it and stat-ing it —
                // just skip it this run, next reindex will reconcile it.
                continue;
            };
            let size = Some(meta.len() as i64);
            let mtime = dedup::mtime_secs(&meta);
            let file_id = file_identity(path, &meta);

            if let Some(row) = by_path.get(path) {
                seen_ids.insert(row.id);
                if row.size == size && row.mtime == mtime {
                    summary.unchanged += 1;
                } else {
                    to_hash.push(PendingItem::UpdateContent {
                        id: row.id,
                        path: path.clone(),
                        size,
                        mtime,
                        file_id,
                    });
                }
                continue;
            }

            if let Some(row) = file_id.as_ref().and_then(|id| by_file_id.get(id))
                && !seen_ids.contains(&row.id)
            {
                seen_ids.insert(row.id);
                if row.size == size {
                    moves.push(PendingMove { id: row.id, new_path: path.clone(), size, mtime });
                } else {
                    to_hash.push(PendingItem::UpdateContent { id: row.id, path: path.clone(), size, mtime, file_id });
                }
                continue;
            }

            to_hash.push(PendingItem::New { path: path.clone(), size, mtime, file_id });
        }

        let hashed: Vec<(PendingItem, String)> = to_hash
            .into_par_iter()
            .filter_map(|item| {
                let content_hash = dedup::content_hash(item.path()).ok()?;
                Some((item, content_hash))
            })
            .collect();

        let mut conn = db::open(db_path)?;
        let tx = conn.transaction()?;
        for mv in &moves {
            db::update_moved_path(&tx, mv.id, &mv.new_path.to_string_lossy(), mv.size, mv.mtime)?;
        }
        summary.moved += moves.len();

        let imported_at = Utc::now().to_rfc3339();
        for (item, content_hash) in &hashed {
            let write = db::IndexedFileWrite {
                current_path: &item.path().to_string_lossy(),
                content_hash,
                size: item.size(),
                mtime: item.mtime(),
                file_id: item.file_id(),
            };
            match item {
                PendingItem::New { .. } => {
                    db::insert_indexed_file(&tx, &write, &imported_at, batch_id)?;
                    summary.new += 1;
                }
                PendingItem::UpdateContent { id, .. } => {
                    db::update_file_content(&tx, *id, &write)?;
                    summary.content_changed += 1;
                }
            }
        }
        tx.commit()?;

        on_item(walked);
    }

    let missing_ids: Vec<i64> =
        by_path.values().filter(|row| !seen_ids.contains(&row.id)).map(|row| row.id).collect();
    if !missing_ids.is_empty() {
        let mut conn = db::open(db_path)?;
        let tx = conn.transaction()?;
        db::delete_files(&tx, &missing_ids)?;
        tx.commit()?;
    }
    summary.removed = missing_ids.len();

    Ok(summary)
}

fn resolve_scope_root(options: &IndexOptions) -> Result<PathBuf> {
    match options.scope_root {
        None => Ok(options.destination_root.to_path_buf()),
        Some(scope) => {
            if !scope.starts_with(options.destination_root) {
                bail!(
                    "scope root {} is not inside destination root {}",
                    scope.display(),
                    options.destination_root.display()
                );
            }
            Ok(scope.to_path_buf())
        }
    }
}

enum PendingItem {
    New { path: PathBuf, size: Option<i64>, mtime: Option<i64>, file_id: Option<String> },
    UpdateContent { id: i64, path: PathBuf, size: Option<i64>, mtime: Option<i64>, file_id: Option<String> },
}

impl PendingItem {
    fn path(&self) -> &Path {
        match self {
            PendingItem::New { path, .. } | PendingItem::UpdateContent { path, .. } => path,
        }
    }
    fn size(&self) -> Option<i64> {
        match self {
            PendingItem::New { size, .. } | PendingItem::UpdateContent { size, .. } => *size,
        }
    }
    fn mtime(&self) -> Option<i64> {
        match self {
            PendingItem::New { mtime, .. } | PendingItem::UpdateContent { mtime, .. } => *mtime,
        }
    }
    fn file_id(&self) -> Option<&str> {
        match self {
            PendingItem::New { file_id, .. } | PendingItem::UpdateContent { file_id, .. } => file_id.as_deref(),
        }
    }
}

struct PendingMove {
    id: i64,
    new_path: PathBuf,
    size: Option<i64>,
    mtime: Option<i64>,
}

/// Stable OS-level file identity, used to recognize "this content just
/// moved" without rehashing. Always `Option` and only ever trusted by the
/// caller alongside a matching `size` — on a filesystem where this isn't
/// reliable (exFAT, network shares, cloud-sync folders that rewrite files
/// on sync), a mismatch just falls through to a rehash. Wrong here can only
/// cost extra hashing, never a wrong dedup result.
#[cfg(unix)]
fn file_identity(_path: &Path, meta: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn file_identity(path: &Path, _meta: &fs::Metadata) -> Option<String> {
    // NB: file_index()/volume_serial_number() reportedly need a live file
    // handle to populate reliably, not a bare fs::metadata() stat — hence
    // re-opening here rather than reusing the caller's `meta`. Needs
    // verification on real Windows hardware; harmless either way since a
    // `None` here just means "no fast-path data, rehash it."
    use std::os::windows::fs::MetadataExt;
    let file = fs::File::open(path).ok()?;
    let handle_meta = file.metadata().ok()?;
    Some(format!("{}:{}", handle_meta.volume_serial_number()?, handle_meta.file_index()?))
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_path: &Path, _meta: &fs::Metadata) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn db_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("library.sqlite")
    }

    fn all_rows(db_path: &Path) -> Vec<db::IndexedFileMeta> {
        let conn = db::open(db_path).unwrap();
        db::all_indexed_files(&conn).unwrap()
    }

    #[test]
    fn indexes_new_files_into_the_database() {
        let db_dir = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        fs::write(library.path().join("a.jpg"), b"fixture a").unwrap();
        fs::write(library.path().join("b.jpg"), b"fixture b").unwrap();

        let summary = index_library(
            &db_path(&db_dir),
            &IndexOptions { destination_root: library.path(), scope_root: None },
        )
        .unwrap();

        assert_eq!(summary.new, 2);
        assert_eq!(summary.unchanged, 0);
        assert_eq!(all_rows(&db_path(&db_dir)).len(), 2);
    }

    #[test]
    fn rescanning_unchanged_files_does_not_touch_their_rows() {
        let db_dir = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        fs::write(library.path().join("a.jpg"), b"fixture a").unwrap();

        let options = IndexOptions { destination_root: library.path(), scope_root: None };
        index_library(&db_path(&db_dir), &options).unwrap();
        let first_pass_hash = all_rows(&db_path(&db_dir))[0].content_hash.clone();

        let summary = index_library(&db_path(&db_dir), &options).unwrap();

        assert_eq!(summary.unchanged, 1);
        assert_eq!(summary.new, 0);
        assert_eq!(summary.content_changed, 0);
        assert_eq!(all_rows(&db_path(&db_dir))[0].content_hash, first_pass_hash);
    }

    #[test]
    fn detects_a_moved_file_via_file_id_without_rehashing() {
        let db_dir = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        fs::create_dir_all(library.path().join("misc")).unwrap();
        let original = library.path().join("misc/a.jpg");
        fs::write(&original, b"fixture a").unwrap();

        let options = IndexOptions { destination_root: library.path(), scope_root: None };
        index_library(&db_path(&db_dir), &options).unwrap();
        let original_hash = all_rows(&db_path(&db_dir))[0].content_hash.clone();

        fs::create_dir_all(library.path().join("2023/2023-08-15")).unwrap();
        let moved = library.path().join("2023/2023-08-15/a.jpg");
        fs::rename(&original, &moved).unwrap();

        let summary = index_library(&db_path(&db_dir), &options).unwrap();

        assert_eq!(summary.moved, 1, "a same-filesystem rename should be recognized as a move");
        assert_eq!(summary.new, 0);
        let rows = all_rows(&db_path(&db_dir));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].current_path, moved.to_string_lossy());
        assert_eq!(rows[0].content_hash, original_hash, "a move must never trigger a rehash");
    }

    #[test]
    fn detects_content_changed_at_the_same_path() {
        let db_dir = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        let path = library.path().join("a.jpg");
        fs::write(&path, b"original bytes").unwrap();

        let options = IndexOptions { destination_root: library.path(), scope_root: None };
        index_library(&db_path(&db_dir), &options).unwrap();

        // Ensure a distinguishable mtime — some filesystems have 1s mtime
        // resolution.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::write(&path, b"totally different bytes now").unwrap();

        let summary = index_library(&db_path(&db_dir), &options).unwrap();

        assert_eq!(summary.content_changed, 1);
        assert_eq!(summary.new, 0);
        assert_eq!(summary.moved, 0);
    }

    #[test]
    fn removes_rows_for_files_no_longer_present() {
        let db_dir = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        let path = library.path().join("a.jpg");
        fs::write(&path, b"fixture a").unwrap();

        let options = IndexOptions { destination_root: library.path(), scope_root: None };
        index_library(&db_path(&db_dir), &options).unwrap();

        fs::remove_file(&path).unwrap();
        let summary = index_library(&db_path(&db_dir), &options).unwrap();

        assert_eq!(summary.removed, 1);
        assert!(all_rows(&db_path(&db_dir)).is_empty());
    }

    #[test]
    fn scoped_reindex_only_touches_files_under_the_scope() {
        let db_dir = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        fs::create_dir_all(library.path().join("2025")).unwrap();
        fs::create_dir_all(library.path().join("2026")).unwrap();
        fs::write(library.path().join("2025/old.jpg"), b"old fixture").unwrap();
        fs::write(library.path().join("2026/new.jpg"), b"new fixture").unwrap();

        // Full index first so both rows exist.
        index_library(
            &db_path(&db_dir),
            &IndexOptions { destination_root: library.path(), scope_root: None },
        )
        .unwrap();

        // Remove the 2025 file, then reindex *only* the 2026 scope.
        fs::remove_file(library.path().join("2025/old.jpg")).unwrap();
        let summary = index_library(
            &db_path(&db_dir),
            &IndexOptions { destination_root: library.path(), scope_root: Some(&library.path().join("2026")) },
        )
        .unwrap();

        assert_eq!(summary.removed, 0, "scoped reindex must not remove rows outside its scope");
        let rows = all_rows(&db_path(&db_dir));
        assert_eq!(rows.len(), 2, "the 2025 row should still exist, untouched by an out-of-scope walk");
    }

    #[test]
    fn rejects_a_scope_root_outside_the_destination_root() {
        let db_dir = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();

        let result = index_library(
            &db_path(&db_dir),
            &IndexOptions { destination_root: library.path(), scope_root: Some(elsewhere.path()) },
        );

        assert!(result.is_err());
    }
}

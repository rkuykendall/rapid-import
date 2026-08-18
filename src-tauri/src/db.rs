use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

/// Schema from execution-plan.md §5.
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS files (
    id              INTEGER PRIMARY KEY,
    content_hash    TEXT NOT NULL,
    current_path    TEXT NOT NULL UNIQUE,
    capture_date    TEXT,
    date_source     TEXT,
    date_confidence REAL,
    imported_at     TEXT NOT NULL,
    batch_id        INTEGER NOT NULL,
    size            INTEGER,
    mtime           INTEGER,
    file_id         TEXT,
    FOREIGN KEY (batch_id) REFERENCES batches(id)
);
CREATE INDEX IF NOT EXISTS idx_files_content_hash ON files(content_hash);

CREATE TABLE IF NOT EXISTS batches (
    id            INTEGER PRIMARY KEY,
    started_at    TEXT NOT NULL,
    profile_id    INTEGER,
    kind          TEXT NOT NULL,
    undo_log_path TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS profiles (
    id                  INTEGER PRIMARY KEY,
    name                TEXT NOT NULL,
    folder_template     TEXT NOT NULL,
    source_root         TEXT,
    destination_root    TEXT,
    date_fallback_order TEXT NOT NULL,
    conflict_policy     TEXT NOT NULL
);
"#;

/// Opens (creating if needed) the index file at `path` and ensures the
/// schema exists. `path` is expected to live under the app's data dir
/// (`tauri::path`), never inside a user's photo directories.
///
/// WAL mode lets a long-running `index_library` write transaction coexist
/// with interactive readers (`scan_source`, `list_profiles`, ...) without
/// blocking them — harmless for the in-memory/test connections too, since
/// `pragma_update` on those is a no-op-equivalent single-writer case.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    init_schema(&conn)?;
    Ok(conn)
}

/// In-memory connection, primarily for tests and the pre-Tauri CLI harness.
pub fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA_SQL)?;
    migrate_files_table(conn)
}

/// Additive/removal migration for `library.sqlite` files predating a schema
/// change — `CREATE TABLE IF NOT EXISTS` is a no-op against an
/// already-existing table, so column changes need to be applied by hand.
/// Safe to run idempotently on every `open`/`open_in_memory` call rather
/// than needing a real migration/versioning framework, since every step
/// here is guarded by `PRAGMA table_info`.
fn migrate_files_table(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(files)")?;
    let existing: std::collections::HashSet<String> =
        stmt.query_map([], |row| row.get::<_, String>(1))?.collect::<rusqlite::Result<_>>()?;
    drop(stmt);

    // Added for the destination-library indexer: `size`/`mtime`/`file_id`.
    // All three are nullable (an old row just has no fast-path data, which
    // `index_library` treats as "rehash it").
    for (column, ddl) in [
        ("size", "ALTER TABLE files ADD COLUMN size INTEGER"),
        ("mtime", "ALTER TABLE files ADD COLUMN mtime INTEGER"),
        ("file_id", "ALTER TABLE files ADD COLUMN file_id TEXT"),
    ] {
        if !existing.contains(column) {
            conn.execute(ddl, [])?;
        }
    }
    conn.execute("CREATE INDEX IF NOT EXISTS idx_files_file_id ON files(file_id)", [])?;

    // Removed along with near-duplicate detection — dropped explicitly
    // (index first, then column) on any DB that still has it, rather than
    // left behind as dead weight.
    if existing.contains("perceptual_hash") {
        conn.execute("DROP INDEX IF EXISTS idx_files_perceptual_hash", [])?;
        conn.execute("ALTER TABLE files DROP COLUMN perceptual_hash", [])?;
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct NewBatch {
    pub started_at: String,
    pub profile_id: Option<i64>,
    pub kind: String,
    pub undo_log_path: String,
}

pub fn insert_batch(conn: &Connection, batch: &NewBatch) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO batches (started_at, profile_id, kind, undo_log_path) VALUES (?1, ?2, ?3, ?4)",
        params![batch.started_at, batch.profile_id, batch.kind, batch.undo_log_path],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn undo_log_path_for_batch(conn: &Connection, batch_id: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT undo_log_path FROM batches WHERE id = ?1",
        params![batch_id],
        |row| row.get(0),
    )
    .optional()
}

#[derive(Debug, Clone)]
pub struct NewFileRecord {
    pub content_hash: String,
    pub current_path: String,
    pub capture_date: Option<String>,
    pub date_source: Option<String>,
    pub date_confidence: Option<f64>,
    pub imported_at: String,
    pub batch_id: i64,
}

pub fn insert_file(conn: &Connection, file: &NewFileRecord) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO files
            (content_hash, current_path, capture_date, date_source, date_confidence, imported_at, batch_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            file.content_hash,
            file.current_path,
            file.capture_date,
            file.date_source,
            file.date_confidence,
            file.imported_at,
            file.batch_id,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Looks up an already-indexed file by content hash — the basis for
/// recognizing files a previous import already handled (e.g. re-scanning
/// the same SD card) and for exact-duplicate detection.
pub fn find_by_content_hash(conn: &Connection, content_hash: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT current_path FROM files WHERE content_hash = ?1 LIMIT 1",
        params![content_hash],
        |row| row.get(0),
    )
    .optional()
}

#[derive(Debug, Clone)]
pub struct IndexedFileHash {
    pub current_path: String,
    pub content_hash: String,
}

/// Bulk fetch of every indexed file's hashes, for `dedup::find_duplicates`
/// to cross-reference a scan against — a single query beats one round-trip
/// per scanned file.
pub fn all_file_hashes(conn: &Connection) -> rusqlite::Result<Vec<IndexedFileHash>> {
    let mut stmt = conn.prepare("SELECT current_path, content_hash FROM files")?;
    let rows = stmt.query_map([], |row| {
        Ok(IndexedFileHash { current_path: row.get(0)?, content_hash: row.get(1)? })
    })?;
    rows.collect()
}

#[derive(Debug, Clone)]
pub struct IndexedFileMeta {
    pub id: i64,
    pub current_path: String,
    pub content_hash: String,
    pub size: Option<i64>,
    pub mtime: Option<i64>,
    pub file_id: Option<String>,
}

/// Whole-table load, same precedent as `all_file_hashes` — `index_library`
/// diffs a destination walk against this in memory rather than doing one
/// query per walked file, the same tradeoff `dedup::find_duplicates` already
/// makes at this same scale.
pub fn all_indexed_files(conn: &Connection) -> rusqlite::Result<Vec<IndexedFileMeta>> {
    let mut stmt = conn.prepare("SELECT id, current_path, content_hash, size, mtime, file_id FROM files")?;
    let rows = stmt.query_map([], |row| {
        Ok(IndexedFileMeta {
            id: row.get(0)?,
            current_path: row.get(1)?,
            content_hash: row.get(2)?,
            size: row.get(3)?,
            mtime: row.get(4)?,
            file_id: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// Single-row lookup by `current_path` (which is `UNIQUE`, so this is a
/// plain indexed lookup) — the fast-path check `scan.rs` uses to recognize
/// a source file that's unchanged since it was last indexed at this exact
/// path (same size+mtime), so it can reuse the stored hash instead of
/// rehashing. Only useful when the source path can actually coincide with
/// an indexed `current_path` — i.e. reorganize-in-place, where source and
/// destination are the same tree; a plain import from a different source
/// (an SD card) will never match anything here, which is correct, since
/// that content genuinely hasn't been hashed yet.
pub fn find_by_path(conn: &Connection, path: &str) -> rusqlite::Result<Option<IndexedFileMeta>> {
    conn.query_row(
        "SELECT id, current_path, content_hash, size, mtime, file_id FROM files WHERE current_path = ?1",
        params![path],
        |row| {
            Ok(IndexedFileMeta {
                id: row.get(0)?,
                current_path: row.get(1)?,
                content_hash: row.get(2)?,
                size: row.get(3)?,
                mtime: row.get(4)?,
                file_id: row.get(5)?,
            })
        },
    )
    .optional()
}

#[derive(Debug, Clone)]
pub struct IndexedFileWrite<'a> {
    pub current_path: &'a str,
    pub content_hash: &'a str,
    pub size: Option<i64>,
    pub mtime: Option<i64>,
    pub file_id: Option<&'a str>,
}

/// Inserts a newly-discovered file into the index. Uses `ON CONFLICT DO
/// UPDATE` rather than a bare `INSERT` — defensive against `current_path`
/// already being taken by a stale row (e.g. a concurrent import touched the
/// same path between `index_library`'s initial snapshot read and this
/// write), so a same-path race self-heals instead of erroring the whole run.
pub fn insert_indexed_file(
    conn: &Connection,
    file: &IndexedFileWrite,
    imported_at: &str,
    batch_id: i64,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO files
            (content_hash, current_path, imported_at, batch_id, size, mtime, file_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(current_path) DO UPDATE SET
            content_hash = excluded.content_hash,
            imported_at = excluded.imported_at,
            batch_id = excluded.batch_id,
            size = excluded.size,
            mtime = excluded.mtime,
            file_id = excluded.file_id",
        params![file.content_hash, file.current_path, imported_at, batch_id, file.size, file.mtime, file.file_id],
    )?;
    conn.query_row(
        "SELECT id FROM files WHERE current_path = ?1",
        params![file.current_path],
        |row| row.get(0),
    )
}

/// Path unchanged, content changed (size/mtime mismatch on rescan) — rehash
/// happened, refresh everything but the row's original date-resolution
/// fields, which a plain reindex has no opinion on.
pub fn update_file_content(conn: &Connection, id: i64, file: &IndexedFileWrite) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE files SET
            content_hash = ?1,
            size = ?2,
            mtime = ?3,
            file_id = ?4
         WHERE id = ?5",
        params![file.content_hash, file.size, file.mtime, file.file_id, id],
    )?;
    Ok(())
}

/// Content unchanged, only the path (and observed size/mtime) moved —
/// detected via matching `file_id`, so no rehash is needed.
pub fn update_moved_path(
    conn: &Connection,
    id: i64,
    new_path: &str,
    size: Option<i64>,
    mtime: Option<i64>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE files SET current_path = ?1, size = ?2, mtime = ?3 WHERE id = ?4",
        params![new_path, size, mtime, id],
    )?;
    Ok(())
}

/// Removes rows for content that's no longer found anywhere in a reindexed
/// scope — see `index_library`'s doc comment for why this deletes rather
/// than tombstones.
pub fn delete_files(conn: &Connection, ids: &[i64]) -> rusqlite::Result<()> {
    for id in ids {
        conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_all_three_tables() {
        let conn = open_in_memory().unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(names, vec!["batches", "files", "profiles"]);
    }

    #[test]
    fn opening_twice_is_idempotent() {
        let conn = open_in_memory().unwrap();
        init_schema(&conn).unwrap();
    }

    #[test]
    fn insert_and_find_file_by_content_hash() {
        let conn = open_in_memory().unwrap();
        let batch_id = insert_batch(
            &conn,
            &NewBatch {
                started_at: "2026-08-16T00:00:00".to_string(),
                profile_id: None,
                kind: "import".to_string(),
                undo_log_path: "/tmp/undo.json".to_string(),
            },
        )
        .unwrap();

        insert_file(
            &conn,
            &NewFileRecord {
                content_hash: "abc123".to_string(),
                current_path: "/library/2023/2023-08-15/IMG_0001.jpg".to_string(),
                capture_date: Some("2023-08-15T14:15:23".to_string()),
                date_source: Some("exif".to_string()),
                date_confidence: Some(0.95),
                imported_at: "2026-08-16T00:00:01".to_string(),
                batch_id,
            },
        )
        .unwrap();

        let found = find_by_content_hash(&conn, "abc123").unwrap();
        assert_eq!(found.as_deref(), Some("/library/2023/2023-08-15/IMG_0001.jpg"));

        assert!(find_by_content_hash(&conn, "not-present").unwrap().is_none());
    }

    #[test]
    fn all_file_hashes_returns_every_row() {
        let conn = open_in_memory().unwrap();
        let batch_id = insert_batch(
            &conn,
            &NewBatch {
                started_at: "2026-08-16T00:00:00".to_string(),
                profile_id: None,
                kind: "import".to_string(),
                undo_log_path: "/tmp/undo.json".to_string(),
            },
        )
        .unwrap();

        insert_file(
            &conn,
            &NewFileRecord {
                content_hash: "abc123".to_string(),
                current_path: "/library/a.jpg".to_string(),
                capture_date: None,
                date_source: None,
                date_confidence: None,
                imported_at: "2026-08-16T00:00:01".to_string(),
                batch_id,
            },
        )
        .unwrap();
        insert_file(
            &conn,
            &NewFileRecord {
                content_hash: "def456".to_string(),
                current_path: "/library/b.jpg".to_string(),
                capture_date: None,
                date_source: None,
                date_confidence: None,
                imported_at: "2026-08-16T00:00:01".to_string(),
                batch_id,
            },
        )
        .unwrap();

        let hashes = all_file_hashes(&conn).unwrap();
        assert_eq!(hashes.len(), 2);
        assert!(hashes.iter().any(|h| h.current_path == "/library/a.jpg" && h.content_hash == "abc123"));
        assert!(hashes.iter().any(|h| h.current_path == "/library/b.jpg" && h.content_hash == "def456"));
    }

    #[test]
    fn undo_log_path_for_batch_round_trips() {
        let conn = open_in_memory().unwrap();
        let batch_id = insert_batch(
            &conn,
            &NewBatch {
                started_at: "2026-08-16T00:00:00".to_string(),
                profile_id: None,
                kind: "import".to_string(),
                undo_log_path: "/tmp/undo-42.json".to_string(),
            },
        )
        .unwrap();

        assert_eq!(
            undo_log_path_for_batch(&conn, batch_id).unwrap().as_deref(),
            Some("/tmp/undo-42.json")
        );
        assert!(undo_log_path_for_batch(&conn, 9999).unwrap().is_none());
    }

    fn index_batch(conn: &Connection) -> i64 {
        insert_batch(
            conn,
            &NewBatch {
                started_at: "2026-08-16T00:00:00".to_string(),
                profile_id: None,
                kind: "index".to_string(),
                undo_log_path: String::new(),
            },
        )
        .unwrap()
    }

    #[test]
    fn migrating_an_older_db_adds_the_new_columns() {
        let conn = Connection::open_in_memory().unwrap();
        // Simulates a `library.sqlite` created before size/mtime/file_id
        // existed — the original schema, minus those three columns.
        conn.execute_batch(
            "CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                content_hash TEXT NOT NULL,
                perceptual_hash TEXT,
                current_path TEXT NOT NULL UNIQUE,
                capture_date TEXT,
                date_source TEXT,
                date_confidence REAL,
                imported_at TEXT NOT NULL,
                batch_id INTEGER NOT NULL
            );",
        )
        .unwrap();

        migrate_files_table(&conn).unwrap();
        migrate_files_table(&conn).unwrap(); // idempotent

        let mut stmt = conn.prepare("PRAGMA table_info(files)").unwrap();
        let columns: std::collections::HashSet<String> =
            stmt.query_map([], |row| row.get::<_, String>(1)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert!(columns.contains("size"));
        assert!(columns.contains("mtime"));
        assert!(columns.contains("file_id"));
        assert!(!columns.contains("perceptual_hash"), "removed along with near-duplicate detection");
    }

    #[test]
    fn opening_a_real_pre_migration_db_file_does_not_panic() {
        // Regression test for a bug the above test missed by calling
        // migrate_files_table directly: SCHEMA_SQL used to include
        // `CREATE INDEX ... ON files(file_id)` itself, which ran via
        // execute_batch *before* migrate_files_table had a chance to add
        // the column to a pre-existing on-disk table — crashing `db::open`
        // on any real `library.sqlite` written before this migration
        // existed. This goes through the actual `open` entry point instead.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE files (
                    id INTEGER PRIMARY KEY,
                    content_hash TEXT NOT NULL,
                    perceptual_hash TEXT,
                    current_path TEXT NOT NULL UNIQUE,
                    capture_date TEXT,
                    date_source TEXT,
                    date_confidence REAL,
                    imported_at TEXT NOT NULL,
                    batch_id INTEGER NOT NULL
                );
                CREATE TABLE batches (
                    id            INTEGER PRIMARY KEY,
                    started_at    TEXT NOT NULL,
                    profile_id    INTEGER,
                    kind          TEXT NOT NULL,
                    undo_log_path TEXT NOT NULL
                );",
            )
            .unwrap();
        }

        let conn = open(&path).unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(files)").unwrap();
        let columns: std::collections::HashSet<String> =
            stmt.query_map([], |row| row.get::<_, String>(1)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert!(columns.contains("file_id"));
        assert!(!columns.contains("perceptual_hash"));
    }

    #[test]
    fn insert_indexed_file_round_trips_and_returns_its_row() {
        let conn = open_in_memory().unwrap();
        let batch_id = index_batch(&conn);

        let id = insert_indexed_file(
            &conn,
            &IndexedFileWrite {
                current_path: "/library/2023/2023-08-15/a.jpg",
                content_hash: "abc123",
                size: Some(1024),
                mtime: Some(1_700_000_000),
                file_id: Some("1:42"),
            },
            "2026-08-16T00:00:01",
            batch_id,
        )
        .unwrap();

        let all = all_indexed_files(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].content_hash, "abc123");
        assert_eq!(all[0].size, Some(1024));
        assert_eq!(all[0].file_id.as_deref(), Some("1:42"));

        let found = find_by_path(&conn, "/library/2023/2023-08-15/a.jpg").unwrap().unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.content_hash, "abc123");
        assert!(find_by_path(&conn, "/library/nope.jpg").unwrap().is_none());
    }

    #[test]
    fn insert_indexed_file_upserts_on_conflicting_path() {
        let conn = open_in_memory().unwrap();
        let batch_id = index_batch(&conn);
        let write = IndexedFileWrite {
            current_path: "/library/a.jpg",
            content_hash: "first-hash",
            size: Some(10),
            mtime: Some(100),
            file_id: None,
        };
        insert_indexed_file(&conn, &write, "2026-08-16T00:00:01", batch_id).unwrap();

        let write2 = IndexedFileWrite { content_hash: "second-hash", size: Some(20), mtime: Some(200), ..write };
        insert_indexed_file(&conn, &write2, "2026-08-16T00:00:02", batch_id).unwrap();

        let all = all_indexed_files(&conn).unwrap();
        assert_eq!(all.len(), 1, "same path should upsert, not duplicate");
        assert_eq!(all[0].content_hash, "second-hash");
        assert_eq!(all[0].size, Some(20));
    }

    #[test]
    fn update_file_content_rehashes_in_place() {
        let conn = open_in_memory().unwrap();
        let batch_id = index_batch(&conn);
        let id = insert_indexed_file(
            &conn,
            &IndexedFileWrite {
                current_path: "/library/a.jpg",
                content_hash: "old-hash",
                size: Some(10),
                mtime: Some(100),
                file_id: Some("1:1"),
            },
            "2026-08-16T00:00:01",
            batch_id,
        )
        .unwrap();

        update_file_content(
            &conn,
            id,
            &IndexedFileWrite {
                current_path: "/library/a.jpg",
                content_hash: "new-hash",
                size: Some(20),
                mtime: Some(200),
                file_id: Some("1:1"),
            },
        )
        .unwrap();

        let all = all_indexed_files(&conn).unwrap();
        assert_eq!(all[0].content_hash, "new-hash");
        assert_eq!(all[0].current_path, "/library/a.jpg", "content update must not touch the path");
        assert_eq!(all[0].size, Some(20));
    }

    #[test]
    fn update_moved_path_leaves_content_hash_untouched() {
        let conn = open_in_memory().unwrap();
        let batch_id = index_batch(&conn);
        let id = insert_indexed_file(
            &conn,
            &IndexedFileWrite {
                current_path: "/library/misc/a.jpg",
                content_hash: "stable-hash",
                size: Some(10),
                mtime: Some(100),
                file_id: Some("1:7"),
            },
            "2026-08-16T00:00:01",
            batch_id,
        )
        .unwrap();

        update_moved_path(&conn, id, "/library/2023/2023-08-15/a.jpg", Some(10), Some(150)).unwrap();

        let all = all_indexed_files(&conn).unwrap();
        assert_eq!(all[0].current_path, "/library/2023/2023-08-15/a.jpg");
        assert_eq!(all[0].content_hash, "stable-hash", "a move must never touch the content hash");
        assert_eq!(all[0].mtime, Some(150));
    }

    #[test]
    fn delete_files_removes_only_the_given_rows() {
        let conn = open_in_memory().unwrap();
        let batch_id = index_batch(&conn);
        let keep = insert_indexed_file(
            &conn,
            &IndexedFileWrite {
                current_path: "/library/keep.jpg",
                content_hash: "keep-hash",
                size: None,
                mtime: None,
                file_id: None,
            },
            "2026-08-16T00:00:01",
            batch_id,
        )
        .unwrap();
        let gone = insert_indexed_file(
            &conn,
            &IndexedFileWrite {
                current_path: "/library/gone.jpg",
                content_hash: "gone-hash",
                size: None,
                mtime: None,
                file_id: None,
            },
            "2026-08-16T00:00:01",
            batch_id,
        )
        .unwrap();

        delete_files(&conn, &[gone]).unwrap();

        let all = all_indexed_files(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, keep);
    }
}

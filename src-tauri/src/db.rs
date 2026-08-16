use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

/// Schema from execution-plan.md §5.
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS files (
    id              INTEGER PRIMARY KEY,
    content_hash    TEXT NOT NULL,
    perceptual_hash TEXT,
    current_path    TEXT NOT NULL UNIQUE,
    capture_date    TEXT,
    date_source     TEXT,
    date_confidence REAL,
    imported_at     TEXT NOT NULL,
    batch_id        INTEGER NOT NULL,
    FOREIGN KEY (batch_id) REFERENCES batches(id)
);
CREATE INDEX IF NOT EXISTS idx_files_content_hash ON files(content_hash);
CREATE INDEX IF NOT EXISTS idx_files_perceptual_hash ON files(perceptual_hash);

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
    filename_template   TEXT,
    date_fallback_order TEXT NOT NULL,
    conflict_policy     TEXT NOT NULL
);
"#;

/// Opens (creating if needed) the index file at `path` and ensures the
/// schema exists. `path` is expected to live under the app's data dir
/// (`tauri::path`), never inside a user's photo directories.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
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
    conn.execute_batch(SCHEMA_SQL)
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
    pub perceptual_hash: Option<String>,
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
            (content_hash, perceptual_hash, current_path, capture_date, date_source, date_confidence, imported_at, batch_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            file.content_hash,
            file.perceptual_hash,
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
                perceptual_hash: None,
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
}

use std::path::Path;

use rusqlite::Connection;

/// Schema from execution-plan.md §5. `files`/`batches` stay unpopulated
/// until the commit engine (phase 5) and dedup (phase 7) land — this phase
/// only wires up the schema itself plus profile persistence.
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
}

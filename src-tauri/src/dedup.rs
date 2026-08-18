use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::db;
use crate::plan::PlanItem;

/// BLAKE3 content hash, hex-encoded — the basis for exact-duplicate
/// detection and for recognizing files already present in the SQLite
/// index (e.g. re-scanning the same SD card after a prior import).
pub fn content_hash(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// A file's mtime as Unix seconds — the cheap (no byte read) staleness
/// signal `scan.rs` and `index_library.rs` both compare against an indexed
/// row's stored `mtime` to decide whether a file needs (re)hashing at all.
/// Shared here rather than duplicated in each so the two can't drift apart.
pub fn mtime_secs(meta: &fs::Metadata) -> Option<i64> {
    meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DuplicateGroup {
    pub members: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
struct HashRef<'a> {
    path: &'a Path,
    content_hash: &'a str,
}

fn compare(a: HashRef, b: HashRef) -> Option<DuplicateGroup> {
    (a.content_hash == b.content_hash)
        .then(|| DuplicateGroup { members: vec![a.path.to_path_buf(), b.path.to_path_buf()] })
}

/// Cross-references every scanned file's content hash against both the
/// other files in this same scan and the SQLite index, per §7
/// (`get_duplicates`: "against the SQLite index and against other files in
/// the current scan"). Each result is one pairing (2 members), not a fully
/// clustered group — a file that matches three others surfaces as three
/// separate pairs.
///
/// Reuses `item.content_hash` — already computed by `scan::build_plan_item`
/// (and skipped there entirely for a file unchanged since it was last
/// indexed) — rather than reading and hashing every scanned file's bytes a
/// second time here. An item with no `content_hash`
/// (index was `None` at scan time) is silently excluded, same as before:
/// with nothing indexed there was never anything to cross-reference it
/// against anyway.
pub fn find_duplicates(conn: &rusqlite::Connection, items: &[PlanItem]) -> Vec<DuplicateGroup> {
    let hashed: Vec<&PlanItem> = items.iter().filter(|item| item.content_hash.is_some()).collect();

    let indexed = db::all_file_hashes(conn).unwrap_or_default();

    let mut groups = Vec::new();
    for i in 0..hashed.len() {
        let a = HashRef { path: &hashed[i].source_path, content_hash: hashed[i].content_hash.as_deref().unwrap() };

        for other in &hashed[i + 1..] {
            let b = HashRef { path: &other.source_path, content_hash: other.content_hash.as_deref().unwrap() };
            if let Some(group) = compare(a, b) {
                groups.push(group);
            }
        }

        for existing in &indexed {
            if existing.current_path == hashed[i].source_path.to_string_lossy() {
                continue;
            }
            let b = HashRef { path: Path::new(&existing.current_path), content_hash: &existing.content_hash };
            if let Some(group) = compare(a, b) {
                groups.push(group);
            }
        }
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_bytes_produce_identical_hash() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.jpg");
        fs::write(&a, b"same bytes").unwrap();
        fs::write(&b, b"same bytes").unwrap();

        assert_eq!(content_hash(&a).unwrap(), content_hash(&b).unwrap());
    }

    #[test]
    fn different_bytes_produce_different_hash() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.jpg");
        fs::write(&a, b"one").unwrap();
        fs::write(&b, b"two").unwrap();

        assert_ne!(content_hash(&a).unwrap(), content_hash(&b).unwrap());
    }

    // `find_duplicates` no longer hashes anything itself — it reads
    // `content_hash` straight off the `PlanItem`, the way
    // `scan::build_plan_item` actually populates it. So this test helper
    // hashes the (already-written-to-disk) fixture file itself, standing in
    // for what a real scan would have already computed.
    fn plan_item(source_path: PathBuf) -> PlanItem {
        let hash = content_hash(&source_path).unwrap();
        PlanItem {
            source_path,
            candidates: vec![],
            destination_path: None,
            needs_review: true,
            conflict: crate::plan::ConflictKind::None,
            no_op: false,
            already_imported: false,
            excluded: false,
            content_hash: Some(hash),
        }
    }

    #[test]
    fn flags_exact_matches_within_the_same_scan() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.jpg");
        fs::write(&a, b"identical bytes").unwrap();
        fs::write(&b, b"identical bytes").unwrap();

        let conn = crate::db::open_in_memory().unwrap();
        let items = vec![plan_item(a.clone()), plan_item(b.clone())];
        let groups = find_duplicates(&conn, &items);

        assert_eq!(groups.len(), 1);
        assert!(groups[0].members.contains(&a) && groups[0].members.contains(&b));
    }

    #[test]
    fn flags_exact_matches_against_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let scanned = dir.path().join("scanned.jpg");
        fs::write(&scanned, b"already imported bytes").unwrap();

        let conn = crate::db::open_in_memory().unwrap();
        let batch_id = crate::db::insert_batch(
            &conn,
            &crate::db::NewBatch {
                started_at: "2026-08-01T00:00:00".to_string(),
                profile_id: None,
                kind: "import".to_string(),
                undo_log_path: "/tmp/undo.json".to_string(),
            },
        )
        .unwrap();
        crate::db::insert_file(
            &conn,
            &crate::db::NewFileRecord {
                content_hash: content_hash(&scanned).unwrap(),
                current_path: "/library/2023/2023-08-15/existing.jpg".to_string(),
                capture_date: None,
                date_source: None,
                date_confidence: None,
                imported_at: "2026-08-01T00:00:01".to_string(),
                batch_id,
            },
        )
        .unwrap();

        let items = vec![plan_item(scanned.clone())];
        let groups = find_duplicates(&conn, &items);

        assert_eq!(groups.len(), 1);
        assert!(groups[0].members.contains(&scanned));
        assert!(groups[0].members.contains(&PathBuf::from("/library/2023/2023-08-15/existing.jpg")));
    }

    #[test]
    fn no_groups_for_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.jpg");
        fs::write(&a, b"completely unrelated content one").unwrap();
        fs::write(&b, b"totally different stuff over here").unwrap();

        let conn = crate::db::open_in_memory().unwrap();
        let items = vec![plan_item(a), plan_item(b)];
        let groups = find_duplicates(&conn, &items);

        assert!(groups.is_empty());
    }
}

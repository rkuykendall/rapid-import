use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use rusqlite::Connection;
use walkdir::WalkDir;

use crate::date_resolution::{self, DateInputs};
use crate::db;
use crate::dedup;
use crate::formats::is_supported_media_file;
use crate::plan::{render_template, ConflictKind, Plan, PlanItem, NEEDS_REVIEW_THRESHOLD};
use crate::sidecar_interop;

pub struct ScanOptions<'a> {
    pub source_root: &'a Path,
    pub destination_root: &'a Path,
    pub folder_template: &'a str,
    pub now: NaiveDate,
    /// Optional index to cross-reference by content hash — flags items
    /// whose bytes already appear in a previous import (e.g. re-scanning
    /// the same SD card). `None` skips the check (and the hashing cost).
    pub index: Option<&'a Connection>,
}

/// Recursively walks `source_root` (nested/already-organized trees included
/// — reorganize-in-place needs the walk to not assume a flat layout) and
/// computes a dry-run plan. No files are read except for metadata/bytes
/// needed to resolve a date; nothing on disk is written.
pub fn scan(options: &ScanOptions) -> Plan {
    scan_with_progress(options, |_| {})
}

/// Same as `scan`, but calls `on_item` with the running count after each
/// file is processed — a UI can throttle these into a live "N files
/// scanned so far" display. `scan` itself is just this with a no-op
/// callback, so `scan_cli`/existing tests are unaffected.
pub fn scan_with_progress(options: &ScanOptions, mut on_item: impl FnMut(usize)) -> Plan {
    let mut items: Vec<PlanItem> = Vec::new();
    for entry in WalkDir::new(options.source_root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| is_supported_media_file(entry.path()))
    {
        items.push(build_plan_item(entry.path(), options));
        on_item(items.len());
    }

    align_sidecars_with_primary(&mut items);
    flag_conflicts(&mut items);

    Plan { items }
}

/// Sidecars (`.xmp`, `.rrdata`, `.rrexif`, a paired RAW+JPEG, ...) are
/// resolved above using their *own* metadata — usually just their own
/// filesystem timestamp, since a sidecar rarely carries EXIF/XMP of its
/// own. That produces a technically-real but irrelevant low-confidence date
/// for a file that never actually moves on its own: `commit_plan` always
/// relocates a sidecar into its primary's destination folder (see
/// `commit.rs`'s grouping), regardless of what date this scan resolved for
/// it individually. Left uncorrected, the preview plan shows a sidecar with
/// a bogus destination and a false "needs review" flag for a decision that
/// isn't actually its own — exactly the confusion this pass exists to
/// prevent, by making every group's siblings inherit the primary's
/// destination, candidates, and review status before anything is displayed
/// or committed.
fn align_sidecars_with_primary(items: &mut [PlanItem]) {
    for group in crate::plan::group_associated_indices(items) {
        let [primary_index, siblings @ ..] = group.as_slice() else {
            continue;
        };
        if siblings.is_empty() {
            continue;
        }

        let primary = &items[*primary_index];
        let Some(primary_folder) = primary.destination_path.as_deref().and_then(Path::parent) else {
            continue;
        };
        let primary_folder = primary_folder.to_path_buf();
        let primary_candidates = primary.candidates.clone();
        let primary_needs_review = primary.needs_review;

        for &sibling_index in siblings {
            if let Some(filename) = items[sibling_index].source_path.file_name() {
                items[sibling_index].destination_path = Some(primary_folder.join(filename));
            }
            items[sibling_index].candidates = primary_candidates.clone();
            items[sibling_index].needs_review = primary_needs_review;
        }
    }
}

fn build_plan_item(source_path: &Path, options: &ScanOptions) -> PlanItem {
    let filename = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    let inputs = DateInputs {
        filename,
        exif_date_time_original: read_exif_date(source_path),
        xmp_date_time_original: sidecar_interop::read_xmp_date(source_path),
        fs_created: file_time(source_path, |m| m.created()),
        fs_modified: file_time(source_path, |m| m.modified()),
    };

    let resolution = date_resolution::resolve(&inputs, options.now);
    let chosen = resolution.chosen().cloned();

    let needs_review = match &chosen {
        Some(candidate) => candidate.confidence < NEEDS_REVIEW_THRESHOLD,
        None => true,
    };

    let destination_path = chosen.as_ref().map(|candidate| {
        let rendered_folder = render_template(options.folder_template, candidate.date);
        let destination_folder =
            preserve_existing_subfolder(source_path, options.source_root, &rendered_folder)
                .unwrap_or_else(|| PathBuf::from(&rendered_folder));
        options.destination_root.join(destination_folder).join(filename)
    });

    let (content_hash, already_imported) = compute_hash(source_path, options.index);

    PlanItem {
        source_path: source_path.to_path_buf(),
        candidates: resolution.candidates,
        destination_path,
        needs_review,
        conflict: ConflictKind::None,
        no_op: false,
        already_imported,
        excluded: false,
        content_hash,
    }
}

/// Computes this file's content hash and whether it's already present in
/// the index — or, when possible, skips hashing entirely.
///
/// The fast path: if the index already has a row at this *exact* source
/// path with a matching size+mtime, the file is unchanged since it was
/// last indexed/imported, so its stored hash is reused verbatim instead of
/// re-reading the file. This only ever triggers for reorganize-in-place
/// (source_root == destination_root) — a plain import's source path (an SD
/// card) never coincides with any indexed `current_path` — which is
/// exactly the case that used to mean "rehash every file in the library,
/// every time you click Scan."
///
/// `dedup::find_duplicates` reuses whatever this returns rather than
/// hashing a second time itself.
fn compute_hash(source_path: &Path, index: Option<&Connection>) -> (Option<String>, bool) {
    let Some(conn) = index else {
        return (None, false);
    };

    let path_str = source_path.to_string_lossy();
    let stat = fs::metadata(source_path).ok();
    let size = stat.as_ref().map(|m| m.len() as i64);
    let mtime = stat.as_ref().and_then(dedup::mtime_secs);

    if let Ok(Some(indexed)) = db::find_by_path(conn, &path_str)
        && indexed.size == size
        && indexed.mtime == mtime
    {
        return (Some(indexed.content_hash), true);
    }

    let Ok(content_hash) = dedup::content_hash(source_path) else {
        return (None, false);
    };
    let already_imported = matches!(db::find_by_content_hash(conn, &content_hash), Ok(Some(_)));
    (Some(content_hash), already_imported)
}

fn read_exif_date(path: &Path) -> Option<NaiveDateTime> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)?;
    let raw = field.display_value().to_string();
    let cleaned = raw.trim_matches('"').trim();
    NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(cleaned, "%Y:%m:%d %H:%M:%S"))
        .ok()
}

fn file_time(
    path: &Path,
    extract: fn(&fs::Metadata) -> std::io::Result<SystemTime>,
) -> Option<NaiveDateTime> {
    let metadata = fs::metadata(path).ok()?;
    let system_time = extract(&metadata).ok()?;
    let datetime: DateTime<Local> = system_time.into();
    Some(datetime.naive_local())
}

/// If `source_path` already lives at or below the rendered date folder
/// (relative to `source_root`), returns that fuller relative path so any
/// extra subfolders the user added on top of the date structure — an event
/// or trip name, say `2018/02 February/Wedding/` — are preserved rather
/// than flattened away. Returns `None` when the file isn't already nested
/// under a matching date folder at all, so a fresh import still lands at
/// the plain templated folder.
fn preserve_existing_subfolder(source_path: &Path, source_root: &Path, rendered_folder: &str) -> Option<PathBuf> {
    let relative_parent = source_path.strip_prefix(source_root).ok()?.parent()?;
    let rendered_components: Vec<_> = Path::new(rendered_folder).components().collect();
    let relative_components: Vec<_> = relative_parent.components().collect();

    relative_components
        .starts_with(rendered_components.as_slice())
        .then(|| relative_parent.to_path_buf())
}

/// Flags no-ops (source already sits at its computed destination — the
/// reorganize-in-place case) and conflicts: destinations that collide with
/// an existing file on disk, or with another item resolved in this same
/// plan.
fn flag_conflicts(items: &mut [PlanItem]) {
    let mut destination_counts: HashMap<PathBuf, usize> = HashMap::new();
    for item in items.iter() {
        if let Some(dest) = &item.destination_path {
            *destination_counts.entry(dest.clone()).or_insert(0) += 1;
        }
    }

    for item in items.iter_mut() {
        let Some(dest) = &item.destination_path else {
            continue;
        };

        if is_same_file(&item.source_path, dest) {
            item.no_op = true;
            continue;
        }

        if destination_counts.get(dest).copied().unwrap_or(0) > 1 {
            item.conflict = ConflictKind::DuplicateInPlan;
        } else if dest.exists() {
            item.conflict = if has_identical_content(&item.source_path, dest) {
                ConflictKind::DuplicateAtDestination
            } else {
                ConflictKind::DestinationExists
            };
        }
    }
}

/// Canonicalizes both sides before comparing so a reorganize-in-place scan
/// (where source and destination roots overlap) recognizes a file already
/// sitting at its correctly-computed path, rather than flagging itself as a
/// conflict with itself.
fn is_same_file(source: &Path, destination: &Path) -> bool {
    match (fs::canonicalize(source), fs::canonicalize(destination)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Distinguishes a real naming collision from a harmless duplicate — see
/// `ConflictKind`. There is no `Overwrite` option anywhere in this app
/// precisely so this check can't be skipped: identical content is always
/// safe to leave alone, different content is never safe to destroy.
fn has_identical_content(source: &Path, destination: &Path) -> bool {
    match (dedup::content_hash(source), dedup::content_hash(destination)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    fn find<'a>(plan: &'a Plan, filename: &str) -> &'a PlanItem {
        plan.items
            .iter()
            .find(|item| item.source_path.file_name().unwrap().to_str().unwrap() == filename)
            .unwrap()
    }

    #[test]
    fn scan_with_progress_reports_the_running_count_and_matches_scan() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        fs::write(source.path().join("IMG_20230815_141523.jpg"), b"fixture").unwrap();
        fs::write(source.path().join("DSC00001.jpg"), b"fixture").unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };

        let mut counts = Vec::new();
        let plan = scan_with_progress(&options, |count| counts.push(count));

        assert_eq!(plan.items.len(), 2);
        counts.sort_unstable();
        assert_eq!(counts, vec![1, 2]);
    }

    #[test]
    fn walks_nested_already_organized_trees() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        fs::create_dir_all(source.path().join("2023/2023-08-15")).unwrap();
        fs::write(
            source.path().join("2023/2023-08-15/IMG_20230815_141523.jpg"),
            b"fixture",
        )
        .unwrap();
        fs::write(source.path().join("IMG_20230816_090000.jpg"), b"fixture").unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        assert_eq!(plan.items.len(), 2);
        let nested = find(&plan, "IMG_20230815_141523.jpg");
        assert_eq!(
            nested.destination_path.as_ref().unwrap(),
            &destination.path().join("2023/2023-08-15/IMG_20230815_141523.jpg")
        );
    }

    #[test]
    fn flags_destination_that_already_exists() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        fs::write(source.path().join("IMG_20230815_141523.jpg"), b"fixture").unwrap();
        fs::create_dir_all(destination.path().join("2023/2023-08-15")).unwrap();
        fs::write(
            destination
                .path()
                .join("2023/2023-08-15/IMG_20230815_141523.jpg"),
            b"already there",
        )
        .unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        assert_eq!(find(&plan, "IMG_20230815_141523.jpg").conflict, ConflictKind::DestinationExists);
    }

    #[test]
    fn flags_identical_content_at_destination_as_duplicate_not_a_conflict() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        fs::write(source.path().join("IMG_20230815_141523.jpg"), b"exact same bytes").unwrap();
        fs::create_dir_all(destination.path().join("2023/2023-08-15")).unwrap();
        fs::write(
            destination.path().join("2023/2023-08-15/IMG_20230815_141523.jpg"),
            b"exact same bytes",
        )
        .unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        assert_eq!(
            find(&plan, "IMG_20230815_141523.jpg").conflict,
            ConflictKind::DuplicateAtDestination
        );
    }

    #[test]
    fn flags_duplicate_destinations_within_the_same_plan() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        fs::create_dir_all(source.path().join("card_a")).unwrap();
        fs::create_dir_all(source.path().join("card_b")).unwrap();
        fs::write(
            source.path().join("card_a/IMG_20230815_141523.jpg"),
            b"fixture a",
        )
        .unwrap();
        fs::write(
            source.path().join("card_b/IMG_20230815_141523.jpg"),
            b"fixture b",
        )
        .unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        assert!(plan
            .items
            .iter()
            .all(|item| item.conflict == ConflictKind::DuplicateInPlan));
    }

    #[test]
    fn falls_back_to_low_confidence_fs_time_and_flags_needs_review() {
        // No EXIF, no filename pattern to match — only the fs mtime/ctime
        // fallback survives, which sits well under the review threshold.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        fs::write(source.path().join("DSC00001.jpg"), b"fixture").unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let item = find(&plan, "DSC00001.jpg");
        assert!(item.needs_review);
        assert!(item.chosen().unwrap().confidence < NEEDS_REVIEW_THRESHOLD);
        assert!(item.destination_path.is_some());
    }

    #[test]
    fn xmp_sidecar_date_flows_through_to_the_resolved_plan() {
        // No EXIF, no filename pattern — a RAW file with only a Lightroom
        // XMP sidecar for its capture date, the exact scenario this
        // integration is meant to cover.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        fs::write(source.path().join("DSC00001.CR3"), b"raw bytes").unwrap();
        fs::write(
            source.path().join("DSC00001.xmp"),
            br#"<rdf:Description exif:DateTimeOriginal="2023-08-15T14:15:23" xmlns:exif="http://ns.adobe.com/exif/1.0/"/>"#,
        )
        .unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let item = find(&plan, "DSC00001.CR3");
        let chosen = item.chosen().unwrap();
        assert_eq!(chosen.source, crate::date_resolution::DateSource::Xmp);
        assert_eq!(chosen.confidence, 0.9);
        assert!(!item.needs_review);
        assert_eq!(
            item.destination_path.as_ref().unwrap(),
            &destination.path().join("2023/2023-08-15/DSC00001.CR3")
        );
    }

    #[test]
    fn rrdata_sidecar_inherits_its_primarys_destination_and_review_status() {
        // The real-world bug this covers: a RapidRAW `.rrdata` edit-history
        // sidecar has no EXIF/XMP of its own, only its own (irrelevant)
        // filesystem timestamp — which on its own would resolve to a
        // low-confidence "needs review" date wildly unrelated to the photo
        // it travels with. It should instead just follow the primary.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        fs::write(
            source.path().join("IMG_20230815_141523.CR2"),
            b"raw bytes",
        )
        .unwrap();
        fs::write(
            source.path().join("IMG_20230815_141523.CR2.rrdata"),
            b"{}",
        )
        .unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let primary = find(&plan, "IMG_20230815_141523.CR2");
        assert!(!primary.needs_review);

        let sidecar = find(&plan, "IMG_20230815_141523.CR2.rrdata");
        assert!(
            !sidecar.needs_review,
            "sidecar should inherit the primary's review status, not its own fs-time guess"
        );
        assert_eq!(
            sidecar.chosen().map(|c| (c.date, c.source, c.confidence)),
            primary.chosen().map(|c| (c.date, c.source, c.confidence)),
            "sidecar should inherit the primary's resolved date"
        );
        assert_eq!(
            sidecar.destination_path.as_ref().unwrap(),
            &destination.path().join("2023/2023-08-15/IMG_20230815_141523.CR2.rrdata"),
            "sidecar should land next to its primary, not wherever its own timestamp resolves to"
        );
    }

    #[test]
    fn reorganize_in_place_already_correct_location_is_a_noop() {
        // Source and destination overlap — the actual reorganize-in-place
        // scenario — and the file already sits where it computes to.
        let library = tempfile::tempdir().unwrap();
        fs::create_dir_all(library.path().join("2023/2023-08-15")).unwrap();
        fs::write(
            library.path().join("2023/2023-08-15/IMG_20230815_141523.jpg"),
            b"fixture",
        )
        .unwrap();

        let options = ScanOptions {
            source_root: library.path(),
            destination_root: library.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let item = find(&plan, "IMG_20230815_141523.jpg");
        assert!(item.no_op);
        assert_eq!(item.conflict, ConflictKind::None);
    }

    #[test]
    fn reorganize_in_place_misfiled_file_is_still_a_pending_move() {
        let library = tempfile::tempdir().unwrap();
        // Sits in the wrong folder for its own resolved date.
        fs::create_dir_all(library.path().join("misc")).unwrap();
        fs::write(
            library.path().join("misc/IMG_20230815_141523.jpg"),
            b"fixture",
        )
        .unwrap();

        let options = ScanOptions {
            source_root: library.path(),
            destination_root: library.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let item = find(&plan, "IMG_20230815_141523.jpg");
        assert!(!item.no_op);
        assert_eq!(item.conflict, ConflictKind::None);
        assert_eq!(
            item.destination_path.as_ref().unwrap(),
            &library.path().join("2023/2023-08-15/IMG_20230815_141523.jpg")
        );
    }

    #[test]
    fn reorganize_in_place_preserves_a_legal_subfolder_under_the_date_folder() {
        // "Wedding" is a subfolder the user added on top of the correct
        // date structure — it should stay put, not get flattened away.
        let library = tempfile::tempdir().unwrap();
        fs::create_dir_all(library.path().join("2018/02 February/Wedding")).unwrap();
        fs::write(
            library.path().join("2018/02 February/Wedding/IMG_20180215_090000.jpg"),
            b"fixture",
        )
        .unwrap();

        let options = ScanOptions {
            source_root: library.path(),
            destination_root: library.path(),
            folder_template: "%Y/%m %B",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let item = find(&plan, "IMG_20180215_090000.jpg");
        assert!(item.no_op, "already in the right place, subfolder and all — nothing to do");
        assert_eq!(item.conflict, ConflictKind::None);
        assert_eq!(
            item.destination_path.as_ref().unwrap(),
            &library.path().join("2018/02 February/Wedding/IMG_20180215_090000.jpg")
        );
    }

    #[test]
    fn import_preserves_a_legal_subfolder_into_a_different_destination_root() {
        // Same idea, but importing from one root into a different one — the
        // subfolder should still carry over rather than being flattened.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join("2018/02 February/Wedding")).unwrap();
        fs::write(
            source.path().join("2018/02 February/Wedding/IMG_20180215_090000.jpg"),
            b"fixture",
        )
        .unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%m %B",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let item = find(&plan, "IMG_20180215_090000.jpg");
        assert_eq!(
            item.destination_path.as_ref().unwrap(),
            &destination.path().join("2018/02 February/Wedding/IMG_20180215_090000.jpg")
        );
    }

    #[test]
    fn a_subfolder_not_nested_under_the_date_folder_is_not_preserved() {
        // "misc" isn't part of the date structure at all — falls back to
        // the plain templated folder, same as before this feature existed.
        let library = tempfile::tempdir().unwrap();
        fs::create_dir_all(library.path().join("misc/Wedding")).unwrap();
        fs::write(
            library.path().join("misc/Wedding/IMG_20180215_090000.jpg"),
            b"fixture",
        )
        .unwrap();

        let options = ScanOptions {
            source_root: library.path(),
            destination_root: library.path(),
            folder_template: "%Y/%m %B",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let item = find(&plan, "IMG_20180215_090000.jpg");
        assert_eq!(
            item.destination_path.as_ref().unwrap(),
            &library.path().join("2018/02 February/IMG_20180215_090000.jpg")
        );
    }

    #[test]
    fn flags_files_already_present_in_the_index_by_content_hash() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(
            source.path().join("IMG_20230815_141523.jpg"),
            b"exact same bytes",
        )
        .unwrap();

        let conn = db::open_in_memory().unwrap();
        let batch_id = db::insert_batch(
            &conn,
            &db::NewBatch {
                started_at: "2026-08-01T00:00:00".to_string(),
                profile_id: None,
                kind: "import".to_string(),
                undo_log_path: "/tmp/undo.json".to_string(),
            },
        )
        .unwrap();
        let hash = dedup::content_hash(&source.path().join("IMG_20230815_141523.jpg")).unwrap();
        db::insert_file(
            &conn,
            &db::NewFileRecord {
                content_hash: hash,
                current_path: "/library/2023/2023-08-15/IMG_20230815_141523.jpg".to_string(),
                capture_date: Some("2023-08-15T14:15:23".to_string()),
                date_source: Some("exif".to_string()),
                date_confidence: Some(0.95),
                imported_at: "2026-08-01T00:00:01".to_string(),
                batch_id,
            },
        )
        .unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: Some(&conn),
        };
        let plan = scan(&options);

        assert!(find(&plan, "IMG_20230815_141523.jpg").already_imported);
    }

    #[test]
    fn does_not_flag_new_files_as_already_imported() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(source.path().join("IMG_20230815_141523.jpg"), b"brand new bytes").unwrap();

        let conn = db::open_in_memory().unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: Some(&conn),
        };
        let plan = scan(&options);

        assert!(!find(&plan, "IMG_20230815_141523.jpg").already_imported);
    }

    #[test]
    fn reuses_indexed_hashes_for_a_file_unchanged_since_it_was_indexed() {
        // Reorganize-in-place: source and destination are the same tree, so
        // a file already indexed at this exact path with a matching
        // size+mtime should be recognized as unchanged and skip hashing
        // entirely — proven here by planting a hash the real file's bytes
        // could never produce and confirming it comes back unchanged.
        let library = tempfile::tempdir().unwrap();
        let path = library.path().join("IMG_20230815_141523.jpg");
        fs::write(&path, b"fixture bytes").unwrap();

        let conn = db::open_in_memory().unwrap();
        let batch_id = db::insert_batch(
            &conn,
            &db::NewBatch {
                started_at: "2026-08-01T00:00:00".to_string(),
                profile_id: None,
                kind: "index".to_string(),
                undo_log_path: String::new(),
            },
        )
        .unwrap();
        let meta = fs::metadata(&path).unwrap();
        db::insert_indexed_file(
            &conn,
            &db::IndexedFileWrite {
                current_path: &path.to_string_lossy(),
                content_hash: "sentinel-hash-the-real-bytes-would-never-produce",
                size: Some(meta.len() as i64),
                mtime: dedup::mtime_secs(&meta),
                file_id: None,
            },
            "2026-08-01T00:00:01",
            batch_id,
        )
        .unwrap();

        let options = ScanOptions {
            source_root: library.path(),
            destination_root: library.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: Some(&conn),
        };
        let plan = scan(&options);
        let item = find(&plan, "IMG_20230815_141523.jpg");

        assert_eq!(item.content_hash.as_deref(), Some("sentinel-hash-the-real-bytes-would-never-produce"));
        assert!(item.already_imported);
    }

    #[test]
    fn rehashes_when_the_file_changed_since_it_was_indexed() {
        let library = tempfile::tempdir().unwrap();
        let path = library.path().join("IMG_20230815_141523.jpg");
        fs::write(&path, b"fixture bytes").unwrap();

        let conn = db::open_in_memory().unwrap();
        let batch_id = db::insert_batch(
            &conn,
            &db::NewBatch {
                started_at: "2026-08-01T00:00:00".to_string(),
                profile_id: None,
                kind: "index".to_string(),
                undo_log_path: String::new(),
            },
        )
        .unwrap();
        // Deliberately stale size/mtime, as if the file changed since this
        // row was written.
        db::insert_indexed_file(
            &conn,
            &db::IndexedFileWrite {
                current_path: &path.to_string_lossy(),
                content_hash: "stale-hash",
                size: Some(999_999),
                mtime: Some(1),
                file_id: None,
            },
            "2026-08-01T00:00:01",
            batch_id,
        )
        .unwrap();

        let options = ScanOptions {
            source_root: library.path(),
            destination_root: library.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: Some(&conn),
        };
        let plan = scan(&options);
        let item = find(&plan, "IMG_20230815_141523.jpg");

        let real_hash = dedup::content_hash(&path).unwrap();
        assert_eq!(item.content_hash.as_deref(), Some(real_hash.as_str()));
    }
}

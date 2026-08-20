use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
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
    flag_conflicts(&mut items, options.index);

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
///
/// Also collects the extensions of any *true* sidecar siblings (`is_sidecar`
/// — `.xmp`/`.rrdata`/`.rrexif`, not a real paired file like a RAW+JPEG) onto
/// the primary's `sidecar_extensions`, so the Plan table can show one
/// "Sidecars" column on the primary's row instead of a separate row per
/// sidecar file.
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

        let mut sidecar_extensions = Vec::new();
        for &sibling_index in siblings {
            if let Some(filename) = items[sibling_index].source_path.file_name() {
                items[sibling_index].destination_path = Some(primary_folder.join(filename));
            }
            items[sibling_index].candidates = primary_candidates.clone();
            items[sibling_index].needs_review = primary_needs_review;

            if items[sibling_index].is_sidecar
                && let Some(ext) = items[sibling_index].source_path.extension().and_then(|s| s.to_str())
            {
                sidecar_extensions.push(ext.to_lowercase());
            }
        }
        items[*primary_index].sidecar_extensions = sidecar_extensions;
    }
}

fn build_plan_item(source_path: &Path, options: &ScanOptions) -> PlanItem {
    let filename = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    // One stat, reused below for both fs timestamps and the indexed-row
    // staleness check — rather than three separate `fs::metadata` calls for
    // the same file.
    let metadata = fs::metadata(source_path).ok();

    // One indexed-row lookup (when indexing at all), reused below for both
    // the resolved-date fast path and the content-hash fast path — a single
    // DB round-trip per file instead of two. `None` just means "no
    // fast-path data for this exact path" (never indexed, a different path,
    // or changed since) — always safe to fall through to a real read.
    let indexed = options.index.and_then(|conn| unchanged_indexed_row(conn, source_path, &metadata));

    // EXIF/XMP are the only two inputs that cost a real file read. A
    // previous resolution's winning source says whether that read can be
    // skipped for a file unchanged since then: if EXIF or XMP won, its
    // stored date is reused directly; if filename or an fs time won, EXIF
    // and XMP must have been absent/implausible at import time (an
    // authoritative source is never outranked once present — see
    // `date_resolution::downgrade_conflicting`), so skipping the read here
    // reproduces the exact same inputs, not an approximation. These labels
    // must stay in sync with `commit::date_source_label`.
    let cached_exif = cached_date(&indexed, "exif");
    let cached_xmp = cached_date(&indexed, "xmp");
    let exif_xmp_settled = cached_exif.is_some()
        || cached_xmp.is_some()
        || matches!(indexed.as_ref().and_then(|row| row.date_source.as_deref()), Some("filename") | Some("mtime"));

    // One open file, reused below for both the EXIF read and, when hashing
    // actually turns out to be necessary, the content hash — see
    // `compute_hash`'s doc comment for why that's more than just saving a
    // syscall. Skipped entirely when the date fast path above already
    // settled EXIF/XMP, since then the only remaining reason to open it —
    // hashing — is also already settled (`indexed` covers both).
    let mut file_reader = if exif_xmp_settled {
        None
    } else {
        fs::File::open(source_path).ok().map(BufReader::new)
    };

    let inputs = DateInputs {
        filename,
        exif_date_time_original: cached_exif.or_else(|| file_reader.as_mut().and_then(read_exif_date)),
        xmp_date_time_original: cached_xmp
            .or_else(|| (!exif_xmp_settled).then(|| sidecar_interop::read_xmp_date(source_path)).flatten()),
        fs_created: metadata.as_ref().and_then(|m| file_time(m, |m| m.created())),
        fs_modified: metadata.as_ref().and_then(|m| file_time(m, |m| m.modified())),
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

    let (content_hash, already_imported) =
        compute_hash(source_path, file_reader.as_mut(), indexed.as_ref(), options.index, options.destination_root);

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
        is_sidecar: crate::formats::is_sidecar_file(source_path),
        sidecar_extensions: Vec::new(),
    }
}

/// Re-renders every item's `destination_path` against a new folder
/// template, reusing each item's already-resolved date candidates and
/// content hash instead of re-walking/re-reading the source tree — the
/// folder template only ever feeds `render_template`'s output, never which
/// files exist or what date they resolve to. Still does the same read-only
/// destination-side existence/hash checks `scan` does (via `flag_conflicts`)
/// to keep conflict/no-op flags accurate for the *new* paths; nothing on
/// disk is written and no source file is re-read either way.
pub fn retemplate_plan(
    plan: &Plan,
    source_root: &Path,
    destination_root: &Path,
    folder_template: &str,
    index: Option<&Connection>,
) -> Plan {
    let mut items: Vec<PlanItem> = plan
        .items
        .iter()
        .cloned()
        .map(|mut item| {
            item.destination_path = item.chosen().map(|candidate| {
                let rendered_folder = render_template(folder_template, candidate.date);
                let destination_folder =
                    preserve_existing_subfolder(&item.source_path, source_root, &rendered_folder)
                        .unwrap_or_else(|| PathBuf::from(&rendered_folder));
                let filename = item.source_path.file_name().unwrap_or_default();
                destination_root.join(destination_folder).join(filename)
            });
            item.no_op = false;
            item.conflict = ConflictKind::None;
            item
        })
        .collect();

    align_sidecars_with_primary(&mut items);
    flag_conflicts(&mut items, index);

    Plan { items }
}

/// The indexed row at this exact path, only if it's unchanged (matching
/// size+mtime) since it was last indexed — i.e. safe to trust its stored
/// content hash and resolved date without re-reading the file. `None` for
/// any other reason (never indexed, a different path, or genuinely changed)
/// just means "no fast-path data," never a wrong answer either way: a
/// content change always clears the row's cached hash/date together (see
/// `db::update_file_content`/`db::insert_indexed_file`), so a stale
/// size+mtime match can never carry a stale hash or date along with it.
fn unchanged_indexed_row(conn: &Connection, path: &Path, metadata: &Option<fs::Metadata>) -> Option<db::IndexedFileMeta> {
    let size = metadata.as_ref().map(|m| m.len() as i64);
    let mtime = metadata.as_ref().and_then(dedup::mtime_secs);
    let row = db::find_by_path(conn, &path.to_string_lossy()).ok().flatten()?;
    (row.size == size && row.mtime == mtime).then_some(row)
}

/// The indexed row's stored capture date, only if it was actually won by
/// `label` ("exif" or "xmp" — the two sources a real file read is needed
/// for). Malformed/unparseable stored data (shouldn't happen, but this
/// column has no format constraint at the SQL level) is treated the same
/// as "not cached" — falls through to a real read rather than erroring.
fn cached_date(indexed: &Option<db::IndexedFileMeta>, label: &str) -> Option<NaiveDateTime> {
    let row = indexed.as_ref()?;
    (row.date_source.as_deref() == Some(label))
        .then_some(row.capture_date.as_deref())
        .flatten()
        .and_then(|date| NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%S").ok())
}

/// Computes this file's content hash and whether it's already present
/// *elsewhere in this destination* — or, when possible, skips hashing
/// entirely.
///
/// The fast path: `indexed` (already looked up once in `build_plan_item`
/// via `unchanged_indexed_row`, alongside the date fast path) means this
/// file is unchanged since it was last indexed/imported, so its stored hash
/// is reused verbatim instead of re-reading the file. This only ever
/// triggers for reorganize-in-place (source_root == destination_root) — a
/// plain import's source path (an SD card) never coincides with any indexed
/// `current_path` — which is exactly the case that used to mean "rehash
/// every file in the library, every time you click Scan."
///
/// Taking the fast path only tells us this file has a row at its *own*
/// current path — during reorganize-in-place that's true for nearly every
/// already-indexed file, misfiled or not, so it can never be trusted as
/// "duplicate content exists" on its own (see `has_other_indexed_copy`).
///
/// `dedup::find_duplicates` reuses whatever this returns rather than
/// hashing a second time itself.
///
/// `file_reader` is `build_plan_item`'s already-open EXIF reader (`None`
/// when the date fast path determined it wasn't needed at all — in which
/// case `indexed` is always `Some` too, so hashing is never reached below).
/// When a hash actually needs computing, the reader is rewound and reused
/// instead of a second `File::open`. For TIFF-based RAW formats,
/// `read_exif_date` already reads the *entire* file into memory to locate
/// the EXIF IFD (`kamadak-exif`'s own behavior, not a choice made here) —
/// reusing that handle avoids reading those bytes off disk twice, not just
/// avoiding a syscall.
fn compute_hash(
    source_path: &Path,
    file_reader: Option<&mut BufReader<fs::File>>,
    indexed: Option<&db::IndexedFileMeta>,
    index: Option<&Connection>,
    destination_root: &Path,
) -> (Option<String>, bool) {
    let Some(conn) = index else {
        return (None, false);
    };

    if let Some(row) = indexed {
        let already_imported = has_other_indexed_copy(conn, &row.content_hash, source_path, destination_root);
        return (Some(row.content_hash.clone()), already_imported);
    }

    let content_hash = match file_reader {
        Some(reader) => reader
            .seek(SeekFrom::Start(0))
            .ok()
            .and_then(|_| dedup::hash_reader(reader).ok()),
        None => None,
    }
    .or_else(|| dedup::content_hash(source_path).ok());
    let Some(content_hash) = content_hash else {
        return (None, false);
    };
    let already_imported = has_other_indexed_copy(conn, &content_hash, source_path, destination_root);
    (Some(content_hash), already_imported)
}

/// Whether `hash` is indexed at any path other than `source_path` itself,
/// *and* under `destination_root` — the real meaning of "already imported":
/// duplicate content already in the library you're currently scanning into
/// or reorganizing, not this exact file matching its own indexed row, and
/// not a match against some other, unrelated destination this app has
/// indexed at some point. `library.sqlite` is one global index shared
/// across every destination ever scanned — without the `destination_root`
/// filter, scanning source A into destination B could show "Already
/// imported" for content that only actually exists under an unrelated
/// destination C, which is misleading: nothing about *this* scan says B and
/// C are the same library.
///
/// Every plain-import source path (an SD card) never coincides with any
/// indexed `current_path`, so the self-match half of this filter is a no-op
/// there — it only changes behavior for reorganize-in-place, where a file's
/// own current path is always indexed by the time you'd want to reorganize
/// it. Without it, that self-match alone used to mark nearly every
/// already-indexed file "already imported," which at commit time made
/// `commit::commit_item` route a misfiled-but-indexed file through
/// `relocate_duplicate` — a no-op under the default `DuplicatePolicy::Skip`
/// — instead of actually moving it to its correct folder.
fn has_other_indexed_copy(conn: &Connection, hash: &str, source_path: &Path, destination_root: &Path) -> bool {
    db::find_paths_by_content_hash(conn, hash)
        .unwrap_or_default()
        .iter()
        .any(|indexed_path| {
            let indexed_path = Path::new(indexed_path);
            indexed_path != source_path && indexed_path.starts_with(destination_root)
        })
}

fn read_exif_date<R: BufRead + Seek>(reader: &mut R) -> Option<NaiveDateTime> {
    let exif = exif::Reader::new().read_from_container(reader).ok()?;
    let field = exif.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)?;
    let raw = field.display_value().to_string();
    let cleaned = raw.trim_matches('"').trim();
    NaiveDateTime::parse_from_str(cleaned, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(cleaned, "%Y:%m:%d %H:%M:%S"))
        .ok()
}

fn file_time(
    metadata: &fs::Metadata,
    extract: fn(&fs::Metadata) -> std::io::Result<SystemTime>,
) -> Option<NaiveDateTime> {
    let system_time = extract(metadata).ok()?;
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
fn flag_conflicts(items: &mut [PlanItem], index: Option<&Connection>) {
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
            let is_duplicate = has_identical_content(&item.source_path, dest, item.content_hash.as_deref(), index);
            item.conflict = if is_duplicate {
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
///
/// `known_source_hash` is `item.content_hash`, already computed by
/// `compute_hash` for every item — reused here instead of rehashing the
/// source file a second time. Only falls back to hashing it fresh when
/// there is none (indexing was off for this scan). The destination side
/// gets the same treatment via `indexed_hash_if_unchanged` — a real naming
/// collision only ever happens against a file already living in the
/// library, which is exactly the kind of file a prior `reindex_cli`/
/// "Refresh Library Index" run is likely to have already hashed.
fn has_identical_content(source: &Path, destination: &Path, known_source_hash: Option<&str>, index: Option<&Connection>) -> bool {
    let source_hash = match known_source_hash {
        Some(hash) => hash.to_string(),
        None => match dedup::content_hash(source) {
            Ok(hash) => hash,
            Err(_) => return false,
        },
    };

    let dest_hash = match indexed_hash_if_unchanged(destination, index) {
        Some(hash) => hash,
        None => match dedup::content_hash(destination) {
            Ok(hash) => hash,
            Err(_) => return false,
        },
    };

    dest_hash == source_hash
}

/// Mirrors `compute_hash`'s fast path but for a destination path: if the
/// index already has a row here with matching size+mtime, its stored hash
/// is reused instead of rehashing a file that's already been hashed once
/// before.
fn indexed_hash_if_unchanged(path: &Path, index: Option<&Connection>) -> Option<String> {
    let conn = index?;
    let metadata = fs::metadata(path).ok();
    unchanged_indexed_row(conn, path, &metadata).map(|row| row.content_hash)
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
    fn reuses_the_indexed_hash_for_an_unchanged_destination_file() {
        // Proves `has_identical_content` reuses a stored hash for the
        // destination side rather than rehashing it: the indexed hash here
        // is planted equal to the *source*'s hash (so a match means the
        // stored value was used), while the destination's real on-disk
        // bytes differ (so a fresh rehash would say DestinationExists, not
        // DuplicateAtDestination).
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        let source_path = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&source_path, b"source bytes").unwrap();
        let source_hash = dedup::content_hash(&source_path).unwrap();

        let dest_folder = destination.path().join("2023").join("2023-08-15");
        fs::create_dir_all(&dest_folder).unwrap();
        let dest_path = dest_folder.join("IMG_20230815_141523.jpg");
        fs::write(&dest_path, b"different bytes actually on disk at the destination").unwrap();
        let dest_meta = fs::metadata(&dest_path).unwrap();

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
        db::insert_indexed_file(
            &conn,
            &db::IndexedFileWrite {
                current_path: &dest_path.to_string_lossy(),
                content_hash: &source_hash,
                size: Some(dest_meta.len() as i64),
                mtime: dedup::mtime_secs(&dest_meta),
                file_id: None,
            },
            "2026-08-01T00:00:01",
            batch_id,
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

        assert_eq!(
            find(&plan, "IMG_20230815_141523.jpg").conflict,
            ConflictKind::DuplicateAtDestination,
            "should trust the indexed hash rather than rehashing the destination's real (different) bytes"
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
    fn resolves_a_compact_video_filename_from_its_own_timestamp_not_fs_time() {
        // Regression test for the reported bug: a file named e.g.
        // VID20260716183013.mp4 was resolving from fs_created (today,
        // whenever the test actually runs) instead of the date plainly
        // encoded in its own filename, because no filename pattern covered
        // a compact YYYYMMDDHHMMSS run with no separator between date and
        // time. Real fs_created/fs_modified here will be "now" (whenever
        // this test runs), never 2026-07-16 — so this only passes if the
        // filename genuinely won.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(source.path().join("VID20260716183013.mp4"), b"fixture").unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let item = find(&plan, "VID20260716183013.mp4");
        let chosen = item.chosen().unwrap();
        assert_eq!(chosen.source, crate::date_resolution::DateSource::Filename);
        assert_eq!(chosen.date, NaiveDate::from_ymd_opt(2026, 7, 16).unwrap().and_hms_opt(18, 30, 13).unwrap());
        assert!(!item.needs_review);
        assert_eq!(
            item.destination_path.as_ref().unwrap(),
            &destination.path().join("2026").join("2026-07-16").join("VID20260716183013.mp4")
        );
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
        assert!(!primary.is_sidecar);
        assert_eq!(
            primary.sidecar_extensions,
            vec!["rrdata".to_string()],
            "primary should list its sidecar's extension for the UI's Sidecars column"
        );

        let sidecar = find(&plan, "IMG_20230815_141523.CR2.rrdata");
        assert!(sidecar.is_sidecar);
        assert!(
            sidecar.sidecar_extensions.is_empty(),
            "the sidecar itself has no sidecars of its own"
        );
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
    fn primary_lists_every_sidecar_extension_when_it_has_more_than_one() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();

        fs::write(source.path().join("IMG_0001.CR3"), b"raw bytes").unwrap();
        fs::write(source.path().join("IMG_0001.CR3.xmp"), b"<x/>").unwrap();
        fs::write(source.path().join("IMG_0001.CR3.rrdata"), b"{}").unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let mut extensions = find(&plan, "IMG_0001.CR3").sidecar_extensions.clone();
        extensions.sort();
        assert_eq!(extensions, vec!["rrdata".to_string(), "xmp".to_string()]);
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
        // Under destination.path(), not some unrelated hardcoded path -
        // already_imported is scoped to matches under this scan's own
        // destination_root, not the whole global index.
        let indexed_path = destination.path().join("2023/2023-08-15/IMG_20230815_141523.jpg");
        db::insert_file(
            &conn,
            &db::NewFileRecord {
                content_hash: hash,
                current_path: indexed_path.to_string_lossy().to_string(),
                capture_date: Some("2023-08-15T14:15:23".to_string()),
                date_source: Some("exif".to_string()),
                date_confidence: Some(0.95),
                imported_at: "2026-08-01T00:00:01".to_string(),
                batch_id,
                ..Default::default()
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
        //
        // The only indexed row here is this file's own row at its own
        // path — no other copy exists anywhere — so already_imported must
        // stay false. It used to come back true purely from matching
        // itself, which silently broke reorganize-in-place: commit_item
        // would route a misfiled-but-indexed file through
        // relocate_duplicate (a no-op under the default
        // DuplicatePolicy::Skip) instead of actually moving it.
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
        assert!(!item.already_imported, "matching only its own indexed row is not a duplicate");
    }

    #[test]
    fn already_imported_is_true_for_a_genuine_second_copy_during_reorganize() {
        // Same fast path as the test above, but this time a *second*,
        // different-path row shares the hash too — a real duplicate found
        // during reorganize-in-place (e.g. the same photo already sitting
        // under two different folder-template runs). This one must come
        // back true.
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
                content_hash: "shared-hash",
                size: Some(meta.len() as i64),
                mtime: dedup::mtime_secs(&meta),
                file_id: None,
            },
            "2026-08-01T00:00:01",
            batch_id,
        )
        .unwrap();
        db::insert_indexed_file(
            &conn,
            &db::IndexedFileWrite {
                current_path: &library.path().join("elsewhere/IMG_20230815_141523.jpg").to_string_lossy(),
                content_hash: "shared-hash",
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

        assert!(item.already_imported, "a second copy at a different path is a genuine duplicate");
    }

    #[test]
    fn already_imported_ignores_a_match_outside_this_scans_destination_root() {
        // library.sqlite is one global index shared across every destination
        // this app has ever scanned. A source file scanned into destination
        // A shouldn't come back "already imported" just because its content
        // happens to also live under some unrelated destination B - that
        // match has nothing to do with the library this scan is actually
        // working with.
        let source = tempfile::tempdir().unwrap();
        let destination_a = tempfile::tempdir().unwrap();
        let destination_b = tempfile::tempdir().unwrap();
        let scanned = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&scanned, b"fixture bytes").unwrap();

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
        db::insert_file(
            &conn,
            &db::NewFileRecord {
                content_hash: dedup::content_hash(&scanned).unwrap(),
                current_path: destination_b.path().join("existing.jpg").to_string_lossy().to_string(),
                capture_date: None,
                date_source: None,
                date_confidence: None,
                imported_at: "2026-08-01T00:00:01".to_string(),
                batch_id,
                ..Default::default()
            },
        )
        .unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination_a.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: Some(&conn),
        };
        let plan = scan(&options);
        let item = find(&plan, "IMG_20230815_141523.jpg");

        assert!(
            !item.already_imported,
            "a match under an unrelated destination must not count as already imported here"
        );
    }

    #[test]
    fn a_misfiled_indexed_file_is_still_a_pending_move_not_already_imported() {
        // Regression test for the real bug this fixes: an already-indexed
        // library being reorganized in place must not treat a misfiled
        // file's own indexed row as "already imported" — that flag used to
        // make commit::commit_item skip actually moving it.
        let library = tempfile::tempdir().unwrap();
        fs::create_dir_all(library.path().join("misc")).unwrap();
        let misfiled = library.path().join("misc/IMG_20230815_141523.jpg");
        fs::write(&misfiled, b"fixture bytes").unwrap();

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
        let meta = fs::metadata(&misfiled).unwrap();
        db::insert_indexed_file(
            &conn,
            &db::IndexedFileWrite {
                current_path: &misfiled.to_string_lossy(),
                content_hash: &dedup::content_hash(&misfiled).unwrap(),
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

        assert!(!item.already_imported);
        assert!(!item.no_op);
        assert_eq!(
            item.destination_path.as_ref().unwrap(),
            &library.path().join("2023/2023-08-15/IMG_20230815_141523.jpg")
        );
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

    #[test]
    fn reuses_the_indexed_date_for_a_file_unchanged_since_it_was_indexed() {
        // "DSC00001.jpg" has no filename-pattern match, and the fixture
        // bytes below aren't real EXIF — so the only way the planted
        // 2020-01-01 date could show up as chosen is via the cached-date
        // fast path, not a real (impossible) read of this fixture.
        let library = tempfile::tempdir().unwrap();
        let path = library.path().join("DSC00001.jpg");
        fs::write(&path, b"fixture bytes").unwrap();
        let meta = fs::metadata(&path).unwrap();

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
        db::insert_file(
            &conn,
            &db::NewFileRecord {
                content_hash: "sentinel-hash".to_string(),
                current_path: path.to_string_lossy().to_string(),
                capture_date: Some("2020-01-01T00:00:00".to_string()),
                date_source: Some("exif".to_string()),
                date_confidence: Some(0.95),
                imported_at: "2026-08-01T00:00:01".to_string(),
                batch_id,
                size: Some(meta.len() as i64),
                mtime: dedup::mtime_secs(&meta),
            },
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
        let chosen = find(&plan, "DSC00001.jpg").chosen().unwrap();

        assert_eq!(
            chosen.date,
            NaiveDateTime::parse_from_str("2020-01-01T00:00:00", "%Y-%m-%dT%H:%M:%S").unwrap()
        );
        assert_eq!(chosen.source, crate::date_resolution::DateSource::Exif);
    }

    #[test]
    fn retemplate_moves_items_to_the_new_templates_folder() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(source.path().join("IMG_20230815_141523.jpg"), b"fixture").unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let retemplated = retemplate_plan(&plan, source.path(), destination.path(), "%Y/%m %B", None);

        let item = find(&retemplated, "IMG_20230815_141523.jpg");
        assert_eq!(
            item.destination_path.as_ref().unwrap(),
            &destination.path().join("2023/08 August/IMG_20230815_141523.jpg")
        );
    }

    #[test]
    fn retemplate_does_not_rehash_or_reread_the_source_file() {
        // The fixture bytes aren't real EXIF, so if retemplate re-resolved
        // the date from scratch it would fall back to a low-confidence fs
        // time instead of reusing the original filename-derived date —
        // proving the chosen candidate (and its source) survives untouched.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(source.path().join("IMG_20230815_141523.jpg"), b"fixture").unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);
        let original_chosen = find(&plan, "IMG_20230815_141523.jpg")
            .chosen()
            .map(|c| (c.date, c.source, c.confidence));

        let retemplated = retemplate_plan(&plan, source.path(), destination.path(), "%Y/%m %B", None);

        assert_eq!(
            find(&retemplated, "IMG_20230815_141523.jpg").chosen().map(|c| (c.date, c.source, c.confidence)),
            original_chosen
        );
    }

    #[test]
    fn retemplate_reflags_a_conflict_that_only_exists_at_the_new_destination() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(source.path().join("IMG_20230815_141523.jpg"), b"fixture").unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);
        assert_eq!(find(&plan, "IMG_20230815_141523.jpg").conflict, ConflictKind::None);

        // Nothing collides under the original template, but something
        // already sits at the *new* template's computed path.
        fs::create_dir_all(destination.path().join("2023/08 August")).unwrap();
        fs::write(
            destination.path().join("2023/08 August/IMG_20230815_141523.jpg"),
            b"already there",
        )
        .unwrap();

        let retemplated = retemplate_plan(&plan, source.path(), destination.path(), "%Y/%m %B", None);

        assert_eq!(
            find(&retemplated, "IMG_20230815_141523.jpg").conflict,
            ConflictKind::DestinationExists
        );
    }

    #[test]
    fn retemplate_clears_a_conflict_that_no_longer_applies_under_the_new_destination() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(source.path().join("IMG_20230815_141523.jpg"), b"fixture").unwrap();
        fs::create_dir_all(destination.path().join("2023/2023-08-15")).unwrap();
        fs::write(
            destination.path().join("2023/2023-08-15/IMG_20230815_141523.jpg"),
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

        let retemplated = retemplate_plan(&plan, source.path(), destination.path(), "%Y/%m %B", None);

        assert_eq!(find(&retemplated, "IMG_20230815_141523.jpg").conflict, ConflictKind::None);
    }

    #[test]
    fn retemplate_keeps_a_sidecar_aligned_with_its_primary_under_the_new_template() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(source.path().join("IMG_20230815_141523.CR2"), b"raw bytes").unwrap();
        fs::write(source.path().join("IMG_20230815_141523.CR2.rrdata"), b"{}").unwrap();

        let options = ScanOptions {
            source_root: source.path(),
            destination_root: destination.path(),
            folder_template: "%Y/%Y-%m-%d",
            now: today(),
            index: None,
        };
        let plan = scan(&options);

        let retemplated = retemplate_plan(&plan, source.path(), destination.path(), "%Y/%m %B", None);

        let primary = find(&retemplated, "IMG_20230815_141523.CR2");
        let sidecar = find(&retemplated, "IMG_20230815_141523.CR2.rrdata");
        assert_eq!(sidecar.destination_path.as_deref().and_then(Path::parent), primary.destination_path.as_deref().and_then(Path::parent));
    }

    #[test]
    fn skips_the_exif_xmp_read_when_a_previous_scan_found_neither_present() {
        // date_source "mtime" means EXIF/XMP were absent/implausible last
        // time this file was resolved (an authoritative source is never
        // outranked once present, so "mtime" winning proves neither was
        // there) — the filename pattern's date should still win fresh
        // (computed from the path string, not the index) even though the
        // EXIF/XMP read itself is skipped.
        let library = tempfile::tempdir().unwrap();
        let path = library.path().join("IMG_20230815_141523.jpg");
        fs::write(&path, b"fixture bytes").unwrap();
        let meta = fs::metadata(&path).unwrap();

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
        db::insert_file(
            &conn,
            &db::NewFileRecord {
                content_hash: "sentinel-hash".to_string(),
                current_path: path.to_string_lossy().to_string(),
                capture_date: Some("2023-08-15T09:00:00".to_string()),
                date_source: Some("mtime".to_string()),
                date_confidence: Some(0.2),
                imported_at: "2026-08-01T00:00:01".to_string(),
                batch_id,
                size: Some(meta.len() as i64),
                mtime: dedup::mtime_secs(&meta),
            },
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
        let chosen = find(&plan, "IMG_20230815_141523.jpg").chosen().unwrap();

        assert_eq!(
            chosen.source,
            crate::date_resolution::DateSource::Filename,
            "the filename pattern should still win fresh, even though the EXIF/XMP read was skipped"
        );
    }

    // Minimal ISOBMFF/HEIF file wrapping a hand-built TIFF/Exif blob with a
    // single DateTimeOriginal tag, built the same way kamadak-exif's own
    // isobmff.rs test suite constructs its fixtures (see `unknown_before_ftyp`
    // in that crate). A real HEIC photo isn't practical to author or vendor
    // into this repo just for a test fixture, but the container format
    // itself is simple enough to build directly here — which also proves
    // this app's own `read_exif_date` actually drives the exif crate's
    // ISOBMFF path correctly, not just that the crate supports it in the
    // abstract.
    fn u32be(n: u32) -> [u8; 4] {
        n.to_be_bytes()
    }

    fn minimal_heic_with_date_time_original(date_time: &str) -> Vec<u8> {
        assert_eq!(date_time.len(), 19, "expected YYYY:MM:DD HH:MM:SS");
        let mut value = date_time.as_bytes().to_vec();
        value.push(0); // NUL-terminated ASCII, per the Exif spec

        // --- TIFF/Exif blob: header, IFD0 (just an ExifIFDPointer), the
        // Exif SubIFD (just DateTimeOriginal), then the string itself.
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"MM"); // big-endian
        tiff.extend_from_slice(&[0x00, 0x2a]); // TIFF magic
        tiff.extend_from_slice(&u32be(8)); // offset of IFD0

        let ifd0_offset = 8u32;
        let exif_subifd_offset = ifd0_offset + 2 + 12 + 4; // count + 1 entry + next-IFD offset
        tiff.extend_from_slice(&(1u16).to_be_bytes()); // IFD0: 1 entry
        tiff.extend_from_slice(&(0x8769u16).to_be_bytes()); // tag: ExifIFDPointer
        tiff.extend_from_slice(&(4u16).to_be_bytes()); // type: LONG
        tiff.extend_from_slice(&u32be(1)); // count
        tiff.extend_from_slice(&u32be(exif_subifd_offset)); // value: offset to Exif SubIFD
        tiff.extend_from_slice(&u32be(0)); // no next IFD

        let value_offset = exif_subifd_offset + 2 + 12 + 4;
        assert_eq!(tiff.len() as u32, exif_subifd_offset, "offset arithmetic must match actual position");
        tiff.extend_from_slice(&(1u16).to_be_bytes()); // Exif SubIFD: 1 entry
        tiff.extend_from_slice(&(0x9003u16).to_be_bytes()); // tag: DateTimeOriginal
        tiff.extend_from_slice(&(2u16).to_be_bytes()); // type: ASCII
        tiff.extend_from_slice(&u32be(value.len() as u32)); // count, incl. NUL
        tiff.extend_from_slice(&u32be(value_offset)); // value: offset to the string
        tiff.extend_from_slice(&u32be(0)); // no next IFD
        assert_eq!(tiff.len() as u32, value_offset, "offset arithmetic must match actual position");
        tiff.extend_from_slice(&value);

        // --- ISOBMFF container: ftyp (declares HEIF-compatible), then a
        // meta box holding one "Exif" item (iinf/iloc/idat), pointing at the
        // TIFF blob above.
        let mut ftyp_body = Vec::new();
        ftyp_body.extend_from_slice(b"heic"); // major brand
        ftyp_body.extend_from_slice(&u32be(0)); // minor version
        ftyp_body.extend_from_slice(b"mif1"); // compatible brand kamadak-exif checks for
        let ftyp = boxed(b"ftyp", &ftyp_body);

        // Item id is arbitrary - iloc and infe just need to agree.
        let item_id: u16 = 1;

        let mut infe_body = Vec::new();
        infe_body.extend_from_slice(&[0x02, 0, 0, 0]); // fullbox version=2, flags=0
        infe_body.extend_from_slice(&item_id.to_be_bytes()); // item_id (version 2 -> u16)
        infe_body.extend_from_slice(&[0, 0]); // item_protection_index
        infe_body.extend_from_slice(b"Exif"); // item_type
        let infe = boxed(b"infe", &infe_body);

        let mut iinf_body = Vec::new();
        iinf_body.extend_from_slice(&[0, 0, 0, 0]); // fullbox version=0, flags=0
        iinf_body.extend_from_slice(&(1u16).to_be_bytes()); // entry_count
        iinf_body.extend_from_slice(&infe);
        let iinf = boxed(b"iinf", &iinf_body);

        // version=1, every size field zeroed -> one extent covering the
        // whole idat box's content (see kamadak-exif's isobmff.rs: a zero
        // `len` means "read idat to the end", not "read zero bytes").
        let mut iloc_body = Vec::new();
        iloc_body.extend_from_slice(&[0x01, 0, 0, 0]); // fullbox version=1, flags=0
        iloc_body.extend_from_slice(&[0, 0]); // offset/length/base_offset/index sizes, all 0
        iloc_body.extend_from_slice(&(1u16).to_be_bytes()); // item_count
        iloc_body.extend_from_slice(&item_id.to_be_bytes());
        iloc_body.extend_from_slice(&[0, 0x01]); // construction_method=1 (idat-based)
        iloc_body.extend_from_slice(&[0, 0]); // data_ref_index
        iloc_body.extend_from_slice(&(1u16).to_be_bytes()); // extent_count
        let iloc = boxed(b"iloc", &iloc_body);

        // The Exif item's own data block: a 4-byte "offset to TIFF header"
        // prefix (0 - no padding here) followed by the TIFF blob itself.
        let mut idat_body = Vec::new();
        idat_body.extend_from_slice(&u32be(0));
        idat_body.extend_from_slice(&tiff);
        let idat = boxed(b"idat", &idat_body);

        let mut meta_body = Vec::new();
        meta_body.extend_from_slice(&[0, 0, 0, 0]); // fullbox version=0, flags=0
        meta_body.extend_from_slice(&iloc);
        meta_body.extend_from_slice(&iinf);
        meta_body.extend_from_slice(&idat);
        let meta = boxed(b"meta", &meta_body);

        let mut file = Vec::new();
        file.extend_from_slice(&ftyp);
        file.extend_from_slice(&meta);
        file
    }

    fn boxed(box_type: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&u32be(8 + body.len() as u32));
        out.extend_from_slice(box_type);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn heic_exif_date_flows_through_to_the_resolved_plan() {
        // iPhones have shot HEIC by default since iOS 11 - the extension
        // whitelist in formats.rs now admits it, but that's only half the
        // fix: this proves an actual DateTimeOriginal embedded in a HEIC
        // file's ISOBMFF container is read and wins over the filename/fs
        // fallbacks, the same as it would for a JPEG.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::write(
            source.path().join("IMG_0001.heic"),
            minimal_heic_with_date_time_original("2023:08:15 14:15:23"),
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

        let item = find(&plan, "IMG_0001.heic");
        let chosen = item.chosen().unwrap();
        assert_eq!(chosen.source, crate::date_resolution::DateSource::Exif);
        assert_eq!(chosen.date, NaiveDate::from_ymd_opt(2023, 8, 15).unwrap().and_hms_opt(14, 15, 23).unwrap());
        assert!(!item.needs_review);
        assert_eq!(
            item.destination_path.as_ref().unwrap(),
            &destination.path().join("2023/2023-08-15/IMG_0001.heic")
        );
    }
}

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::date_resolution::DateSource;
use crate::db;
use crate::dedup;
use crate::plan::{Plan, PlanItem};
use crate::profiles::ConflictPolicy;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoMove {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UndoManifest {
    pub batch_id: i64,
    pub moves: Vec<UndoMove>,
}

#[derive(Debug, Default)]
pub struct CommitSummary {
    pub batch_id: i64,
    pub moved: usize,
    /// Reorganize-in-place items that were already at their correct
    /// destination — nothing to move.
    pub already_in_place: usize,
    /// Items whose content hash already exists in the index (e.g.
    /// re-importing from the same SD card) — left untouched at the source.
    pub already_imported: usize,
    /// The destination path was already occupied, but by a byte-identical
    /// file — not caught by the index (e.g. it was never imported through
    /// this tool). Left untouched at the source; nothing would be gained by
    /// moving a duplicate.
    pub duplicate_at_destination: usize,
    /// Items skipped due to a genuine naming collision (different content)
    /// under `ConflictPolicy::Skip`.
    pub skipped: usize,
    /// Items manually excluded by the caller (`PlanItem::excluded`) — left
    /// untouched at the source regardless of conflict status. Distinct
    /// from `skipped`, which always implies a real naming collision.
    pub excluded: usize,
}

pub struct CommitOptions<'a> {
    pub conn: &'a Connection,
    pub undo_log_dir: &'a Path,
    /// `"import"` | `"reorganize"`.
    pub kind: &'a str,
    pub profile_id: Option<i64>,
    pub conflict_policy: ConflictPolicy,
}

/// Executes a dry-run `Plan` against disk — the only place in the app that
/// writes to a user's photo directories. Associated files (RAW+JPEG pairs,
/// `.xmp` companions, live-photo `.mov` files — anything sharing a basename
/// stem in the same source folder) move together to the same destination
/// folder as their highest-confidence sibling, even if resolved
/// independently to a different date. Every move is appended to an
/// undo-log JSON manifest before returning.
pub fn commit_plan(plan: &Plan, options: &CommitOptions) -> Result<CommitSummary> {
    fs::create_dir_all(options.undo_log_dir).context("creating undo log directory")?;

    let undo_log_path = options
        .undo_log_dir
        .join(format!("undo-{}.json", Utc::now().timestamp_nanos_opt().unwrap_or_default()));

    let batch_id = db::insert_batch(
        options.conn,
        &db::NewBatch {
            started_at: Utc::now().to_rfc3339(),
            profile_id: options.profile_id,
            kind: options.kind.to_string(),
            undo_log_path: undo_log_path.to_string_lossy().to_string(),
        },
    )?;

    let mut summary = CommitSummary {
        batch_id,
        ..Default::default()
    };
    let mut undo_moves = Vec::new();

    for group in group_by_associated_stem(&plan.items) {
        commit_group(&group, options, batch_id, &mut summary, &mut undo_moves)?;
    }

    let manifest = UndoManifest {
        batch_id,
        moves: undo_moves,
    };
    fs::write(&undo_log_path, serde_json::to_string_pretty(&manifest)?)?;

    Ok(summary)
}

/// Reads a batch's undo manifest and replays every move in reverse. Also
/// removes that batch's rows from the `files` index — otherwise a later
/// re-scan would still see the undone files as "already imported".
pub fn undo_batch(conn: &Connection, batch_id: i64) -> Result<usize> {
    let undo_log_path = db::undo_log_path_for_batch(conn, batch_id)?
        .context("no such batch")?;
    let manifest: UndoManifest = serde_json::from_str(&fs::read_to_string(&undo_log_path)?)?;

    let mut restored = 0;
    for mv in manifest.moves.iter().rev() {
        let from = PathBuf::from(&mv.to);
        let to = PathBuf::from(&mv.from);
        if !from.exists() {
            continue;
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        move_file(&from, &to)?;
        restored += 1;
    }

    conn.execute("DELETE FROM files WHERE batch_id = ?1", params![batch_id])?;

    Ok(restored)
}

struct FileGroup<'a> {
    primary: &'a PlanItem,
    siblings: Vec<&'a PlanItem>,
}

/// Groups items sharing a source parent directory + filename stem
/// (case-insensitive), e.g. `IMG_0001.CR3` / `IMG_0001.JPG` / `IMG_0001.xmp`.
/// The group's primary is whichever member has the highest-confidence
/// resolved date; its destination folder wins for the whole group. Items
/// with no resolvable date at all (no destination) are excluded — nothing
/// to compute a destination from.
fn group_by_associated_stem(items: &[PlanItem]) -> Vec<FileGroup<'_>> {
    let mut by_key: HashMap<(PathBuf, String), Vec<&PlanItem>> = HashMap::new();
    for item in items {
        if item.destination_path.is_none() {
            continue;
        }
        let parent = item.source_path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        let stem = item
            .source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_lowercase();
        by_key.entry((parent, stem)).or_default().push(item);
    }

    by_key
        .into_values()
        .map(|mut members| {
            members.sort_by(|a, b| {
                let confidence_of = |item: &&PlanItem| item.chosen().map(|c| c.confidence).unwrap_or(0.0);
                confidence_of(b)
                    .partial_cmp(&confidence_of(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let primary = members.remove(0);
            FileGroup {
                primary,
                siblings: members,
            }
        })
        .collect()
}

fn commit_group(
    group: &FileGroup,
    options: &CommitOptions,
    batch_id: i64,
    summary: &mut CommitSummary,
    undo_moves: &mut Vec<UndoMove>,
) -> Result<()> {
    let target_folder = group
        .primary
        .destination_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .context("primary item in a commit group has no destination")?;

    let mut members = vec![group.primary];
    members.extend(group.siblings.iter().copied());

    for item in members {
        commit_item(item, &target_folder, options, batch_id, summary, undo_moves)?;
    }

    Ok(())
}

fn commit_item(
    item: &PlanItem,
    target_folder: &Path,
    options: &CommitOptions,
    batch_id: i64,
    summary: &mut CommitSummary,
    undo_moves: &mut Vec<UndoMove>,
) -> Result<()> {
    if item.no_op {
        summary.already_in_place += 1;
        return Ok(());
    }
    if item.already_imported {
        summary.already_imported += 1;
        return Ok(());
    }
    if item.excluded {
        // Caller's explicit "don't import this one" wins regardless of
        // conflict status — checked ahead of any conflict detection so it
        // short-circuits everything below.
        summary.excluded += 1;
        return Ok(());
    }

    let filename = item
        .source_path
        .file_name()
        .context("source path has no filename")?;
    let mut destination = target_folder.join(filename);

    // Computed once, before any move, and reused below for the index row —
    // the file's bytes don't change across a rename/copy, so this stays
    // valid post-move and avoids hashing the file twice.
    let source_hash = dedup::content_hash(&item.source_path).ok();

    if destination.exists() {
        let identical =
            source_hash.is_some() && source_hash.as_deref() == dedup::content_hash(&destination).ok().as_deref();
        if identical {
            // Same bytes already sit at this path — not a conflict, just a
            // duplicate. There's no `Overwrite` policy to fall back on
            // here: identical content is always safe to leave alone,
            // different content (below) is never safe to destroy.
            summary.duplicate_at_destination += 1;
            return Ok(());
        }
        match options.conflict_policy {
            ConflictPolicy::Skip => {
                summary.skipped += 1;
                return Ok(());
            }
            ConflictPolicy::Rename => destination = disambiguate(&destination),
        }
    }

    fs::create_dir_all(target_folder)?;
    move_file(&item.source_path, &destination)?;

    let chosen = item.chosen();
    db::insert_file(
        options.conn,
        &db::NewFileRecord {
            content_hash: source_hash.unwrap_or_default(),
            perceptual_hash: dedup::perceptual_hash(&destination),
            current_path: destination.to_string_lossy().to_string(),
            capture_date: chosen.map(|c| c.date.format("%Y-%m-%dT%H:%M:%S").to_string()),
            date_source: chosen.map(|c| date_source_label(c.source).to_string()),
            date_confidence: chosen.map(|c| c.confidence as f64),
            imported_at: Utc::now().to_rfc3339(),
            batch_id,
        },
    )?;

    undo_moves.push(UndoMove {
        from: item.source_path.to_string_lossy().to_string(),
        to: destination.to_string_lossy().to_string(),
    });
    summary.moved += 1;

    Ok(())
}

/// Matches the `date_source` domain documented in execution-plan.md §5
/// (`'exif' | 'filename' | 'xmp' | 'mtime' | 'manual'`) — fs-created and
/// fs-modified both collapse to `'mtime'` in storage.
fn date_source_label(source: DateSource) -> &'static str {
    match source {
        DateSource::Exif => "exif",
        DateSource::Xmp => "xmp",
        DateSource::Filename => "filename",
        DateSource::FsCreated | DateSource::FsModified => "mtime",
    }
}

fn disambiguate(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let extension = path.extension().and_then(|s| s.to_str());

    let mut counter = 1;
    loop {
        let candidate_name = match extension {
            Some(ext) => format!("{stem} ({counter}).{ext}"),
            None => format!("{stem} ({counter})"),
        };
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_err() {
        fs::copy(from, to).context("copying across filesystems after rename failed")?;
        fs::remove_file(from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::date_resolution::DateCandidate;
    use chrono::NaiveDate;

    fn candidate(y: i32, mo: u32, d: u32, confidence: f32) -> DateCandidate {
        DateCandidate {
            date: NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(0, 0, 0).unwrap(),
            source: DateSource::Filename,
            confidence,
            pattern_name: None,
        }
    }

    fn item(source_path: PathBuf, destination_path: Option<PathBuf>, candidates: Vec<DateCandidate>) -> PlanItem {
        PlanItem {
            source_path,
            candidates,
            destination_path,
            needs_review: false,
            conflict: crate::plan::ConflictKind::None,
            no_op: false,
            already_imported: false,
            excluded: false,
        }
    }

    fn options<'a>(conn: &'a Connection, undo_log_dir: &'a Path) -> CommitOptions<'a> {
        CommitOptions {
            conn,
            undo_log_dir,
            kind: "import",
            profile_id: None,
            conflict_policy: ConflictPolicy::Skip,
        }
    }

    #[test]
    fn moves_a_single_file_and_records_undo_manifest() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let src_path = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&src_path, b"fixture").unwrap();
        let dest_path = destination.path().join("2023/2023-08-15/IMG_20230815_141523.jpg");

        let plan = Plan {
            items: vec![item(src_path.clone(), Some(dest_path.clone()), vec![candidate(2023, 8, 15, 0.85)])],
        };

        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();

        assert_eq!(summary.moved, 1);
        assert!(!src_path.exists());
        assert!(dest_path.exists());
        assert_eq!(db::find_by_content_hash(&conn, &dedup::content_hash(&dest_path).unwrap()).unwrap(), Some(dest_path.to_string_lossy().to_string()));

        let undo_log_path = db::undo_log_path_for_batch(&conn, summary.batch_id).unwrap().unwrap();
        let manifest: UndoManifest = serde_json::from_str(&fs::read_to_string(undo_log_path).unwrap()).unwrap();
        assert_eq!(manifest.moves.len(), 1);
        assert_eq!(manifest.moves[0].to, dest_path.to_string_lossy());
    }

    #[test]
    fn associated_files_move_together_to_primarys_folder() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let raw_path = source.path().join("IMG_0001.CR3");
        let xmp_path = source.path().join("IMG_0001.xmp");
        fs::write(&raw_path, b"raw bytes").unwrap();
        fs::write(&xmp_path, b"xmp bytes").unwrap();

        let primary_dest = destination.path().join("2023/2023-08-15/IMG_0001.CR3");
        // Deliberately resolves to a different date/folder on its own —
        // the sidecar should still follow the primary's folder, not its own.
        let sidecar_own_dest = destination.path().join("2026/2026-08-16/IMG_0001.xmp");

        let plan = Plan {
            items: vec![
                item(raw_path.clone(), Some(primary_dest.clone()), vec![candidate(2023, 8, 15, 0.95)]),
                item(xmp_path.clone(), Some(sidecar_own_dest), vec![candidate(2026, 8, 16, 0.2)]),
            ],
        };

        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();

        assert_eq!(summary.moved, 2);
        assert!(primary_dest.exists());
        let sidecar_final = destination.path().join("2023/2023-08-15/IMG_0001.xmp");
        assert!(sidecar_final.exists(), "sidecar should follow the primary's destination folder");
    }

    #[test]
    fn excluded_primary_still_leaves_sibling_in_its_target_folder() {
        // The primary (highest-confidence sibling) determines the group's
        // target folder before any member's commit_item runs, so excluding
        // the primary itself must not disturb where its sibling lands.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let raw_path = source.path().join("IMG_0001.CR3");
        let xmp_path = source.path().join("IMG_0001.xmp");
        fs::write(&raw_path, b"raw bytes").unwrap();
        fs::write(&xmp_path, b"xmp bytes").unwrap();

        let primary_dest = destination.path().join("2023/2023-08-15/IMG_0001.CR3");
        let sidecar_own_dest = destination.path().join("2026/2026-08-16/IMG_0001.xmp");

        let mut primary = item(raw_path.clone(), Some(primary_dest.clone()), vec![candidate(2023, 8, 15, 0.95)]);
        primary.excluded = true;
        let sibling = item(xmp_path.clone(), Some(sidecar_own_dest), vec![candidate(2026, 8, 16, 0.2)]);

        let plan = Plan {
            items: vec![primary, sibling],
        };

        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();

        assert_eq!(summary.excluded, 1);
        assert_eq!(summary.moved, 1);
        assert!(raw_path.exists(), "excluded primary stays at the source");
        assert!(!primary_dest.exists());
        let sidecar_final = destination.path().join("2023/2023-08-15/IMG_0001.xmp");
        assert!(
            sidecar_final.exists(),
            "sidecar should still land in the primary's computed folder, not its own"
        );
    }

    #[test]
    fn no_op_items_are_not_moved_or_counted_as_moves() {
        let library = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let path = library.path().join("IMG_20230815_141523.jpg");
        fs::write(&path, b"fixture").unwrap();

        let mut noop_item = item(path.clone(), Some(path.clone()), vec![candidate(2023, 8, 15, 0.85)]);
        noop_item.no_op = true;

        let plan = Plan { items: vec![noop_item] };
        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();

        assert_eq!(summary.moved, 0);
        assert_eq!(summary.already_in_place, 1);
        assert!(path.exists());
        assert!(db::find_by_content_hash(&conn, &dedup::content_hash(&path).unwrap()).unwrap().is_none());
    }

    #[test]
    fn already_imported_items_are_left_at_the_source() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let src_path = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&src_path, b"fixture").unwrap();
        let dest_path = destination.path().join("2023/2023-08-15/IMG_20230815_141523.jpg");

        let mut dup_item = item(src_path.clone(), Some(dest_path.clone()), vec![candidate(2023, 8, 15, 0.85)]);
        dup_item.already_imported = true;

        let plan = Plan { items: vec![dup_item] };
        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();

        assert_eq!(summary.moved, 0);
        assert_eq!(summary.already_imported, 1);
        assert!(src_path.exists());
        assert!(!dest_path.exists());
    }

    #[test]
    fn excluded_item_is_left_at_the_source() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let src_path = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&src_path, b"fixture").unwrap();
        let dest_path = destination.path().join("2023/2023-08-15/IMG_20230815_141523.jpg");

        let mut excluded_item = item(src_path.clone(), Some(dest_path.clone()), vec![candidate(2023, 8, 15, 0.85)]);
        excluded_item.excluded = true;

        let plan = Plan { items: vec![excluded_item] };
        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();

        assert_eq!(summary.excluded, 1);
        assert_eq!(summary.moved, 0);
        assert_eq!(summary.skipped, 0);
        assert!(src_path.exists());
        assert!(!dest_path.exists());
    }

    #[test]
    fn excluded_item_with_a_conflict_is_still_just_excluded() {
        // Exclusion short-circuits before conflict detection even runs —
        // it should never fall into the `skipped` (conflict-driven) bucket.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let src_path = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&src_path, b"new bytes").unwrap();
        let dest_folder = destination.path().join("2023/2023-08-15");
        fs::create_dir_all(&dest_folder).unwrap();
        let dest_path = dest_folder.join("IMG_20230815_141523.jpg");
        fs::write(&dest_path, b"already there").unwrap();

        let mut excluded_item = item(src_path.clone(), Some(dest_path.clone()), vec![candidate(2023, 8, 15, 0.85)]);
        excluded_item.excluded = true;

        let plan = Plan { items: vec![excluded_item] };
        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();

        assert_eq!(summary.excluded, 1);
        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.moved, 0);
        assert!(src_path.exists());
        assert_eq!(fs::read_to_string(&dest_path).unwrap(), "already there");
    }

    #[test]
    fn identical_content_at_destination_is_left_alone_regardless_of_policy() {
        // Not caught by `already_imported` (that's an index lookup) — this
        // is the filesystem-only safety net: a byte-identical file already
        // sits at the computed destination (e.g. it was placed there by
        // hand, never indexed). No `Overwrite` policy exists to reach for;
        // this is resolved before any policy is consulted at all.
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let src_path = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&src_path, b"identical bytes").unwrap();
        let dest_folder = destination.path().join("2023/2023-08-15");
        fs::create_dir_all(&dest_folder).unwrap();
        let dest_path = dest_folder.join("IMG_20230815_141523.jpg");
        fs::write(&dest_path, b"identical bytes").unwrap();

        let plan = Plan {
            items: vec![item(src_path.clone(), Some(dest_path.clone()), vec![candidate(2023, 8, 15, 0.85)])],
        };
        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();

        assert_eq!(summary.duplicate_at_destination, 1);
        assert_eq!(summary.moved, 0);
        assert_eq!(summary.skipped, 0);
        assert!(src_path.exists(), "source is left in place, not deleted");
        assert_eq!(fs::read_to_string(&dest_path).unwrap(), "identical bytes");
    }

    #[test]
    fn skip_conflict_policy_leaves_source_file_untouched() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let src_path = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&src_path, b"new bytes").unwrap();
        let dest_folder = destination.path().join("2023/2023-08-15");
        fs::create_dir_all(&dest_folder).unwrap();
        let dest_path = dest_folder.join("IMG_20230815_141523.jpg");
        fs::write(&dest_path, b"already there").unwrap();

        let plan = Plan {
            items: vec![item(src_path.clone(), Some(dest_path.clone()), vec![candidate(2023, 8, 15, 0.85)])],
        };
        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();

        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.moved, 0);
        assert!(src_path.exists());
        assert_eq!(fs::read_to_string(&dest_path).unwrap(), "already there");
    }

    #[test]
    fn rename_conflict_policy_disambiguates_destination() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let src_path = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&src_path, b"new bytes").unwrap();
        let dest_folder = destination.path().join("2023/2023-08-15");
        fs::create_dir_all(&dest_folder).unwrap();
        let dest_path = dest_folder.join("IMG_20230815_141523.jpg");
        fs::write(&dest_path, b"already there").unwrap();

        let plan = Plan {
            items: vec![item(src_path.clone(), Some(dest_path.clone()), vec![candidate(2023, 8, 15, 0.85)])],
        };
        let mut opts = options(&conn, undo_dir.path());
        opts.conflict_policy = ConflictPolicy::Rename;
        let summary = commit_plan(&plan, &opts).unwrap();

        assert_eq!(summary.moved, 1);
        let renamed = dest_folder.join("IMG_20230815_141523 (1).jpg");
        assert!(renamed.exists());
        assert_eq!(fs::read_to_string(&dest_path).unwrap(), "already there");
    }

    #[test]
    fn undo_batch_reverses_moves_and_clears_index_rows() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let undo_dir = tempfile::tempdir().unwrap();
        let conn = db::open_in_memory().unwrap();

        let src_path = source.path().join("IMG_20230815_141523.jpg");
        fs::write(&src_path, b"fixture").unwrap();
        let dest_path = destination.path().join("2023/2023-08-15/IMG_20230815_141523.jpg");

        let plan = Plan {
            items: vec![item(src_path.clone(), Some(dest_path.clone()), vec![candidate(2023, 8, 15, 0.85)])],
        };
        let summary = commit_plan(&plan, &options(&conn, undo_dir.path())).unwrap();
        assert!(dest_path.exists());

        let restored = undo_batch(&conn, summary.batch_id).unwrap();

        assert_eq!(restored, 1);
        assert!(src_path.exists());
        assert!(!dest_path.exists());
        assert!(db::find_by_content_hash(&conn, &dedup::content_hash(&src_path).unwrap()).unwrap().is_none());
    }
}

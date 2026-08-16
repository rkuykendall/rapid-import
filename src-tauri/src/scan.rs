use std::collections::HashMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use walkdir::WalkDir;

use crate::date_resolution::{self, DateInputs};
use crate::plan::{render_template, ConflictKind, Plan, PlanItem, NEEDS_REVIEW_THRESHOLD};

pub struct ScanOptions<'a> {
    pub source_root: &'a Path,
    pub destination_root: &'a Path,
    pub folder_template: &'a str,
    pub now: NaiveDate,
}

/// Recursively walks `source_root` (nested/already-organized trees included
/// — reorganize-in-place needs the walk to not assume a flat layout) and
/// computes a dry-run plan. No files are read except for metadata/bytes
/// needed to resolve a date; nothing on disk is written.
pub fn scan(options: &ScanOptions) -> Plan {
    let mut items: Vec<PlanItem> = WalkDir::new(options.source_root)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| build_plan_item(entry.path(), options))
        .collect();

    flag_conflicts(&mut items);

    Plan { items }
}

fn build_plan_item(source_path: &Path, options: &ScanOptions) -> PlanItem {
    let filename = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    let inputs = DateInputs {
        filename,
        exif_date_time_original: read_exif_date(source_path),
        xmp_date_time_original: None,
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
        options.destination_root.join(rendered_folder).join(filename)
    });

    PlanItem {
        source_path: source_path.to_path_buf(),
        candidates: resolution.candidates,
        destination_path,
        needs_review,
        conflict: ConflictKind::None,
    }
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

/// Flags destinations that collide with an existing file on disk, or with
/// another item resolved in this same plan. SQLite-index cross-referencing
/// (§5) lands once the index exists.
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
        if destination_counts.get(dest).copied().unwrap_or(0) > 1 {
            item.conflict = ConflictKind::DuplicateInPlan;
        } else if dest.exists() {
            item.conflict = ConflictKind::DestinationExists;
        }
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
            folder_template: "{yyyy}/{yyyy}-{mm}-{dd}",
            now: today(),
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
            folder_template: "{yyyy}/{yyyy}-{mm}-{dd}",
            now: today(),
        };
        let plan = scan(&options);

        assert_eq!(find(&plan, "IMG_20230815_141523.jpg").conflict, ConflictKind::DestinationExists);
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
            folder_template: "{yyyy}/{yyyy}-{mm}-{dd}",
            now: today(),
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
            folder_template: "{yyyy}/{yyyy}-{mm}-{dd}",
            now: today(),
        };
        let plan = scan(&options);

        let item = find(&plan, "DSC00001.jpg");
        assert!(item.needs_review);
        assert!(item.chosen().unwrap().confidence < NEEDS_REVIEW_THRESHOLD);
        assert!(item.destination_path.is_some());
    }
}

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::Local;

use rapid_import_core::db;
use rapid_import_core::plan::ConflictKind;
use rapid_import_core::scan::{scan, ScanOptions};

const DEFAULT_FOLDER_TEMPLATE: &str = "{yyyy}/{yyyy}-{mm}-{dd}";

/// Minimal dry-run harness for the scan/plan engine, per execution-plan.md
/// §9 phase 2 ("exposed via a CLI or minimal test harness before the UI
/// exists"). Read-only: prints the plan, writes nothing.
fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let (Some(source), Some(destination)) = (args.next(), args.next()) else {
        eprintln!("usage: scan_cli <source_dir> <destination_dir> [folder_template] [index_db_path]");
        eprintln!("  default folder_template: {DEFAULT_FOLDER_TEMPLATE}");
        eprintln!("  index_db_path: optional SQLite index to cross-check for already-imported files");
        return ExitCode::FAILURE;
    };
    let folder_template = args.next().unwrap_or_else(|| DEFAULT_FOLDER_TEMPLATE.to_string());
    let index_path = args.next();

    let conn = match index_path.as_deref().map(|p| db::open(Path::new(p))).transpose() {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("failed to open index db: {e}");
            return ExitCode::FAILURE;
        }
    };

    let options = ScanOptions {
        source_root: &PathBuf::from(source),
        destination_root: &PathBuf::from(destination),
        folder_template: &folder_template,
        now: Local::now().date_naive(),
        index: conn.as_ref(),
    };

    let mut items = scan(&options).items;
    items.sort_by(|a, b| a.source_path.cmp(&b.source_path));

    for item in &items {
        let (date_str, source_str, confidence_str) = match item.chosen() {
            Some(candidate) => (
                candidate.date.format("%Y-%m-%d %H:%M:%S").to_string(),
                format!("{:?}", candidate.source),
                format!("{:.2}", candidate.confidence),
            ),
            None => ("-".to_string(), "-".to_string(), "-".to_string()),
        };
        let destination = item
            .destination_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unresolved)".to_string());

        let mut flags = Vec::new();
        if item.no_op {
            flags.push("ALREADY ORGANIZED".to_string());
        }
        if item.already_imported {
            flags.push("ALREADY IMPORTED".to_string());
        }
        if item.needs_review {
            flags.push("NEEDS REVIEW".to_string());
        }
        match item.conflict {
            ConflictKind::None => {}
            ConflictKind::DestinationExists => flags.push("CONFLICT: destination exists".to_string()),
            ConflictKind::DuplicateInPlan => flags.push("CONFLICT: duplicate in plan".to_string()),
        }
        let flags_str = if flags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", flags.join(", "))
        };

        println!(
            "{}\n  -> {destination}\n     date: {date_str}  source: {source_str}  confidence: {confidence_str}{flags_str}\n",
            item.source_path.display(),
        );
    }

    let total = items.len();
    let already_organized = items.iter().filter(|i| i.no_op).count();
    let already_imported = items.iter().filter(|i| i.already_imported).count();
    let needs_review = items.iter().filter(|i| i.needs_review).count();
    let conflicts = items.iter().filter(|i| i.conflict != ConflictKind::None).count();
    println!(
        "{total} file(s) scanned, {already_organized} already organized, {already_imported} already imported, \
         {needs_review} needing review, {conflicts} conflict(s). Dry run only — nothing written."
    );

    ExitCode::SUCCESS
}

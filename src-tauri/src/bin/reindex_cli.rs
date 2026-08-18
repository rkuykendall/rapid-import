use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use rapid_import_core::db;
use rapid_import_core::index_library::{index_library, IndexOptions};

/// Standalone harness for building/refreshing the SQLite index from an
/// existing destination library, without the UI — mirrors `scan_cli`'s
/// shape. Useful for a one-time backfill of a large existing library, or
/// for debugging the indexing engine in isolation.
///
/// Always writes to the same `library.sqlite` the app itself uses
/// (`db::default_db_path`) — a reindex here is visible to the app, and vice
/// versa.
fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(destination_root) = args.next() else {
        eprintln!("usage: reindex_cli <destination_root> [scope_subfolder]");
        eprintln!("  scope_subfolder: optional, relative to destination_root — reindex just that slice");
        return ExitCode::FAILURE;
    };
    let scope_subfolder = args.next();

    let destination_root = PathBuf::from(destination_root);
    let scope_root = scope_subfolder.as_ref().map(|s| destination_root.join(s));

    let options = IndexOptions {
        destination_root: &destination_root,
        scope_root: scope_root.as_deref(),
    };

    let db_path = match db::default_db_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("failed to resolve index db path: {e}");
            return ExitCode::FAILURE;
        }
    };

    match index_library(&db_path, &options) {
        Ok(summary) => {
            println!(
                "unchanged: {}  moved: {}  content changed: {}  new: {}  removed: {}  (batch {})",
                summary.unchanged, summary.moved, summary.content_changed, summary.new, summary.removed, summary.batch_id
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("reindex failed: {e}");
            ExitCode::FAILURE
        }
    }
}

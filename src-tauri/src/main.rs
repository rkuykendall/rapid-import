#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rapid_import_core::{db, dedup, index_library, plan, plan::Plan, profiles, scan};
use tauri::{Emitter, Manager};

/// `scan_source`'s response: the plan itself, plus every exact-duplicate
/// pairing `dedup::find_duplicates` found (against the rest of the scan and
/// against the persistent index). Bundled into the one round-trip rather
/// than a separate command/invoke, since the frontend always wants both
/// together right after a scan.
#[derive(serde::Serialize)]
struct ScanResult {
    plan: Plan,
    duplicates: Vec<dedup::DuplicateGroup>,
}

/// Emit a `scan-progress` event at most this often, to avoid flooding the
/// frontend with an IPC message per file on a large library.
const SCAN_PROGRESS_EVERY: usize = 10;

/// Same idea as `SCAN_PROGRESS_EVERY`, but a much larger stride — a library
/// reindex walks far more files than a single source scan.
const INDEX_PROGRESS_EVERY: usize = 500;

/// Profiles are keyed by destination path — pick a destination (library),
/// recall the source/template last used for *that* destination.
/// `date_fallback_order`/`conflict_policy` aren't exposed in the UI yet, so
/// a sensible default is hardcoded here rather than left for the user to
/// configure.
const DEFAULT_DATE_FALLBACK_ORDER: [&str; 4] = ["exif", "filename", "xmp", "mtime"];

/// A profile's display name is just its destination's last path
/// component (e.g. `/Users/x/Pictures` -> `"Pictures"`) — the sidebar
/// needs something recognizable at a glance, not a user-chosen name (no
/// UI for that yet).
fn profile_name_for(destination_root: &str) -> String {
    Path::new(destination_root)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(destination_root)
        .to_string()
}

struct AppState {
    db: Mutex<rusqlite::Connection>,
    /// `index_library` opens its own short-lived connections (a full-library
    /// reindex can run far longer than any other command, and shouldn't
    /// hold the shared `db` lock the whole time) — it needs the path, not
    /// the live connection.
    db_path: PathBuf,
}

/// `async` + `spawn_blocking` deliberately, not a plain `fn` — Tauri runs
/// non-async commands on the main thread by default, which would freeze
/// the whole UI (window becomes unresponsive, OS shows the spinning
/// pinwheel on macOS) for the entire scan. Every command that does real
/// I/O should follow this same pattern.
///
/// Emits `scan-progress` (payload: running file count) every
/// `SCAN_PROGRESS_EVERY` files so the UI can show a live count — there's
/// no separate `scan-complete` event; the resolved `Plan` this command
/// returns *is* the completion signal.
#[tauri::command]
async fn scan_source(
    app_handle: tauri::AppHandle,
    source_root: String,
    destination_root: String,
    folder_template: String,
) -> Result<ScanResult, String> {
    let progress_handle = app_handle.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let options = scan::ScanOptions {
            source_root: Path::new(&source_root),
            destination_root: Path::new(&destination_root),
            folder_template: &folder_template,
            now: chrono::Local::now().date_naive(),
            index: Some(&conn),
        };
        let plan = scan::scan_with_progress(&options, |count| {
            if count % SCAN_PROGRESS_EVERY == 0 {
                let _ = progress_handle.emit("scan-progress", count);
            }
        });
        let duplicates = dedup::find_duplicates(&conn, &plan.items);
        Ok(ScanResult { plan, duplicates })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Renders `folder_template` against "now" — lets the UI show a live
/// preview instead of us validating chrono's strftime syntax ourselves.
/// Cheap (pure string formatting, no I/O), but still `async` per the
/// project-wide command convention (see execution-plan.md §7).
#[tauri::command]
async fn preview_folder_template(folder_template: String) -> String {
    plan::render_template(&folder_template, chrono::Local::now().naive_local())
}

/// Looks up prior settings for a destination the user just picked —
/// `None` means this destination has never been used before.
#[tauri::command]
async fn load_profile_for_destination(
    app_handle: tauri::AppHandle,
    destination_root: String,
) -> Result<Option<profiles::Profile>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        profiles::find_profile_by_destination_root(&conn, &destination_root).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Creates a profile for this destination on first use, updates it
/// thereafter. Called once, when the user actually commits to a scan —
/// not on every keystroke while they're still typing.
#[tauri::command]
async fn save_profile_for_destination(
    app_handle: tauri::AppHandle,
    source_root: String,
    destination_root: String,
    folder_template: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let existing = profiles::find_profile_by_destination_root(&conn, &destination_root).map_err(|e| e.to_string())?;
        let new_profile = profiles::NewProfile {
            name: profile_name_for(&destination_root),
            folder_template,
            source_root: Some(source_root),
            destination_root: Some(destination_root),
            date_fallback_order: DEFAULT_DATE_FALLBACK_ORDER.iter().map(|s| s.to_string()).collect(),
            conflict_policy: profiles::ConflictPolicy::Skip,
        };
        match existing {
            Some(profile) => profiles::update_profile(&conn, profile.id, &new_profile),
            None => profiles::save_profile(&conn, &new_profile).map(|_| ()),
        }
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Builds/refreshes the SQLite index from what's actually on disk under
/// `destination_root` (or, when `scope_root` is given, just that subfolder)
/// — the mechanism for making `already_imported`/`DuplicatePolicy::Skip`
/// recognize content that was never imported *through* this app (an
/// existing library, or one imported by another tool). Explicit and
/// user-triggered only — a plain `scan_source` never re-walks the
/// destination itself, which is what keeps every import fast regardless of
/// how large the destination has grown.
///
/// Opens its own connections via `state.db_path` rather than locking
/// `state.db` for the whole run — a full-library backfill can take a long
/// time, and shouldn't block `scan_source`/`list_profiles` for its
/// duration.
#[tauri::command]
async fn refresh_library_index(
    app_handle: tauri::AppHandle,
    destination_root: String,
    scope_root: Option<String>,
) -> Result<index_library::IndexSummary, String> {
    let progress_handle = app_handle.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let options = index_library::IndexOptions {
            destination_root: Path::new(&destination_root),
            scope_root: scope_root.as_deref().map(Path::new),
        };
        index_library::index_library_with_progress(&state.db_path, &options, |count| {
            if count % INDEX_PROGRESS_EVERY == 0 {
                let _ = progress_handle.emit("index-progress", count);
            }
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Lists every saved profile (one per destination ever used) for the
/// sidebar — newest-created last, since `profiles::load_profiles` orders
/// by `id`.
#[tauri::command]
async fn list_profiles(app_handle: tauri::AppHandle) -> Result<Vec<profiles::Profile>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        profiles::load_profiles(&conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let db_path = app_data_dir.join("library.sqlite");
            let conn = db::open(&db_path)?;
            app.manage(AppState { db: Mutex::new(conn), db_path });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan_source,
            preview_folder_template,
            load_profile_for_destination,
            save_profile_for_destination,
            list_profiles,
            refresh_library_index
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

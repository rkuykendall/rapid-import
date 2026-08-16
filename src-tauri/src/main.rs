#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;
use std::sync::Mutex;

use rapid_import_core::{db, plan, plan::Plan, profiles, scan};
use tauri::{Emitter, Manager};

/// Emit a `scan-progress` event at most this often, to avoid flooding the
/// frontend with an IPC message per file on a large library.
const SCAN_PROGRESS_EVERY: usize = 10;

/// Profiles are keyed by destination path — pick a destination (library),
/// recall the source/template last used for *that* destination. `name` and
/// `date_fallback_order`/`conflict_policy` aren't exposed in the UI yet, so
/// sensible defaults are hardcoded here rather than left for the user to
/// configure.
const DEFAULT_PROFILE_NAME: &str = "Default";
const DEFAULT_DATE_FALLBACK_ORDER: [&str; 4] = ["exif", "filename", "xmp", "mtime"];

struct AppState {
    db: Mutex<rusqlite::Connection>,
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
) -> Result<Plan, String> {
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
        Ok(scan::scan_with_progress(&options, |count| {
            if count % SCAN_PROGRESS_EVERY == 0 {
                let _ = progress_handle.emit("scan-progress", count);
            }
        }))
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
            name: DEFAULT_PROFILE_NAME.to_string(),
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let conn = db::open(&app_data_dir.join("library.sqlite"))?;
            app.manage(AppState { db: Mutex::new(conn) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            scan_source,
            preview_folder_template,
            load_profile_for_destination,
            save_profile_for_destination
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

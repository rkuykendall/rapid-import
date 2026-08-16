#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;
use std::sync::Mutex;

use rapid_import_core::{db, plan, plan::Plan, profiles, scan};
use tauri::Manager;

/// The single auto-persisted profile backing "remember what I typed" —
/// the same `profiles` table a future named-profile-switcher UI would use,
/// just with one implicit row for now. `date_fallback_order`/
/// `conflict_policy` aren't exposed in the UI yet, so sensible defaults
/// are hardcoded here rather than left for the user to configure.
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
#[tauri::command]
async fn scan_source(
    app_handle: tauri::AppHandle,
    source_root: String,
    destination_root: String,
    folder_template: String,
) -> Result<Plan, String> {
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
        Ok(scan::scan(&options))
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

/// Whichever profile was created first stands in for "the" profile until
/// a real profile-switcher exists. `None` on a fresh install (nothing
/// saved yet).
#[tauri::command]
async fn load_default_profile(app_handle: tauri::AppHandle) -> Result<Option<profiles::Profile>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let mut all = profiles::load_profiles(&conn).map_err(|e| e.to_string())?;
        Ok(if all.is_empty() { None } else { Some(all.remove(0)) })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Creates the default profile on first use, updates it thereafter.
#[tauri::command]
async fn save_default_profile(
    app_handle: tauri::AppHandle,
    source_root: String,
    destination_root: String,
    folder_template: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle.state::<AppState>();
        let conn = state.db.lock().map_err(|e| e.to_string())?;
        let existing = profiles::load_profiles(&conn).map_err(|e| e.to_string())?;
        let new_profile = profiles::NewProfile {
            name: DEFAULT_PROFILE_NAME.to_string(),
            folder_template,
            source_root: Some(source_root),
            destination_root: Some(destination_root),
            date_fallback_order: DEFAULT_DATE_FALLBACK_ORDER.iter().map(|s| s.to_string()).collect(),
            conflict_policy: profiles::ConflictPolicy::Skip,
        };
        match existing.first() {
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
            load_default_profile,
            save_default_profile
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

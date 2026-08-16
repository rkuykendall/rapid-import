#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;
use std::sync::Mutex;

use rapid_import_core::{db, plan::Plan, scan};
use tauri::Manager;

struct AppState {
    db: Mutex<rusqlite::Connection>,
}

#[tauri::command]
fn scan_source(
    state: tauri::State<AppState>,
    source_root: String,
    destination_root: String,
    folder_template: String,
) -> Result<Plan, String> {
    let conn = state.db.lock().map_err(|e| e.to_string())?;
    let options = scan::ScanOptions {
        source_root: Path::new(&source_root),
        destination_root: Path::new(&destination_root),
        folder_template: &folder_template,
        now: chrono::Local::now().date_naive(),
        index: Some(&conn),
    };
    Ok(scan::scan(&options))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let conn = db::open(&app_data_dir.join("library.sqlite"))?;
            app.manage(AppState { db: Mutex::new(conn) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![scan_source])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

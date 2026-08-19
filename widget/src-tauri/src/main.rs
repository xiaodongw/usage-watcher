// Thin by design: everything lives in the library so the mobile targets, where
// the platform owns `main` and calls `tauri::mobile_entry_point` instead, run
// exactly the same code.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    usage_watcher_lib::run()
}

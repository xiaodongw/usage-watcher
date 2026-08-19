//! The panel is the product; this shell exists only to host it.
//!
//! All the data lives in `uwd`, which the webview reaches over HTTP/SSE — so
//! this binary holds no credentials, does no polling, and stays the same no
//! matter how many providers are added. That split is what lets the UI run on
//! Windows while the credentials and vendor CLIs stay inside WSL.
//!
//! Built as a library with a thin `main.rs` in front of it because that is what
//! the mobile targets require: on Android and iOS the platform owns `main`, and
//! `tauri::mobile_entry_point` is what gets called instead.

#[cfg(desktop)]
mod tray;

// Only the desktop paths reach for `Manager` (to look up the panel window and
// the tray's shared state), so importing it unconditionally warns on mobile.
#[cfg(desktop)]
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    #[cfg(desktop)]
    let builder = builder
        // Must be registered before anything else, so a second launch is
        // rejected before it has built a window or a second tray icon.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // Launching it again is how people ask for the panel when they have
            // forgotten it is already running.
            if let Some(w) = tray::window(app) {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        // The only command, and it exists to update the tray — so on mobile,
        // where there is no tray, there is nothing to register.
        .invoke_handler(tauri::generate_handler![tray::set_readout]);

    builder
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // Menu-bar app: no Dock icon, no entry in the app switcher, and
            // closing the panel does not quit. Set here as well as in
            // Info.plist because `tauri dev` does not bundle, so the plist has
            // no effect until you ship.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            #[cfg(desktop)]
            tray::build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing the panel must not quit the app — the tray icon is the
            // real entry point, and a widget that dies when dismissed would
            // have to be relaunched from the Start menu every time.
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }

            // A tray popover dismisses when you click away from it. Recorded as
            // well as done, so the click that caused it is not then read as a
            // request to open it again.
            #[cfg(desktop)]
            tauri::WindowEvent::Focused(false) => {
                if window.label() == "panel" {
                    let _ = window.hide();
                    window.app_handle().state::<tray::Panel>().note_auto_hide();
                }
            }

            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running Usage Watcher");
}

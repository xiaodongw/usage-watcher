//! The panel is the product; this shell exists only to host it.
//!
//! All the data lives in `uwd`, which the webview reaches over HTTP/SSE — so
//! the UI holds no credentials, does no polling, and stays the same no matter
//! how many providers are added. That split is what lets the UI run on Windows
//! while the credentials and vendor CLIs stay inside WSL.
//!
//! On the desktop the daemon is started *in this process* rather than left to
//! the user, so the whole app is one file to unzip and double-click. It is the
//! same library `uwd` runs, reached the same way over loopback, and pointing
//! the panel at a daemon on another machine still works — see [`daemon`].
//!
//! Built as a library with a thin `main.rs` in front of it because that is what
//! the mobile targets require: on Android and iOS the platform owns `main`, and
//! `tauri::mobile_entry_point` is what gets called instead.

#[cfg(desktop)]
mod daemon;
#[cfg(desktop)]
mod tray;

// Only the desktop paths reach for `Manager` (to look up the panel window and
// the tray's shared state), so importing it unconditionally warns on mobile.
#[cfg(desktop)]
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The embedded daemon logs which port it took, whether it found an
    // existing one, and why a poll failed. Without a subscriber all of that
    // goes nowhere, and "the panel says it cannot reach uwd" becomes
    // undiagnosable. Quiet by default; `UWD_LOG=debug` opens it up.
    #[cfg(desktop)]
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("UWD_LOG")
                .unwrap_or_else(|_| "uwd=info,uw_core=warn,usage_watcher_lib=info".into()),
        )
        .try_init();

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
        // All desktop-only: two of them drive the tray, and the third answers
        // "which daemon did you start?". On mobile there is no tray and no
        // embedded daemon, so there is nothing to register.
        .invoke_handler(tauri::generate_handler![
            tray::set_readout,
            tray::set_panel_mode,
            daemon::daemon_url
        ]);

    builder
        .plugin(tauri_plugin_notification::init())
        // Every platform: the consent page and the "get a key here" links both
        // have to leave the webview, which has no session with the provider.
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Menu-bar app: no Dock icon, no entry in the app switcher, and
            // closing the panel does not quit. Set here as well as in
            // Info.plist because `tauri dev` does not bundle, so the plist has
            // no effect until you ship.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            #[cfg(desktop)]
            tray::build(app)?;

            // Blocking: the webview asks for the daemon's address before it
            // paints, and answering "not yet" would mean a first render
            // pointed at nothing. Binding a socket and reading a config file
            // is milliseconds.
            #[cfg(desktop)]
            tauri::async_runtime::block_on(daemon::ensure_running(app.handle()));

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
                let panel = window.app_handle().state::<tray::Panel>();
                // Not while the config screens are up: a browser sign-in means
                // clicking away to another window, and dismissing at that
                // moment would hide the field the user has to come back to.
                if window.label() == "panel" && panel.auto_hides() {
                    let _ = window.hide();
                    panel.note_auto_hide();
                }
            }

            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running Usage Watcher");
}

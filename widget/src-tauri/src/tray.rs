//! The tray icon, its menu, and where the panel appears when you click it.
//!
//! Desktop only. Linux is compiled but not really served here: the tray rect is
//! documented as always `None` there, and GNOME dropped the system tray years
//! ago — Linux gets `gnome-extension/` instead, which is a first-class panel
//! indicator rather than a webview pretending to be one.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, PhysicalPosition, Rect, WebviewWindow};

/// Gap between the tray icon and the panel edge, in physical pixels.
const MARGIN: i32 = 8;

/// How recently an auto-hide has to have happened for a tray click to be
/// treated as the cause of it.
///
/// Clicking the tray icon while the panel is open blurs the panel first, which
/// hides it, and only then delivers the click — which would toggle it straight
/// back on. Every tray popover has this bug once. The window is generous
/// because the two events are not ordered by anything we control.
const HIDE_DEBOUNCE: Duration = Duration::from_millis(300);

/// Shared between the window-event handler and the tray click handler.
#[derive(Default)]
pub struct Panel {
    last_auto_hide: Mutex<Option<Instant>>,
}

impl Panel {
    /// Record that the panel hid itself because it lost focus.
    pub fn note_auto_hide(&self) {
        if let Ok(mut g) = self.last_auto_hide.lock() {
            *g = Some(Instant::now());
        }
    }

    /// True when an auto-hide just happened — meaning this click already had
    /// its effect and must not be acted on again.
    fn consume_recent_auto_hide(&self) -> bool {
        let Ok(mut g) = self.last_auto_hide.lock() else {
            return false;
        };
        match g.take() {
            Some(at) if at.elapsed() < HIDE_DEBOUNCE => true,
            _ => false,
        }
    }
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("panel")
}

/// Build the tray icon and register it as managed state.
///
/// The [`TrayIcon`] is managed rather than looked up by id later, so the
/// command that updates the readout cannot fail to find it.
pub fn build(app: &App) -> tauri::Result<()> {
    let handle = app.handle();

    let show = MenuItem::with_id(app, "show", "Show panel", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let tray = TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        // macOS renders a template icon in the menu bar's own colour, so it
        // stays legible in both light and dark. Ignored elsewhere.
        .icon_as_template(true)
        .tooltip("Usage Watcher")
        .menu(&menu)
        // The menu belongs to the right button. A left click should open the
        // panel, which is the entire point of the icon.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = window(app) {
                    show_panel(&w);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            else {
                return;
            };

            let app = tray.app_handle();
            if app.state::<Panel>().consume_recent_auto_hide() {
                return;
            }
            let Some(w) = window(app) else { return };

            if w.is_visible().unwrap_or(false) {
                let _ = w.hide();
            } else {
                // Position before showing, so it never appears at the old spot
                // and jumps.
                let _ = place_at_tray(&w, rect);
                show_panel(&w);
            }
        })
        .build(app)?;

    handle.manage(tray);
    handle.manage(Panel::default());
    Ok(())
}

fn show_panel(w: &WebviewWindow) {
    let _ = w.show();
    let _ = w.set_focus();
}

/// Put the panel next to the tray icon, on the correct side of the screen.
///
/// Which side is not a platform constant: the taskbar is usually at the bottom
/// on Windows and the menu bar always at the top on macOS, but either can be
/// moved. Deciding from the icon's own position handles every arrangement,
/// including a taskbar docked left or right.
fn place_at_tray(w: &WebviewWindow, rect: Rect) -> tauri::Result<()> {
    let scale = w.scale_factor().unwrap_or(1.0);
    let icon_pos = rect.position.to_physical::<i32>(scale);
    let icon_size = rect.size.to_physical::<i32>(scale);
    let panel = w.outer_size()?;
    let (pw, ph) = (panel.width as i32, panel.height as i32);

    // Linux reports no rect at all, and a zero-sized one is not something to
    // compute against — leave the window wherever it was.
    if icon_size.width == 0 && icon_size.height == 0 {
        return Ok(());
    }

    // `None` if the window is not on any monitor yet, which is where it starts:
    // it is created hidden and has never been placed. Every use below is
    // therefore optional rather than defaulted — a sentinel "screen" made of
    // i32 extremes clamps the panel to a coordinate far off the real display,
    // which looks exactly like the window failing to open.
    let monitor = w.current_monitor()?.or(w.primary_monitor()?);
    let bounds = monitor.as_ref().map(|m| {
        let p = m.position();
        let s = m.size();
        (p.x, p.y, s.width as i32, s.height as i32)
    });

    let icon_cx = icon_pos.x + icon_size.width / 2;
    let icon_cy = icon_pos.y + icon_size.height / 2;

    // Centred under the icon, then pulled back inside the screen if that would
    // hang it off an edge — which it does for any icon near a corner.
    let mut x = icon_cx - pw / 2;
    if let Some((mx, _, mw, _)) = bounds {
        let (min_x, max_x) = (mx + MARGIN, mx + mw - pw - MARGIN);
        if max_x >= min_x {
            x = x.clamp(min_x, max_x);
        }
    }

    // Below a tray in the top half of the screen, above one in the bottom half.
    // With no monitor to compare against, below is the safer guess: it is what
    // a top menu bar needs, and a window pushed down is recoverable where one
    // pushed up past y=0 may not be.
    let below = bounds.is_none_or(|(_, my, _, mh)| icon_cy < my + mh / 2);
    let y = if below {
        icon_pos.y + icon_size.height + MARGIN
    } else {
        icon_pos.y - ph - MARGIN
    };

    w.set_position(PhysicalPosition::new(x, y))
}

/// Update what the tray shows at a glance.
///
/// `title` is the short figure macOS puts beside the menu-bar icon; `tooltip`
/// is the longer line Windows shows on hover. Each platform silently ignores
/// the one it does not support, so both are always sent and both errors are
/// discarded — a tray that cannot show a number is not a reason to fail a poll.
//
// The runtime is spelled out rather than left to the `Wry` default, because a
// mismatch between what is `manage`d and what is asked for here is not a
// compile error — it is a panic at first click, when the state lookup misses.
#[tauri::command]
pub fn set_readout(tray: tauri::State<'_, TrayIcon<tauri::Wry>>, title: String, tooltip: String) {
    let _ = tray.set_title(if title.is_empty() { None } else { Some(&title) });
    let _ = tray.set_tooltip(Some(&tooltip));
}

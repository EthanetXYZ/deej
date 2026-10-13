//! The main window's life: created when needed, destroyed when closed (deej keeps running in the tray, and the
//! WebView's memory is released; ARCHITECTURE.md W14/R8), and reopened where it was.

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::AppState;
use crate::prefs::WindowState;

pub const MAIN: &str = "main";

const DEFAULT_SIZE: (f64, f64) = (960.0, 640.0);
const MIN_SIZE: (f64, f64) = (720.0, 480.0);

/// How much of a restored window must be on some monitor, in physical pixels; otherwise it opens centred.
const MIN_VISIBLE: (i32, i32) = (160, 80);

/// Brings the window to the front, creating it if it was closed.
pub fn show(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    match create(app) {
        Ok(window) => {
            let _ = window.show();
            let _ = window.set_focus();
        }
        Err(err) => tracing::error!(error = %err, "Couldn't open the window"),
    }
}

fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::default())
        .title("deej")
        .inner_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1)
        .min_inner_size(MIN_SIZE.0, MIN_SIZE.1)
        .center()
        .visible(false)
        .build()?;

    let saved = app.state::<AppState>().prefs.load().window;
    if let Some(state) = saved.filter(|s| fits_a_monitor(&window, s)) {
        let _ = window.set_size(PhysicalSize::new(state.width, state.height));
        let _ = window.set_position(PhysicalPosition::new(state.x, state.y));
        if state.maximized {
            let _ = window.maximize();
        }
    }

    tracing::info!("Window opened");
    Ok(window)
}

/// Whether enough of the saved window would be on a monitor that's connected now (monitors change: a laptop
/// undocked, a screen switched off).
fn fits_a_monitor(window: &WebviewWindow, state: &WindowState) -> bool {
    let Ok(monitors) = window.available_monitors() else { return false };
    let monitors: Vec<_> = monitors
        .iter()
        .map(|m| {
            let (p, s) = (m.position(), m.size());
            (p.x, p.y, s.width as i32, s.height as i32)
        })
        .collect();
    visible_on_any(&monitors, state)
}

/// `monitors` as (x, y, width, height).
pub fn visible_on_any(monitors: &[(i32, i32, i32, i32)], state: &WindowState) -> bool {
    let (w, h) = (state.width as i32, state.height as i32);
    monitors.iter().any(|&(mx, my, mw, mh)| {
        let overlap_x = (state.x + w).min(mx + mw) - state.x.max(mx);
        let overlap_y = (state.y + h).min(my + mh) - state.y.max(my);
        overlap_x >= MIN_VISIBLE.0 && overlap_y >= MIN_VISIBLE.1
    })
}

/// Remembers where the window is, for the next time it opens. A maximised window keeps its previous normal
/// size and position; a minimised one isn't saved (Windows reports it at -32000).
pub fn save_state(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN) else { return };
    if window.is_minimized().unwrap_or(false) {
        return;
    }
    let maximized = window.is_maximized().unwrap_or(false);
    let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) else { return };

    let result = app.state::<AppState>().prefs.update(|prefs| {
        prefs.window = Some(match (maximized, prefs.window) {
            (true, Some(previous)) => WindowState { maximized: true, ..previous },
            _ => WindowState { x: position.x, y: position.y, width: size.width, height: size.height, maximized },
        });
    });
    if let Err(err) = result {
        tracing::warn!(error = %err, "Couldn't save the window position");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_windows_must_be_mostly_on_screen() {
        let monitors = [(0, 0, 1920, 1080), (-1920, 0, 1920, 1080)];
        let at = |x, y| WindowState { x, y, width: 960, height: 640, maximized: false };

        assert!(visible_on_any(&monitors, &at(100, 100)));
        assert!(visible_on_any(&monitors, &at(-1500, 200)), "on the left monitor");
        assert!(visible_on_any(&monitors, &at(1700, 900)), "partly off the edge, enough still visible");
        assert!(!visible_on_any(&monitors, &at(1900, 100)), "only 20 px visible");
        assert!(!visible_on_any(&monitors, &at(-4000, 100)), "on a monitor that's gone");
        assert!(!visible_on_any(&[], &at(0, 0)));
    }
}

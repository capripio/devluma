//! Closing and quitting Devluma, and its notification-area (tray) icon.
//!
//! The window's ✕ follows the user's choice (`preferences::CloseAction`): ask, hide to the
//! notification area with services still running, or quit. Quitting stops every service
//! Devluma owns while the window shows the progress, then exits (ADR 0006's default). If the
//! UI can't take part (recovery screen, page not loaded, database unreadable), the window
//! closes as before and the services stop on exit, so nobody is stuck with a window that
//! won't close.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime, Window, WindowEvent};

use crate::persistence::services;
use crate::services::preferences::{self, CloseAction};
use crate::state::AppState;

/// Asks the UI to show the close choice.
pub const CLOSE_REQUESTED: &str = "app://close-requested";
/// Asks the UI to show the quit progress and start quitting.
pub const QUIT_REQUESTED: &str = "app://quit-requested";
const MAIN: &str = "main";

#[derive(Debug, Default)]
pub struct Lifecycle {
    /// The UI's close handler is mounted, so it can ask and show quit progress.
    ui_ready: AtomicBool,
    /// Quitting has begun: further close requests go straight through.
    quitting: AtomicBool,
    /// "Quit anyway": exit without waiting for services; they're re-adopted next launch.
    leave_services: AtomicBool,
}

impl Lifecycle {
    pub fn set_ui_ready(&self) {
        self.ui_ready.store(true, Ordering::SeqCst);
    }

    fn ui_ready(&self) -> bool {
        self.ui_ready.load(Ordering::SeqCst)
    }

    fn quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }
}

/// What the ✕ button does, given the saved choice. `None` means close normally.
pub fn close_decision(lc: &Lifecycle, action: Option<CloseAction>) -> Option<CloseAction> {
    if lc.quitting() || !lc.ui_ready() {
        return None;
    }
    action
}

/// `on_window_event` hook: intercepts the main window's ✕.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if window.label() != MAIN {
        return;
    }
    let app = window.app_handle();
    let saved = app
        .state::<AppState>()
        .db()
        .ok()
        .and_then(|db| db.read(preferences::load).ok())
        .map(|p| p.close_action);
    let Some(action) = close_decision(&app.state::<Lifecycle>(), saved) else {
        return;
    };
    api.prevent_close();
    match action {
        CloseAction::Tray => {
            let _ = window.hide();
        }
        CloseAction::Ask => {
            let _ = window.emit(CLOSE_REQUESTED, ());
        }
        CloseAction::Quit => {
            let _ = window.emit(QUIT_REQUESTED, ());
        }
    }
}

/// Stops everything Devluma runs: tunnels and scheduled runs always, services unless the
/// user chose "Quit anyway".
pub fn stop_everything<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    if let Some(t) = &state.tunnels {
        t.stop_all();
    }
    if let Some(s) = &state.scheduler {
        s.stop_all();
    }
    if app
        .state::<Lifecycle>()
        .leave_services
        .load(Ordering::SeqCst)
    {
        return;
    }
    if let Some(sup) = &state.supervisor {
        sup.stop_all_blocking();
    }
}

/// Quits: stops every service (the UI shows each one stopping), then exits. Returns at once.
pub fn begin_quit<R: Runtime>(app: &AppHandle<R>) {
    if app
        .state::<Lifecycle>()
        .quitting
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        stop_everything(&app);
        app.exit(0);
    });
}

/// "Quit anyway": exits now and leaves services that are still running as they are.
pub fn quit_now<R: Runtime>(app: &AppHandle<R>) {
    let lc = app.state::<Lifecycle>();
    lc.leave_services.store(true, Ordering::SeqCst);
    lc.quitting.store(true, Ordering::SeqCst);
    app.exit(0);
}

pub fn hide_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.hide();
    }
}

pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// Quit from the tray: through the window (so the progress is visible) when it can show it.
fn request_quit<R: Runtime>(app: &AppHandle<R>) {
    if app.state::<Lifecycle>().ui_ready() {
        show_main(app);
        let _ = app.emit_to(MAIN, QUIT_REQUESTED, ());
    } else {
        begin_quit(app);
    }
}

/// The notification-area icon: click to open; menu with Open, Start all, Stop all, Quit.
pub fn create_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "open", "Open Devluma", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "start_all", "Start all services", true, None::<&str>)?,
            &MenuItem::with_id(app, "stop_all", "Stop all services", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit Devluma", true, None::<&str>)?,
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id("devluma")
        .tooltip("Devluma")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "start_all" => start_all_services(app),
            "stop_all" => stop_all_services(app),
            "quit" => request_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Every service that isn't running, skipping any whose port another program holds.
fn start_all_services<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    let (Ok(db), Ok(sup)) = (state.db(), state.supervisor()) else {
        return;
    };
    std::thread::spawn(move || {
        if let Ok(all) = db.read(services::list) {
            crate::services::projects::start_each(&sup, all);
        }
    });
}

fn stop_all_services<R: Runtime>(app: &AppHandle<R>) {
    let Ok(sup) = app.state::<AppState>().supervisor() else {
        return;
    };
    std::thread::spawn(move || sup.stop_all_blocking());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_closes_normally_unless_the_ui_can_take_part() {
        let lc = Lifecycle::default();
        // UI not mounted (recovery screen, still loading): close as before.
        assert_eq!(close_decision(&lc, Some(CloseAction::Ask)), None);
        lc.set_ui_ready();
        assert_eq!(
            close_decision(&lc, Some(CloseAction::Ask)),
            Some(CloseAction::Ask)
        );
        assert_eq!(
            close_decision(&lc, Some(CloseAction::Tray)),
            Some(CloseAction::Tray)
        );
        // Database unreadable: close as before.
        assert_eq!(close_decision(&lc, None), None);
        // While quitting, the final close goes through.
        lc.quitting.store(true, Ordering::SeqCst);
        assert_eq!(close_decision(&lc, Some(CloseAction::Quit)), None);
    }
}

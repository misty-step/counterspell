// Counterspell Desktop — a persistent control window over the headless
// enforcement daemon. This backend is an OBSERVER + CONTROLLER only: it reads
// the same state `watch --arm` reads and writes only the two stable control
// surfaces (the global disarm marker and per-session config targets) through
// the `counterspell` crate's library API. It NEVER invokes `launchctl` or
// loads/unloads daemons — closing the window (or quitting the app) leaves
// enforcement running, which is the whole design.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use counterspell::api::{self, ActivationEntry, Health, RebindOutcome, StatusSnapshot, Verdict};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

type CmdResult<T> = Result<T, String>;

fn to_cmd<T>(result: anyhow::Result<T>) -> CmdResult<T> {
    result.map_err(|error| format!("{error:#}"))
}

#[tauri::command]
fn get_status() -> CmdResult<StatusSnapshot> {
    to_cmd(api::status_snapshot())
}

#[tauri::command]
fn get_doctor() -> CmdResult<Health> {
    to_cmd(api::health_snapshot())
}

#[tauri::command]
fn get_activation_log(limit: usize) -> CmdResult<Vec<ActivationEntry>> {
    to_cmd(api::activation_log(limit))
}

#[tauri::command]
fn set_master(enabled: bool) -> CmdResult<()> {
    to_cmd(api::set_master(enabled))
}

#[tauri::command]
fn set_session_enabled(session_id: String, enabled: bool) -> CmdResult<()> {
    to_cmd(api::set_session_enabled(&session_id, enabled))
}

#[tauri::command]
fn rebind_pane(pane_id: String, session_id: String) -> CmdResult<RebindOutcome> {
    to_cmd(api::rebind_pane(&pane_id, &session_id, None))
}

#[tauri::command]
fn swiftbar_present() -> CmdResult<bool> {
    to_cmd(api::swiftbar_plugin_present())
}

#[tauri::command]
fn remove_swiftbar() -> CmdResult<bool> {
    to_cmd(api::remove_swiftbar_plugin())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_doctor,
            get_activation_log,
            set_master,
            set_session_enabled,
            rebind_pane,
            swiftbar_present,
            remove_swiftbar,
        ])
        .setup(|app| {
            build_tray(app.handle())?;
            spawn_tray_updater(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // Hide to tray on close; never stop the daemon. Quitting is a
            // deliberate tray action, and even that leaves enforcement running.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("run Counterspell Desktop");
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItemBuilder::with_id("open", "Open Counterspell").build(app)?;
    let arm = MenuItemBuilder::with_id("arm", "Arm enforcement").build(app)?;
    let disarm = MenuItemBuilder::with_id("disarm", "Disarm enforcement").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit Counterspell").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&open, &arm, &disarm])
        .separator()
        .item(&quit)
        .build()?;

    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Counterspell")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "arm" => {
                let _ = api::set_master(true);
            }
            "disarm" => {
                let _ = api::set_master(false);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { .. } = event {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Keep the tray tooltip honest without the window open: recompute the verdict
/// on a slow cadence and reflect it in the tooltip.
fn spawn_tray_updater(app: AppHandle) {
    std::thread::spawn(move || loop {
        if let Ok(snapshot) = api::status_snapshot() {
            if let Some(tray) = app.tray_by_id("main-tray") {
                let _ = tray.set_tooltip(Some(tray_tooltip(&snapshot)));
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(6));
    });
}

fn tray_tooltip(snapshot: &StatusSnapshot) -> String {
    let detail = match &snapshot.verdict {
        Verdict::Shielded => format!("watching {} session(s)", snapshot.summary.watched),
        Verdict::Acting => "returning a drifted session to Fable".to_string(),
        Verdict::DriftBlocked { reason } => reason.clone(),
        Verdict::Disarmed => "enforcement paused".to_string(),
    };
    format!("Counterspell — {} · {}", snapshot.verdict.label(), detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use counterspell::api::{Health, Summary};

    fn snapshot(verdict: Verdict, watched: usize) -> StatusSnapshot {
        StatusSnapshot {
            verdict,
            summary: Summary {
                total: watched,
                watched,
                live_panes: 0,
                drifting: 0,
            },
            sessions: Vec::new(),
            health: Health {
                master_enabled: true,
                marker_path: "/tmp/disarmed".to_string(),
                daemon_status: "scheduled".to_string(),
                daemon_scheduled: true,
                herdr_reachable: true,
                last_tick_age: Some("3s".to_string()),
                armed_but_idle: false,
            },
            generated_at: "2026-07-07T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn tooltip_shows_shielded_count() {
        let tip = tray_tooltip(&snapshot(Verdict::Shielded, 4));
        assert!(tip.contains("SHIELDED"));
        assert!(tip.contains("watching 4"));
    }

    #[test]
    fn tooltip_shows_disarmed() {
        let tip = tray_tooltip(&snapshot(Verdict::Disarmed, 0));
        assert!(tip.contains("DISARMED"));
        assert!(tip.contains("paused"));
    }

    #[test]
    fn tooltip_shows_block_reason() {
        let tip = tray_tooltip(&snapshot(
            Verdict::DriftBlocked {
                reason: "two panes share this session".to_string(),
            },
            2,
        ));
        assert!(tip.contains("DRIFT-BLOCKED"));
        assert!(tip.contains("two panes"));
    }
}

// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icon;
mod view;

use chrono::Utc;
use juicemeter_core::config::{Mode, Show};
use juicemeter_core::fetch::{self, Machines};
use juicemeter_core::Config;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalPosition, Manager, Rect, RunEvent, WindowEvent, Wry};

/// Agents cache their reports, so reading them this often costs the providers nothing.
const REFRESH: Duration = Duration::from_secs(60);
const TRAY: &str = "juicemeter";
const PANEL: &str = "panel";

#[derive(Default)]
struct Shared {
    view: Mutex<Option<view::View>>,
    checks: Mutex<Vec<(String, CheckMenuItem<Wry>)>>,
    /// When the panel last hid itself on losing focus; a click on the tray icon right after
    /// that is the same click, and shouldn't reopen it.
    hidden_at: Mutex<Option<Instant>>,
    /// Accounts whose session is in the menu bar, kept between updates to avoid flicker.
    sessions: Mutex<std::collections::HashSet<String>>,
}

fn show_key(s: Show) -> &'static str {
    match s {
        Show::Remaining => "remaining",
        Show::Used => "used",
    }
}

/// Re-read the config and all machines, then update the menu bar and the panel.
async fn update(app: &AppHandle) {
    let config = Config::load().unwrap_or_else(|e| {
        eprintln!("juicemeter: {e:#}");
        Config::default()
    });
    let now = Utc::now();
    let (merged, machines) = match fetch::gather(&config, &Machines::All, false).await {
        Ok(g) => {
            let mut machines: Vec<view::Machine> =
                g.reports.iter().map(|r| view::Machine { name: view::short_host(&r.host), error: None }).collect();
            machines.extend(g.unreachable.into_iter().map(|(h, e)| view::Machine { name: h, error: Some(e) }));
            (juicemeter_core::merge::merge(&g.reports), machines)
        }
        Err(e) => (vec![], vec![view::Machine { name: "this mac".into(), error: Some(format!("{e:#}")) }]),
    };
    let mut v = view::build(&merged, machines, &config, now);
    {
        let shared = app.state::<Shared>();
        let mut sessions = shared.sessions.lock().unwrap();
        *sessions = view::sticky_sessions(&v, &sessions, now);
        v.sessions = sessions.clone();
    }
    let h = view::headline(&v, now);
    if std::env::var_os("JUICEMETER_DEBUG").is_some() {
        eprintln!("juicemeter: menu bar {:?}, fill {:.2}", h.title, h.fill);
    }

    if let Some(tray) = app.tray_by_id(TRAY) {
        let (px, w, hgt) = icon::juice_box(h.fill);
        let _ = tray.set_icon(Some(Image::new_owned(px, w, hgt)));
        let _ = tray.set_icon_as_template(true);
        let _ = tray.set_title(h.title.as_deref());
        let _ = tray.set_tooltip(Some(h.title.as_deref().unwrap_or("juicemeter")));
    }
    let state = app.state::<Shared>();
    for (id, item) in state.checks.lock().unwrap().iter() {
        let on = id == &format!("mode:{}", config.mode.key()) || id == &format!("show:{}", show_key(config.show));
        let _ = item.set_checked(on);
    }
    let _ = app.emit("view", &v);
    *state.view.lock().unwrap() = Some(v);
}

#[tauri::command]
fn get_view(state: tauri::State<'_, Shared>) -> Option<view::View> {
    state.view.lock().unwrap().clone()
}

#[tauri::command]
async fn refresh(app: AppHandle) {
    if let Ok(config) = Config::load() {
        fetch::refresh(&config).await;
    }
    update(&app).await;
}

async fn set_setting(app: &AppHandle, key: &str, value: &str) -> Result<(), String> {
    juicemeter_core::edit::set_value(key, Some(value)).map_err(|e| format!("{e:#}"))?;
    update(app).await;
    Ok(())
}

#[tauri::command]
async fn set_show(app: AppHandle, show: Show) -> Result<(), String> {
    set_setting(&app, "show", show_key(show)).await
}

#[tauri::command]
async fn set_mode(app: AppHandle, mode: Mode) -> Result<(), String> {
    set_setting(&app, "mode", mode.key()).await
}

/// Pin or unpin an account in the menu bar. Starts from what is shown now, so the first
/// toggle keeps the default pin unless that is the one being removed.
#[tauri::command]
async fn toggle_pin(app: AppHandle, state: tauri::State<'_, Shared>, key: String) -> Result<(), String> {
    let mut pins: Vec<String> = state
        .view
        .lock()
        .unwrap()
        .as_ref()
        .map(|v| v.entries.iter().filter(|e| e.pinned).map(|e| e.key.clone()).collect())
        .unwrap_or_default();
    match pins.iter().position(|k| *k == key) {
        Some(i) => {
            pins.remove(i);
        }
        None => pins.push(key),
    }
    juicemeter_core::edit::set_list("pins", &pins, &["pin"]).map_err(|e| format!("{e:#}"))?;
    update(&app).await;
    Ok(())
}

/// Menu bar text options: `bar_value` (both, percent, time) or `bar_names` (full, short).
#[tauri::command]
async fn set_bar(app: AppHandle, key: String, value: String) -> Result<(), String> {
    let valid = match key.as_str() {
        "bar_value" => ["both", "percent", "time"].contains(&value.as_str()),
        "bar_names" => ["full", "short"].contains(&value.as_str()),
        _ => false,
    };
    if !valid {
        return Err(format!("unknown menu bar option {key} = {value}"));
    }
    set_setting(&app, &key, &value).await
}

/// Hide an account on this machine, or bring it back.
#[tauri::command]
async fn set_hidden(app: AppHandle, key: String, hidden: bool) -> Result<(), String> {
    let mut list = Config::load().map_err(|e| format!("{e:#}"))?.hidden;
    list.retain(|k| *k != key);
    if hidden {
        list.push(key);
    }
    juicemeter_core::edit::set_list("hidden", &list, &[]).map_err(|e| format!("{e:#}"))?;
    update(&app).await;
    Ok(())
}

/// Name an account; an empty name removes the label.
#[tauri::command]
async fn set_label(app: AppHandle, key: String, label: String) -> Result<(), String> {
    let label = label.trim();
    juicemeter_core::edit::set_label(&key, (!label.is_empty()).then_some(label)).map_err(|e| format!("{e:#}"))?;
    update(&app).await;
    Ok(())
}

/// Add or remove another machine to read. `name` is a Tailscale machine name, optionally `:port`.
#[tauri::command]
async fn set_host(app: AppHandle, name: String, present: bool) -> Result<(), String> {
    let name = name.trim().to_string();
    let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "-.:".contains(c));
    if !valid {
        return Err(format!("`{name}` doesn't look like a machine name"));
    }
    let mut hosts = Config::load().map_err(|e| format!("{e:#}"))?.hosts;
    hosts.retain(|h| *h != name);
    if present {
        hosts.push(name);
    }
    juicemeter_core::edit::set_list("hosts", &hosts, &[]).map_err(|e| format!("{e:#}"))?;
    update(&app).await;
    Ok(())
}

/// Devices on the tailnet and whether each runs a juicemeter agent.
#[tauri::command]
async fn discover() -> Result<Vec<juicemeter_core::tailnet::Found>, String> {
    let net = juicemeter_core::tailnet::status().map_err(|e| format!("{e:#}"))?;
    Ok(juicemeter_core::tailnet::find_agents(&net.peers).await)
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    if enabled { launcher.enable() } else { launcher.disable() }.map_err(|e| e.to_string())
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

/// Show the panel centred under the tray icon, or hide it.
fn toggle_panel(app: &AppHandle, icon: Rect) {
    let Some(w) = app.get_webview_window(PANEL) else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
        return;
    }
    if app.state::<Shared>().hidden_at.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
        return;
    }
    // The icon's rect is in pixels of the display it sits on. Find that display, convert
    // with its own scale, and place the panel in points so it opens on the same screen.
    let monitors = w.available_monitors().unwrap_or_default();
    let on_screen = monitors.iter().find_map(|m| {
        let s = m.scale_factor();
        let (p, mp, ms) =
            (icon.position.to_logical::<f64>(s), m.position().to_logical::<f64>(s), m.size().to_logical::<f64>(s));
        let size = icon.size.to_logical::<f64>(s);
        let inside = p.x >= mp.x && p.x < mp.x + ms.width && p.y >= mp.y && p.y < mp.y + ms.height;
        // With another display's scale the point can still land inside a display, but the
        // icon's height then comes out halved or doubled; a menu bar is 24-37pt tall.
        let plausible = (18.0..=44.0).contains(&size.height);
        (inside && plausible).then_some((p, size, mp.x, mp.x + ms.width))
    });
    let (pos, size, left, right) = on_screen.unwrap_or_else(|| {
        let s = w.scale_factor().unwrap_or(1.0);
        (icon.position.to_logical(s), icon.size.to_logical(s), f64::MIN, f64::MAX)
    });
    let width = w.outer_size().map(|o| o.to_logical::<f64>(w.scale_factor().unwrap_or(1.0)).width).unwrap_or(360.0);
    let x = (pos.x + size.width / 2.0 - width / 2.0).min(right - width - 8.0).max(left + 8.0);
    let _ = w.set_position(LogicalPosition::new(x, pos.y + size.height + 4.0));
    // Show only once the move has landed: shown straight away, macOS draws the window for
    // a frame where it was last, which flashes on the other screen.
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let _ = w.show();
        let _ = w.set_focus();
    });
}

fn menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let mut checks = Vec::new();
    let mut check = |id: String, text: &str| -> tauri::Result<CheckMenuItem<Wry>> {
        let item = CheckMenuItem::with_id(app, &id, text, true, false, None::<&str>)?;
        checks.push((id, item.clone()));
        Ok(item)
    };
    let modes: Vec<CheckMenuItem<Wry>> =
        Mode::ALL.iter().map(|m| check(format!("mode:{}", m.key()), m.title())).collect::<tauri::Result<_>>()?;
    let left = check("show:remaining".into(), "Show what's left")?;
    let used = check("show:used".into(), "Show what's used")?;
    let mode_refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = modes.iter().map(|m| m as _).collect();
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &Submenu::with_items(app, "Menu bar shows", true, &mode_refs)?,
            &left,
            &used,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit juicemeter", true, None::<&str>)?,
        ],
    )?;
    *app.state::<Shared>().checks.lock().unwrap() = checks;
    Ok(menu)
}

fn on_menu(app: &AppHandle, id: &str) {
    let app = app.clone();
    let id = id.to_string();
    tauri::async_runtime::spawn(async move {
        let result = match id.split_once(':') {
            Some(("mode", m)) => set_setting(&app, "mode", m).await,
            Some(("show", s)) => set_setting(&app, "show", s).await,
            _ if id == "refresh" => {
                refresh(app.clone()).await;
                Ok(())
            }
            _ if id == "quit" => {
                app.exit(0);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            eprintln!("juicemeter: {e}");
        }
    });
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .manage(Shared::default())
        .invoke_handler(tauri::generate_handler![
            get_view,
            refresh,
            set_show,
            set_mode,
            toggle_pin,
            set_label,
            set_host,
            get_autostart,
            set_autostart,
            discover,
            set_bar,
            set_hidden,
            quit
        ])
        .setup(|app| {
            // Menu bar only: no Dock icon, no app menu.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let (px, w, h) = icon::juice_box(1.0);
            TrayIconBuilder::with_id(TRAY)
                .icon(Image::new_owned(px, w, h))
                .icon_as_template(true)
                .menu(&menu(&handle)?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| on_menu(app, e.id().as_ref()))
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = e
                    {
                        toggle_panel(tray.app_handle(), rect);
                    }
                })
                .build(app)?;

            if let Some(panel) = app.get_webview_window(PANEL) {
                let p = panel.clone();
                let handle = handle.clone();
                panel.on_window_event(move |e| match e {
                    WindowEvent::Focused(false) => {
                        let _ = p.hide();
                        *handle.state::<Shared>().hidden_at.lock().unwrap() = Some(Instant::now());
                    }
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        let _ = p.hide();
                    }
                    _ => {}
                });
            }

            tauri::async_runtime::spawn(async move {
                loop {
                    update(&handle).await;
                    tokio::time::sleep(REFRESH).await;
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start juicemeter")
        .run(|_, e| {
            // Hiding the panel shouldn't quit the app; only Quit does.
            if let RunEvent::ExitRequested { api, code: None, .. } = e {
                api.prevent_exit();
            }
        });
}

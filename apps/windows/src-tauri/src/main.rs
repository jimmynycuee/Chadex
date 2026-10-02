#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod credentials;
mod paths;
mod preferences;
mod runtime_version;
#[cfg(feature = "desktop-smoke")]
mod smoke;

use chadex_desktop_bridge::Bridge;
use paths::DesktopPaths;
use preferences::Preferences;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

struct Desktop {
    paths: DesktopPaths,
    bridge: Mutex<Option<Arc<Bridge>>>,
    preferences: Mutex<Preferences>,
    lifecycle: Mutex<()>,
    shutdown_started: AtomicBool,
    shutdown_complete: AtomicBool,
    last_attention: Mutex<Option<String>>,
    startup_error: Option<String>,
    runtime_version: Mutex<Option<String>>,
}

fn log_event(event: &str, ok: bool) {
    // Only app-owned event names and outcomes: never RPC params/output or OS errors.
    eprintln!(
        "{}",
        json!({"component":"windows_desktop", "event":event, "ok":ok})
    );
}

fn bridge_error(error: chadex_desktop_bridge::BridgeError) -> String {
    match &error.helper_code {
        Some(code) => format!("{error} ({code})。請查看診斷並重試。"),
        None => error.to_string(),
    }
}

impl Desktop {
    async fn helper(&self) -> Result<Arc<Bridge>, String> {
        self.bridge
            .lock()
            .await
            .clone()
            .ok_or_else(|| "helper_unavailable: 請重新啟動 helper。".into())
    }

    async fn launch(&self) -> Result<(), String> {
        if self.shutdown_started.load(Ordering::SeqCst) {
            return Err("desktop_shutting_down".into());
        }
        let bridge = Bridge::launch(&self.paths.helper, &self.paths.runtime, &self.paths.data)
            .map_err(|_| "helper_start_failed: 請檢查診斷中的 helper/runtime 路徑。")?;
        let bridge = Arc::new(bridge);
        let mut slot = self.bridge.lock().await;
        if self.shutdown_started.load(Ordering::SeqCst) {
            drop(slot);
            let _ = bridge.shutdown().await;
            return Err("desktop_shutting_down".into());
        }
        *slot = Some(bridge);
        drop(slot);
        log_event("helper_started", true);
        // Metadata only; readiness always comes from the helper. A missing or
        // slow version probe remains unknown and cannot mark a runtime ready.
        let binary = self.paths.runtime.join(if cfg!(windows) {
            "chadex-runtime-cli.exe"
        } else {
            "chadex-runtime-cli"
        });
        let version = runtime_version::probe(binary).await;
        *self.runtime_version.lock().await = version;
        let prefs = self.preferences.lock().await.clone();
        if prefs.restore_project {
            if let Some(path) = prefs.last_project {
                self.helper()
                    .await?
                    .request("activateProject", json!({"path":path}))
                    .await
                    .map_err(bridge_error)?;
            }
        }
        // Restoring a selection does not restore an old connected/verified UI.
        Ok(())
    }

    async fn provision(&self, bridge: &Bridge) -> Result<(), String> {
        let tunnel_id = self.preferences.lock().await.tunnel_id.clone();
        if tunnel_id.is_empty() {
            return Err("tunnel_id_required: 請設定 Tunnel ID。".into());
        }
        let credential =
            credentials::read()?.ok_or("credential_required: 請儲存 restricted API key。")?;
        bridge
            .request(
                "provideCredential",
                json!({"tunnel_id":tunnel_id, "api_key":credential.as_str()}),
            )
            .await
            .map_err(|_| "credential_provision_failed: 請檢查連線設定。")?;
        Ok(())
    }

    async fn shutdown(&self) {
        // Shutdown must preempt a long connection/startup RPC instead of
        // waiting behind the mutation lock. The helper owns request draining.
        let graceful = if let Some(bridge) = self.bridge.lock().await.take() {
            bridge.shutdown().await.is_ok()
        } else {
            true
        };
        #[cfg(feature = "desktop-smoke")]
        if let Some(directory) = std::env::var_os("CHADEX_DESKTOP_SMOKE_DIR") {
            let _ = std::fs::write(
                std::path::PathBuf::from(directory).join("shutdown.json"),
                json!({"graceful":graceful}).to_string(),
            );
        }
        self.shutdown_complete.store(true, Ordering::SeqCst);
        log_event("shutdown_complete", graceful);
    }
}

#[tauri::command]
async fn desktop_state(app: tauri::AppHandle, state: State<'_, Desktop>) -> Result<Value, String> {
    let prefs = state.preferences.lock().await.clone();
    let (mut health, mut runtime, mut activity) = match state.helper().await {
        Ok(bridge) => {
            let status = bridge
                .request(
                    "getStatus",
                    json!({"include_mascot_jobs":prefs.ferret_visible}),
                )
                .await;
            match status {
                Ok(snapshot) => {
                    let activities = tokio::time::timeout(
                        std::time::Duration::from_millis(300),
                        bridge.request("queryActivities", json!({"limit":100})),
                    )
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_else(|| json!([]));
                    let health = bridge.health();
                    (
                        json!({"state":health.state, "pid":health.pid, "error":health.error.map(|e| e.to_string())}),
                        Some(snapshot),
                        activities,
                    )
                }
                Err(error) => (
                    json!({"state":"failed", "pid":bridge.health().pid, "error":bridge_error(error)}),
                    None,
                    json!([]),
                ),
            }
        }
        Err(_) => (
            json!({"state":"failed", "pid":null, "error":"helper_unavailable"}),
            None,
            json!([]),
        ),
    };
    if health["state"] != "running" {
        runtime = None;
        activity = json!([]);
    }
    let mut traces = if runtime.is_some() {
        if let Ok(bridge) = state.helper().await {
            match tokio::time::timeout(
                std::time::Duration::from_millis(300),
                bridge.request("queryPerformanceTraces", json!({"limit":100})),
            )
            .await
            {
                Ok(Ok(Value::Array(entries))) => entries
                    .into_iter()
                    .map(|entry| {
                        let mut projected = serde_json::Map::new();
                        for key in [
                            "sequence",
                            "started_at_ms",
                            "finished_at_ms",
                            "tool_names",
                            "tool_failed",
                            "status_code",
                            "total_us",
                            "completion",
                        ] {
                            if let Some(value) = entry.get(key) {
                                projected.insert(key.into(), value.clone());
                            }
                        }
                        Value::Object(projected)
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    if let Ok(bridge) = state.helper().await {
        let final_health = bridge.health();
        if final_health.state != chadex_desktop_bridge::HelperState::Running {
            health = json!({"state":final_health.state, "pid":final_health.pid, "error":final_health.error.map(bridge_error)});
            runtime = None;
            activity = json!([]);
            traces.clear();
        }
    }
    let attention = if runtime.is_none() {
        Some("helper_failure".to_owned())
    } else if runtime
        .as_ref()
        .is_some_and(|r| r["runtime_status"]["needs_attention"] == true)
    {
        Some("runtime_needs_attention".to_owned())
    } else {
        runtime
            .as_ref()
            .and_then(|r| r.get("error"))
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let mut previous = state.last_attention.lock().await;
    if attention != *previous && attention.is_some() && prefs.notifications {
        let _ = app
            .notification()
            .builder()
            .title("Chadex 需要注意")
            .body("執行環境或連線需要檢查。請開啟 Chadex 診斷。")
            .show();
    }
    *previous = attention;
    if let Some(tray) = app.tray_by_id("chadex") {
        let label = if runtime.as_ref().is_some_and(|r| {
            r["chat_gpt_verified_for_selected_project"] == true
                && r["chat_gpt_connected"] == true
                && r["tunnel_ready"] == true
                && r["phase"] == "verified"
                && r["runtime_status"]["runtime_ready"] == true
        }) {
            "Chadex — ChatGPT 已驗證"
        } else if runtime
            .as_ref()
            .is_some_and(|r| r["runtime_status"]["runtime_ready"] == true)
        {
            "Chadex — 本地執行環境就緒"
        } else {
            "Chadex — 尚未就緒"
        };
        let _ = tray.set_tooltip(Some(label));
    }
    let credential = credentials::read();
    let credential_error = credential.as_ref().err().cloned();
    Ok(
        json!({"helper":health, "runtime":runtime, "activity":activity, "traces":traces, "preferences":prefs,
        "paths":state.paths, "credential_stored":credential.ok().flatten().is_some(),
        "credential_error":credential_error, "startup_error":state.startup_error,
        "runtime_version":*state.runtime_version.lock().await, "version":env!("CARGO_PKG_VERSION")}),
    )
}

const METHODS: &[&str] = &[
    "inspectProject",
    "activateProject",
    "switchLocalProject",
    "configureLocalSetup",
    "resumeService",
    "connectChatGPT",
    "startTunnel",
    "stopTunnel",
    "disconnectAI",
    "stopLocalService",
    "updateProxySettings",
    "getStatus",
    "refreshRuntime",
    "observeChatGPTActivity",
    "queryActivities",
    "queryPerformanceTraces",
    "queryLifecyclePerformanceTraces",
    "cancelOperation",
    "cancelTask",
];

#[tauri::command]
async fn runtime_action(
    state: State<'_, Desktop>,
    method: String,
    params: Value,
) -> Result<Value, String> {
    if !METHODS.contains(&method.as_str()) {
        return Err("desktop_method_not_allowed".into());
    }
    let _guard = state.lifecycle.lock().await;
    if state.shutdown_started.load(Ordering::SeqCst) {
        return Err("desktop_shutting_down".into());
    }
    let bridge = state.helper().await?;
    if ["connectChatGPT", "startTunnel"].contains(&method.as_str()) {
        state.provision(&bridge).await?;
    }
    let result = bridge.request(&method, params).await.map_err(bridge_error);
    log_event(&method, result.is_ok());
    let result = result?;
    if ["activateProject", "switchLocalProject"].contains(&method.as_str()) {
        if let Some(path) = result["selected_project"]["path"].as_str() {
            let mut prefs = state.preferences.lock().await;
            prefs.selected(path.into());
            prefs.save(&state.paths.data.join("desktop-preferences.json"))?;
        }
    }
    Ok(result)
}

#[tauri::command]
async fn choose_project(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("選擇 Chadex 專案資料夾")
        .pick_folder(move |selected| {
            let value =
                selected.map(|file| file.into_path().map(|p| p.to_string_lossy().into_owned()));
            let _ = sender.send(value);
        });
    match receiver.await.map_err(|_| "folder_dialog_closed")? {
        Some(Ok(path)) => Ok(Some(path)),
        Some(Err(_)) => Err("folder_path_invalid".into()),
        None => Ok(None),
    }
}

#[tauri::command]
async fn open_project(state: State<'_, Desktop>) -> Result<(), String> {
    let bridge = state.helper().await?;
    let snapshot = bridge
        .request("getStatus", json!({}))
        .await
        .map_err(bridge_error)?;
    let path = snapshot["selected_project"]["path"]
        .as_str()
        .ok_or("project_required")?;
    #[cfg(windows)]
    {
        // The helper already returns a canonical Windows display path. Validate
        // it without replacing it with std's extended-length \\?\ form.
        std::fs::canonicalize(path).map_err(|_| "project_path_unavailable")?;
        let system_root =
            std::env::var_os("SystemRoot").ok_or("windows_system_root_unavailable")?;
        std::process::Command::new(std::path::PathBuf::from(system_root).join("explorer.exe"))
            .arg(path)
            .spawn()
            .map_err(|_| "explorer_launch_failed")?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("windows_explorer_required".into())
    }
}

#[tauri::command]
async fn save_preferences(
    app: tauri::AppHandle,
    state: State<'_, Desktop>,
    mut preferences: Preferences,
) -> Result<Preferences, String> {
    let _guard = state.lifecycle.lock().await;
    let mut current = state.preferences.lock().await;
    // Recent projects/selection are runtime-owned, not writable via settings.
    preferences.recent_projects = current.recent_projects.clone();
    preferences.last_project = current.last_project.clone();
    preferences.validate()?;
    if preferences.launch_at_login != current.launch_at_login {
        let manager = app.autolaunch();
        let result = if preferences.launch_at_login {
            manager.enable()
        } else {
            manager.disable()
        };
        result.map_err(|_| "launch_at_login_update_failed")?;
    }
    if let Err(error) = preferences.save(&state.paths.data.join("desktop-preferences.json")) {
        if preferences.launch_at_login != current.launch_at_login {
            let manager = app.autolaunch();
            let _ = if current.launch_at_login {
                manager.enable()
            } else {
                manager.disable()
            };
        }
        return Err(error);
    }
    *current = preferences.clone();
    Ok(preferences)
}

#[tauri::command]
async fn store_credential(state: State<'_, Desktop>, credential: String) -> Result<(), String> {
    let _guard = state.lifecycle.lock().await;
    let credential = Zeroizing::new(credential);
    credentials::write(&credential)
}

#[tauri::command]
async fn forget_credential(state: State<'_, Desktop>) -> Result<(), String> {
    let _guard = state.lifecycle.lock().await;
    if let Ok(bridge) = state.helper().await {
        if bridge.request("clearCredential", json!({})).await.is_err() {
            // An unavailable transport cannot block OS credential revocation.
            // Stop its owned lifetime so a live tunnel cannot retain the key.
            let _ = bridge.shutdown().await;
            *state.bridge.lock().await = None;
            credentials::delete()?;
            if bridge.health().pid.is_some() {
                return Err("credential_deleted_runtime_cleanup_failed".into());
            }
            return Ok(());
        }
    }
    credentials::delete()
}

#[tauri::command]
async fn restart_helper(state: State<'_, Desktop>) -> Result<(), String> {
    let _guard = state.lifecycle.lock().await;
    if state.shutdown_started.load(Ordering::SeqCst) {
        return Err("desktop_shutting_down".into());
    }
    if let Some(bridge) = state.bridge.lock().await.take() {
        let health = bridge.health();
        let result = bridge.shutdown().await;
        if health.state == chadex_desktop_bridge::HelperState::Running {
            result.map_err(|_| "helper_shutdown_failed")?;
        }
    }
    state.launch().await
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

fn begin_shutdown(app: &tauri::AppHandle) {
    let state = app.state::<Desktop>();
    if state.shutdown_started.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        app.state::<Desktop>().shutdown().await;
        app.exit(0);
    });
}

fn main() {
    let builder = tauri::Builder::default()
        .on_page_load(|webview, payload| {
            #[cfg(feature = "desktop-smoke")]
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                smoke::start(webview);
            }
            #[cfg(not(feature = "desktop-smoke"))]
            let _ = (webview, payload);
        })
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            let mut data = app.path().app_local_data_dir()?;
            #[cfg(feature = "desktop-smoke")]
            if let Some(path) = std::env::var_os("CHADEX_DESKTOP_SMOKE_DATA") {
                data = path.into();
            }
            std::fs::create_dir_all(&data)?;
            let loaded = Preferences::load(&data.join("desktop-preferences.json"));
            let mut startup_error = loaded.as_ref().err().cloned();
            let mut prefs = loaded.unwrap_or_default();
            match app.autolaunch().is_enabled() {
                Ok(enabled) => prefs.launch_at_login = enabled,
                Err(_) => startup_error = Some("launch_at_login_read_failed".into()),
            }
            let paths = DesktopPaths::resolve(&app.path().resource_dir()?, data);
            app.manage(Desktop {
                paths,
                bridge: Mutex::new(None),
                preferences: Mutex::new(prefs),
                lifecycle: Mutex::new(()),
                shutdown_started: AtomicBool::new(false),
                shutdown_complete: AtomicBool::new(false),
                last_attention: Mutex::new(None),
                startup_error,
                runtime_version: Mutex::new(None),
            });
            let show =
                tauri::menu::MenuItem::with_id(app, "show", "Show Chadex", true, None::<&str>)?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &quit])?;
            let icon = tauri::image::Image::from_bytes(include_bytes!(
                "../../../../Sources/ChadexApp/Resources/ChadexIcon.png"
            ))?;
            tauri::tray::TrayIconBuilder::with_id("chadex")
                .menu(&menu)
                .icon(icon)
                .tooltip("Chadex — 尚未就緒")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<Desktop>();
                let _guard = state.lifecycle.lock().await;
                if state.launch().await.is_err() {
                    log_event("helper_start_failed", false);
                }
            });
            #[cfg(feature = "desktop-smoke")]
            smoke::install(app.handle())?;
            Ok(())
        });
    #[cfg(not(feature = "desktop-smoke"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        desktop_state,
        runtime_action,
        choose_project,
        open_project,
        save_preferences,
        store_credential,
        forget_credential,
        restart_helper,
        quit_app
    ]);
    #[cfg(feature = "desktop-smoke")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        desktop_state,
        runtime_action,
        choose_project,
        open_project,
        save_preferences,
        store_credential,
        forget_credential,
        restart_helper,
        quit_app,
        smoke::smoke_checkpoint
    ]);
    let app = builder
        .build(tauri::generate_context!())
        .expect("Chadex desktop initialization failed");
    app.run(|app, event| match event {
        tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } => {
            api.prevent_close();
            begin_shutdown(app);
        }
        tauri::RunEvent::ExitRequested { api, .. } => {
            if !app
                .state::<Desktop>()
                .shutdown_complete
                .load(Ordering::SeqCst)
            {
                api.prevent_exit();
                begin_shutdown(app);
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_and_shutdown_methods_are_backend_only() {
        for name in [
            "provideCredential",
            "clearCredential",
            "shutdown",
            "run_process",
        ] {
            assert!(!METHODS.contains(&name));
        }
    }
}

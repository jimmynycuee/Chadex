//! Compiled only into the CI smoke binary. Production exposes no fixture API.
use serde_json::{json, Value};
use std::path::PathBuf;
use tauri::Manager;

struct Smoke {
    directory: PathBuf,
    project_a: String,
    project_b: String,
    scenario: String,
}

pub fn install(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let Some(directory) = std::env::var_os("CHADEX_DESKTOP_SMOKE_DIR") else {
        return Ok(());
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory)?;
    app.manage(Smoke {
        directory,
        project_a: std::env::var("CHADEX_DESKTOP_SMOKE_PROJECT_A")?,
        project_b: std::env::var("CHADEX_DESKTOP_SMOKE_PROJECT_B")?,
        scenario: std::env::var("CHADEX_DESKTOP_SMOKE_SCENARIO")?,
    });
    Ok(())
}

#[tauri::command]
pub async fn smoke_checkpoint(
    app: tauri::AppHandle,
    stage: String,
    observation: Value,
) -> Result<(), String> {
    let smoke = app.try_state::<Smoke>().ok_or("smoke_not_enabled")?;
    if ![
        "webview_ready",
        "runtime_ready",
        "project_switched",
        "helper_restarted",
        "helper_crash_observed",
        "recovered",
        "startup_failure",
        "app_restored",
        "failed",
    ]
    .contains(&stage.as_str())
    {
        return Err("smoke_stage_invalid".into());
    }
    let mut safe = serde_json::Map::new();
    for field in [
        "helper_running",
        "runtime_ready",
        "verified",
        "rendered",
        "selected_expected",
        "preferences_restored",
    ] {
        if let Some(value) = observation.get(field).and_then(Value::as_bool) {
            safe.insert(field.into(), json!(value));
        }
    }
    if let Some(pid) = observation.get("helper_pid").and_then(Value::as_u64) {
        safe.insert("helper_pid".into(), json!(pid));
    }
    #[cfg(windows)]
    if stage == "webview_ready" {
        // An isolated Credential Manager entry proves OS-backed persistence;
        // never access the user's production service/account in a smoke run.
        let sample = zeroize::Zeroizing::new(format!("desktop-smoke-{}", std::process::id()));
        super::credentials::write(&sample)?;
        let stored = super::credentials::read()?;
        let matched = stored
            .as_ref()
            .is_some_and(|value| value.as_str() == sample.as_str());
        super::credentials::delete()?;
        if !matched {
            return Err("smoke_secure_storage_failed".into());
        }
        safe.insert("secure_storage_roundtrip".into(), json!(true));
    }
    #[cfg(windows)]
    if stage == "helper_crash_observed" {
        let sample = zeroize::Zeroizing::new(format!("desktop-smoke-{}", std::process::id()));
        super::credentials::write(&sample)?;
        super::forget_credential(app.state::<super::Desktop>()).await?;
        if super::credentials::read()?.is_some() {
            return Err("smoke_credential_revoke_failed".into());
        }
        safe.insert("credential_deleted_after_helper_death".into(), json!(true));
    }
    let output = smoke.directory.join(format!("{stage}.json"));
    std::fs::write(
        output,
        serde_json::to_vec(&safe).map_err(|_| "smoke_encode_failed")?,
    )
    .map_err(|_| "smoke_write_failed")?;
    let acknowledgement = smoke.directory.join(format!("{stage}.continue"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !acknowledgement.exists() {
        if std::time::Instant::now() >= deadline {
            return Err("smoke_ack_timeout".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(())
}

pub fn start(webview: &tauri::Webview<tauri::Wry>) {
    let Some(smoke) = webview.app_handle().try_state::<Smoke>() else {
        return;
    };
    let config = json!({"a":smoke.project_a, "b":smoke.project_b, "scenario":smoke.scenario});
    let script = SCRIPT.replace("__SMOKE_CONFIG__", &config.to_string());
    let _ = webview.eval(script);
}

const SCRIPT: &str = r#"
(async () => {
  if (window.__W3_SMOKE_STARTED) return;
  window.__W3_SMOKE_STARTED = true;
  const config = __SMOKE_CONFIG__;
  const invoke = (name, args) => window.__TAURI_INTERNALS__.invoke(name, args);
  const pause = () => new Promise(resolve => setTimeout(resolve, 150));
  const until = async predicate => {
    const deadline = Date.now() + 90000;
    while (Date.now() < deadline) { const s = await invoke('desktop_state'); if (predicate(s)) return s; await pause(); }
    throw new Error('smoke_deadline');
  };
  const checkpoint = (stage, state, extras={}) => invoke('smoke_checkpoint', {stage, observation:{
    helper_running: state.helper.state === 'running', helper_pid: state.helper.pid,
    runtime_ready: state.runtime?.runtime_status?.runtime_ready === true,
    verified: state.runtime?.chat_gpt_verified_for_selected_project === true,
    rendered: document.getElementById('root')?.childElementCount > 0, ...extras}});
  const action = (method, params={}) => invoke('runtime_action', {method, params});
  try {
    await new Promise(resolve => setTimeout(resolve, 1000));
    if (config.scenario === 'startup_failure') {
      const state = await until(s => s.helper.state === 'failed' && !s.runtime);
      await checkpoint('startup_failure', state);
      await invoke('quit_app'); return;
    }
    let state = await until(s => s.helper.state === 'running' && s.runtime);
    if (config.scenario === 'restore_only') {
      state = await until(s => s.helper.state === 'running' && s.runtime?.selected_project?.path === config.b);
      await checkpoint('app_restored', state, {
        selected_expected:state.runtime.selected_project?.path === config.b,
        preferences_restored:state.preferences.last_project === config.b && !state.preferences.ferret_visible});
      await invoke('quit_app'); return;
    }
    await checkpoint('webview_ready', state);
    await action('activateProject', {path:config.a});
    await action('configureLocalSetup');
    state = await until(s => s.runtime?.runtime_status?.runtime_ready === true);
    await checkpoint('runtime_ready', state, {selected_expected:state.runtime.selected_project.path === config.a});
    if (config.scenario === 'force_exit') return;
    await action('switchLocalProject', {path:config.b});
    state = await until(s => s.runtime?.runtime_status?.runtime_ready === true && s.runtime.selected_project.path === config.b);
    await checkpoint('project_switched', state, {selected_expected:true});
    await action('disconnectAI');
    state = await invoke('desktop_state');
    await invoke('save_preferences', {preferences:{...state.preferences, tunnel_id:'w3-smoke-no-credential'}});
    let rejected = false;
    try { await action('connectChatGPT'); } catch (error) { rejected = String(error).includes('credential_required'); }
    if (!rejected) throw new Error('credential_free_connection_not_rejected');
    await action('resumeService');
    state = await invoke('desktop_state');
    await invoke('save_preferences', {preferences:{...state.preferences, ferret_visible:false}});
    await invoke('restart_helper');
    await action('resumeService');
    state = await until(s => s.helper.state === 'running' && s.runtime?.runtime_status?.runtime_ready === true && s.runtime.selected_project?.path === config.b);
    await checkpoint('helper_restarted', state, {selected_expected:true, preferences_restored:!state.preferences.ferret_visible});
    state = await until(s => s.helper.state === 'failed' && s.helper.pid == null && !s.runtime);
    await checkpoint('helper_crash_observed', state);
    await invoke('restart_helper');
    await action('resumeService');
    state = await until(s => s.helper.state === 'running' && s.runtime?.runtime_status?.runtime_ready === true);
    await checkpoint('recovered', state, {selected_expected:state.runtime.selected_project.path === config.b});
    await invoke('quit_app');
  } catch (_) {
    try { await invoke('smoke_checkpoint', {stage:'failed', observation:{}}); } finally { await invoke('quit_app'); }
  }
})();
"#;

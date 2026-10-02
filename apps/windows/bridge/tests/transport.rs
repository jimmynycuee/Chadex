#![cfg(feature = "test-helper")]
use chadex_desktop_bridge::{
    request_timeout, Bridge, ErrorCode, HelperState, MAX_PENDING_REQUESTS, MAX_REQUEST_FRAME_BYTES,
};
use serde_json::json;
#[cfg(windows)]
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn fixture(behavior: &str) -> (tempfile::TempDir, Arc<Bridge>) {
    let root = tempfile::Builder::new()
        .prefix("Chadex bridge 中文 ")
        .tempdir()
        .unwrap();
    let runtime = root.path().join("runtime bins");
    let data = root.path().join("data");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("fake-mode"), behavior).unwrap();
    let bridge = Bridge::launch(
        Path::new(env!("CARGO_BIN_EXE_chadex-bridge-fake-helper")),
        &runtime,
        &data,
    )
    .unwrap();
    (root, Arc::new(bridge))
}

async fn wait_dead(bridge: &Bridge) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while bridge.health().pid.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn real_pipe_routes_out_of_order_ids_and_discards_unknown_ids() {
    let (_root, bridge) = fixture("");
    let (first, second) = tokio::join!(
        async {
            (
                bridge
                    .request("getStatus", json!({"tag": "first", "delay_ms": 250}))
                    .await
                    .unwrap(),
                Instant::now(),
            )
        },
        async {
            (
                bridge
                    .request("inspectProject", json!({"tag": "second", "delay_ms": 10}))
                    .await
                    .unwrap(),
                Instant::now(),
            )
        },
    );
    assert_eq!(first.0["tag"], "first");
    assert_eq!(second.0["tag"], "second");
    assert!(second.1 < first.1);
    assert_eq!(
        bridge
            .request("getStatus", json!({"mode": "unknown-id", "tag": "own"}))
            .await
            .unwrap()["tag"],
        "own"
    );
    bridge.shutdown().await.unwrap();
    assert_eq!(bridge.health().state, HelperState::Stopped);
    assert_eq!(bridge.health().pid, None);
}

#[tokio::test]
async fn protocol_mismatch_invalidates_transport_and_all_pending() {
    let (_root, bridge) = fixture("");
    let (waiting, wrong) = tokio::join!(
        bridge.request("getStatus", json!({"mode": "hang"})),
        bridge.request("getStatus", json!({"mode": "version"})),
    );
    assert_eq!(waiting.unwrap_err().code, ErrorCode::ProtocolMismatch);
    let error = wrong.unwrap_err();
    assert_eq!(error.code, ErrorCode::ProtocolMismatch);
    assert_eq!(error.received_protocol, Some(2));
    wait_dead(&bridge).await;
    assert_eq!(bridge.health().state, HelperState::Failed);
    assert_eq!(
        bridge
            .request("getStatus", json!({}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::ProtocolMismatch
    );
}

#[tokio::test]
async fn timeout_frees_slot_and_keeps_transport_usable_without_replay() {
    let (_root, bridge) = fixture("");
    let start = Instant::now();
    assert_eq!(
        bridge
            .request("getStatus", json!({"tag": "late", "delay_ms": 10200}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::RequestTimeout
    );
    assert!(start.elapsed() >= Duration::from_secs(10));
    assert!(start.elapsed() < Duration::from_secs(12));
    assert_eq!(bridge.health().state, HelperState::Running);
    assert_eq!(
        bridge
            .request("getStatus", json!({"tag": "after", "delay_ms": 500}))
            .await
            .unwrap()["tag"],
        "after"
    );
    bridge.shutdown().await.unwrap();
}

#[tokio::test]
async fn blocked_stdin_has_bounded_timeout_and_invalidates_partial_transport() {
    let (_root, bridge) = fixture("blocked-stdin");
    let started = Instant::now();
    assert_eq!(
        bridge
            .request("getStatus", json!({"padding": "x".repeat(200 * 1024)}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::RequestTimeout
    );
    assert!(started.elapsed() < Duration::from_secs(12));
    wait_dead(&bridge).await;
    assert_eq!(bridge.health().state, HelperState::Failed);
    assert_eq!(
        bridge.health().error.unwrap().code,
        ErrorCode::RequestTimeout
    );
}

#[tokio::test]
async fn helper_death_fails_pending_and_invalidates_health() {
    let (_root, bridge) = fixture("");
    let (waiting, dying) = tokio::join!(
        bridge.request("getStatus", json!({"mode": "hang"})),
        bridge.request("getStatus", json!({"mode": "death"})),
    );
    assert!(waiting.is_err());
    assert!(dying.is_err());
    wait_dead(&bridge).await;
    let health = bridge.health();
    assert_eq!(health.state, HelperState::Failed);
    assert_eq!(health.error.unwrap().code, ErrorCode::HelperDied);
    assert!(bridge.request("getStatus", json!({})).await.is_err());
}

#[tokio::test]
async fn malformed_and_oversized_real_pipe_frames_fail_closed() {
    for (mode, code) in [
        ("malformed", ErrorCode::InvalidResponse),
        ("huge", ErrorCode::ResponseTooLarge),
    ] {
        let (_root, bridge) = fixture("");
        assert_eq!(
            bridge
                .request("getStatus", json!({"mode": mode}))
                .await
                .unwrap_err()
                .code,
            code
        );
        wait_dead(&bridge).await;
        assert_eq!(bridge.health().state, HelperState::Failed);
    }
}

#[tokio::test]
async fn request_size_and_pending_count_are_bounded_and_cancellation_frees_slots() {
    let (root, bridge) = fixture("");
    assert_eq!(
        bridge
            .request(
                "provideCredential",
                json!({"api_key": "x".repeat(MAX_REQUEST_FRAME_BYTES)})
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::RequestTooLarge
    );
    let mut requests = Vec::new();
    for _ in 0..MAX_PENDING_REQUESTS {
        let bridge = Arc::clone(&bridge);
        requests.push(tokio::spawn(async move {
            bridge.request("getStatus", json!({"mode": "hang"})).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while std::fs::read_to_string(root.path().join("data/seen-requests"))
            .ok()
            .as_deref()
            != Some("64")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        bridge
            .request("getStatus", json!({}))
            .await
            .unwrap_err()
            .code,
        ErrorCode::TooManyPendingRequests
    );
    for request in requests {
        request.abort();
        let _ = request.await;
    }
    assert_eq!(
        bridge
            .request("getStatus", json!({"tag": "freed"}))
            .await
            .unwrap()["tag"],
        "freed"
    );
    bridge.shutdown().await.unwrap();
}

#[tokio::test]
async fn stderr_and_free_form_backend_errors_never_enter_health_or_error() {
    let (_root, bridge) = fixture("");
    bridge
        .request("getStatus", json!({"mode": "stderr"}))
        .await
        .unwrap();
    let error = bridge
        .request("getStatus", json!({"mode": "backend"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Backend);
    assert_eq!(error.helper_code.as_deref(), Some("invalid_params"));
    let unknown = bridge
        .request("getStatus", json!({"mode": "unknown-error"}))
        .await
        .unwrap_err();
    assert_eq!(unknown.helper_code.as_deref(), Some("unclassified"));
    for output in [
        serde_json::to_string(&error).unwrap(),
        format!("{error:?}"),
        error.to_string(),
        serde_json::to_string(&unknown).unwrap(),
        serde_json::to_string(&bridge.health()).unwrap(),
    ] {
        assert!(!output.contains("credential"));
    }
    assert!(bridge.health().error.is_none());
    bridge.shutdown().await.unwrap();
}

#[tokio::test]
async fn launch_uses_w2_environment_and_packaged_resource_resolution() {
    let (root, bridge) = fixture("");
    let env = bridge
        .request("getStatus", json!({"mode": "environment"}))
        .await
        .unwrap();
    assert_eq!(
        Path::new(env["data"].as_str().unwrap()),
        root.path().join("data").canonicalize().unwrap()
    );
    assert_eq!(
        Path::new(env["runtime"].as_str().unwrap()),
        root.path().join("runtime bins").canonicalize().unwrap()
    );
    assert_eq!(
        Path::new(env["resources"].as_str().unwrap()),
        root.path()
            .join("data/bridge-resources")
            .canonicalize()
            .unwrap()
    );
    assert_eq!(env["legacy_removed"], true);
    assert_eq!(env["python"], "1");
    bridge.shutdown().await.unwrap();
    let packaged = root.path().join("resources/chadex-runtime");
    std::fs::create_dir_all(&packaged).unwrap();
    let bridge = Bridge::launch(
        Path::new(env!("CARGO_BIN_EXE_chadex-bridge-fake-helper")),
        &packaged,
        &root.path().join("data"),
    )
    .unwrap();
    let env = bridge
        .request("getStatus", json!({"mode": "environment"}))
        .await
        .unwrap();
    assert_eq!(
        Path::new(env["resources"].as_str().unwrap()),
        packaged.parent().unwrap().canonicalize().unwrap()
    );
    bridge.shutdown().await.unwrap();
}

#[tokio::test]
async fn normal_shutdown_cancels_pending_and_reaps_owned_descendant_tree() {
    let (_root, bridge) = fixture("");
    let descendant = bridge
        .request("getStatus", json!({"mode": "spawn_descendant"}))
        .await
        .unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    let pending_bridge = Arc::clone(&bridge);
    let pending = tokio::spawn(async move {
        pending_bridge
            .request("getStatus", json!({"mode": "hang"}))
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    bridge.shutdown().await.unwrap();
    assert!(pending.await.unwrap().is_err());
    assert!(!process_alive(descendant));
    assert_eq!(bridge.health().state, HelperState::Stopped);
    bridge.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_ack_does_not_skip_wait_and_fallback_is_owned() {
    for behavior in ["ack-stuck", "ignore-shutdown", "leak-descendant"] {
        let (_root, bridge) = fixture(behavior);
        let descendant = bridge
            .request("getStatus", json!({"mode": "spawn_descendant"}))
            .await
            .unwrap()["pid"]
            .as_u64()
            .unwrap() as u32;
        let start = Instant::now();
        assert_eq!(
            bridge.shutdown().await.unwrap_err().code,
            ErrorCode::ShutdownForced
        );
        assert!(start.elapsed() >= Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_secs(7));
        wait_dead(&bridge).await;
        assert!(!process_alive(descendant));
        assert_eq!(bridge.health().state, HelperState::Failed);
    }
}

#[tokio::test]
async fn cancelling_shutdown_future_still_cleans_up_via_owner_deadline() {
    let (_root, bridge) = fixture("ignore-shutdown");
    let shutdown_bridge = Arc::clone(&bridge);
    let task = tokio::spawn(async move { shutdown_bridge.shutdown().await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(7), async {
        while bridge.health().pid.is_some() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        bridge.health().error.unwrap().code,
        ErrorCode::ShutdownForced
    );
}

#[tokio::test]
async fn dropping_bridge_terminates_owned_helper() {
    let (_root, bridge) = fixture("");
    bridge.request("getStatus", json!({})).await.unwrap();
    let pid = bridge.health().pid.unwrap();
    drop(bridge);
    tokio::time::timeout(Duration::from_secs(3), async {
        while process_alive(pid) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn timeouts_match_swift_w2_and_launch_failures_are_secret_free() {
    assert_eq!(request_timeout("getStatus"), Duration::from_secs(10));
    assert_eq!(request_timeout("activateProject"), Duration::from_secs(30));
    assert_eq!(
        request_timeout("configureLocalSetup"),
        Duration::from_secs(120)
    );
    assert_eq!(request_timeout("shutdown"), Duration::from_millis(1500));
    let error = Bridge::launch(
        Path::new("credential-secret-missing-helper"),
        Path::new("runtime"),
        Path::new("data"),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, ErrorCode::HelperMissing);
    assert!(!serde_json::to_string(&error)
        .unwrap()
        .contains("credential"));
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // Only known fixture PIDs. For orphan descendants macOS reaps on exit.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // Query known fixture identity only; never enumerate other processes.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        assert_ne!(ok, 0, "fixture process query failed");
        code == 259 // STILL_ACTIVE
    }
}

#[cfg(windows)]
#[test]
fn windows_parent_crash_job_object_kills_helper_and_descendants() {
    use std::process::{Child, Command, Stdio};
    struct Owner(Child);
    impl Drop for Owner {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let runtime = root.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    let marker = root.path().join("owned-pids.json");
    let fake = env!("CARGO_BIN_EXE_chadex-bridge-fake-helper");
    // Do NOT wrap the owner in ManagedChild: we must prove that the bridge's
    // own job closes on abrupt parent death, without an outer test job kill.
    let mut owner = Owner(
        Command::new(fake)
            .arg("--bridge-owner")
            .arg(fake)
            .arg(&runtime)
            .arg(root.path().join("data"))
            .arg(&marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    while !marker.is_file() && start.elapsed() < Duration::from_secs(10) {
        assert!(owner.0.try_wait().unwrap().is_none());
        std::thread::sleep(Duration::from_millis(20));
    }
    let pids: Value = serde_json::from_slice(&std::fs::read(marker).unwrap()).unwrap();
    let helper = pids["helper"].as_u64().unwrap() as u32;
    let descendant = pids["descendant"].as_u64().unwrap() as u32;
    assert!(process_alive(helper));
    assert!(process_alive(descendant));
    owner.0.kill().unwrap(); // TerminateProcess: no Rust drop/shutdown/EOF code.
    owner.0.wait().unwrap();
    let start = Instant::now();
    while (process_alive(helper) || process_alive(descendant))
        && start.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!process_alive(helper), "helper escaped parent Job Object");
    assert!(
        !process_alive(descendant),
        "descendant escaped parent Job Object"
    );
}

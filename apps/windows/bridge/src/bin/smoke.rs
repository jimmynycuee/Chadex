//! Isolated real-helper smoke. No configureLocalSetup, tunnel, credential,
//! model, MCP tool execution, or sensitive project workflow.
use chadex_desktop_bridge::{Bridge, HelperState};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Args {
    helper: PathBuf,
    runtime: PathBuf,
    output: Option<PathBuf>,
}

fn args() -> Option<Args> {
    let mut args = std::env::args_os().skip(1);
    let (mut helper, mut runtime, mut output) = (None, None, None);
    while let Some(flag) = args.next() {
        let value = PathBuf::from(args.next()?);
        if flag == "--helper" && helper.is_none() {
            helper = Some(value);
        } else if flag == "--runtime-dir" && runtime.is_none() {
            runtime = Some(value);
        } else if flag == "--output" && output.is_none() {
            output = Some(value);
        } else {
            return None;
        }
    }
    Some(Args {
        helper: helper?,
        runtime: runtime?,
        output,
    })
}

fn safe_result(stage: &str, result: &Value, project: &Path) -> Option<Value> {
    match stage {
        "inspect" => (result["readable"] == true
            && result["writable"] == true
            && result["path"].as_str().is_some_and(|path| {
                Path::new(path).canonicalize().ok().as_deref() == Some(project)
            }))
        .then(|| json!({"readable": true, "writable": true})),
        "activate" | "getStatus" | "disconnect" => {
            let same_project = result["selected_project"]["path"]
                .as_str()
                .is_some_and(|path| {
                    Path::new(path).canonicalize().ok().as_deref() == Some(project)
                });
            (same_project && result["tunnel_ready"] == false && result["chat_gpt_connected"] == false)
                .then(|| json!({"selected_project_matches": true, "tunnel_ready": false, "chat_gpt_connected": false}))
        }
        "getActivity" => result
            .as_array()
            .map(|entries| json!({"entry_count": entries.len()})),
        _ => None,
    }
}

async fn run(args: &Args, report: &mut Value) -> bool {
    let root = match tempfile::Builder::new()
        .prefix("Chadex W3 bridge 中文 ")
        .tempdir()
    {
        Ok(root) => root,
        Err(_) => {
            report["error_code"] = json!("fixture_create_failed");
            return false;
        }
    };
    let project = root.path().join("專案 with spaces");
    if std::fs::create_dir(&project).is_err() {
        report["error_code"] = json!("fixture_create_failed");
        return false;
    }
    let project = match project.canonicalize() {
        Ok(project) => project,
        Err(_) => {
            report["error_code"] = json!("fixture_create_failed");
            return false;
        }
    };
    let bridge = match Bridge::launch(&args.helper, &args.runtime, &root.path().join("data")) {
        Ok(bridge) => bridge,
        Err(error) => {
            report["launch_error"] = json!(error);
            return false;
        }
    };
    report["launch_health"] = json!(bridge.health());
    let stages = [
        ("inspect", "inspectProject", json!({"path": project})),
        ("activate", "activateProject", json!({"path": project})),
        ("getStatus", "getStatus", json!({})),
        ("getActivity", "queryActivities", json!({"limit": 10})),
        ("disconnect", "disconnectAI", json!({})),
    ];
    let mut passed = true;
    for (stage, method, params) in stages {
        let started = Instant::now();
        let mut entry = json!({"name": stage, "method": method, "passed": false});
        match bridge.request(method, params).await {
            Ok(result) => match safe_result(stage, &result, &project) {
                Some(evidence) => {
                    entry["passed"] = json!(true);
                    entry["evidence"] = evidence;
                }
                None => {
                    entry["error_code"] = json!("unexpected_result_shape");
                    passed = false;
                }
            },
            Err(error) => {
                entry["error"] = json!(error);
                passed = false;
            }
        }
        entry["elapsed_ms"] = json!(started.elapsed().as_millis() as u64);
        report["stages"].as_array_mut().unwrap().push(entry);
        if !passed {
            break;
        }
    }
    let started = Instant::now();
    let result = bridge.shutdown().await;
    let health = bridge.health();
    let graceful = result.is_ok() && health.state == HelperState::Stopped && health.pid.is_none();
    let mut shutdown = json!({"name": "shutdown", "method": "shutdown", "passed": graceful,
        "elapsed_ms": started.elapsed().as_millis() as u64, "health": health});
    if let Err(error) = result {
        shutdown["error"] = json!(error);
    }
    report["stages"].as_array_mut().unwrap().push(shutdown);
    passed &= graceful;
    if root.close().is_err() {
        report["cleanup_error_code"] = json!("fixture_cleanup_failed");
        passed = false;
    } else {
        report["fixture_removed"] = json!(true);
    }
    passed
}

#[tokio::main]
async fn main() {
    let Some(args) = args() else {
        eprintln!("Usage: chadex-bridge-smoke --helper <path> --runtime-dir <path> [--output <json-path>]");
        std::process::exit(2);
    };
    let mut report = json!({"schema": 1, "track": "W3", "passed": false, "status": "failed",
        "platform": std::env::consts::OS, "scope": "isolated real-helper bridge; no tunnel or runtime workflow",
        "default_data_path_verified": false, "default_resource_path_verified": false,
        "isolation_overrides": ["CHADEX_DATA_DIR", "CHADEX_RESOURCE_DIR", "CHADEX_RUNTIME_BIN_DIR", "PYTHONDONTWRITEBYTECODE"],
        "stages": []});
    let passed = run(&args, &mut report).await;
    report["passed"] = json!(passed);
    report["status"] = json!(if passed { "passed" } else { "failed" });
    let bytes = serde_json::to_vec_pretty(&report).unwrap();
    if let Some(output) = args.output {
        if std::fs::write(output, &bytes).is_err() {
            eprintln!("{{\"code\":\"report_write_failed\"}}");
            std::process::exit(1);
        }
    }
    println!("{}", String::from_utf8(bytes).unwrap());
    std::process::exit(if passed { 0 } else { 1 });
}

//! Actual subprocess/pipe fixture, compiled only with --features test-helper.
use chadex_desktop_bridge::Bridge;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn emit(payload: Value, stdout: &Mutex<io::Stdout>) {
    let mut output = stdout.lock().unwrap();
    serde_json::to_writer(&mut *output, &payload).unwrap();
    output.write_all(b"\n").unwrap();
    output.flush().unwrap();
}

fn forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "--descendant") {
        forever();
    }
    if args.get(1).is_some_and(|arg| arg == "--bridge-owner") {
        let bridge = Bridge::launch(
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&args[4]),
        )
        .unwrap();
        let child = bridge
            .request("getStatus", json!({"mode": "spawn_descendant"}))
            .await
            .unwrap();
        let marker = PathBuf::from(&args[5]);
        let temporary = marker.with_extension("tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec(&json!({
                "helper": bridge.health().pid.unwrap(), "descendant": child["pid"],
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::rename(temporary, marker).unwrap();
        forever(); // TerminateProcess deliberately bypasses Bridge::drop.
    }
    let data = PathBuf::from(std::env::var_os("CHADEX_DATA_DIR").unwrap());
    let behavior = std::fs::read_to_string(data.join("fake-mode")).unwrap_or_default();
    if behavior == "blocked-stdin" {
        forever();
    }
    let stdout = Arc::new(Mutex::new(io::stdout()));
    let children: Arc<Mutex<Vec<Child>>> = Arc::new(Mutex::new(Vec::new()));
    for (index, line) in io::stdin().lock().lines().enumerate() {
        let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
        std::fs::write(data.join("seen-requests"), (index + 1).to_string()).unwrap();
        assert_eq!(request["protocol_version"], 1);
        let id = request["request_id"].clone();
        if request["method"] == "shutdown" {
            if behavior == "ignore-shutdown" {
                continue;
            }
            emit(
                json!({"protocol_version": 1, "request_id": id, "result": {"shutdown": true}}),
                &stdout,
            );
            if behavior == "ack-stuck" {
                forever();
            }
            if behavior != "leak-descendant" {
                for child in children.lock().unwrap().iter_mut() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            return;
        }
        let stdout = Arc::clone(&stdout);
        let children = Arc::clone(&children);
        std::thread::spawn(move || {
            let params = &request["params"];
            match params["mode"].as_str().unwrap_or("echo") {
                "hang" => return,
                "death" => std::process::exit(23),
                "version" => {
                    emit(
                        json!({"protocol_version": 2, "request_id": id, "result": {}}),
                        &stdout,
                    );
                    return;
                }
                "malformed" | "partial" => {
                    let mut output = stdout.lock().unwrap();
                    output
                        .write_all(if params["mode"] == "partial" {
                            b"{\"request_id\":"
                        } else {
                            b"{bad-json}\n"
                        })
                        .unwrap();
                    output.flush().unwrap();
                    if params["mode"] == "partial" {
                        std::process::exit(0);
                    }
                    return;
                }
                "huge" => {
                    let mut output = stdout.lock().unwrap();
                    let _ = output.write_all(&vec![b'x'; 1024 * 1024 + 1]);
                    let _ = output.flush();
                    return;
                }
                "stderr" => {
                    let mut stderr = io::stderr().lock();
                    for _ in 0..1024 {
                        stderr.write_all(b"credential-from-stderr\xff\n").unwrap();
                    }
                    stderr.flush().unwrap();
                }
                "backend" | "unknown-error" | "backend-code" => {
                    let code = if params["mode"] == "backend-code" {
                        params["code"].as_str().unwrap_or("invalid_params")
                    } else if params["mode"] == "backend" {
                        "invalid_params"
                    } else {
                        "credential_in_error_code"
                    };
                    emit(
                        json!({"protocol_version": 1, "request_id": id, "error": {
                            "code": code, "message": "credential-from-message", "recovery": "credential-from-recovery",
                            "details": {"api_key": "credential-from-details"},
                        }}),
                        &stdout,
                    );
                    return;
                }
                "environment" => {
                    emit(
                        json!({"protocol_version": 1, "request_id": id, "result": {
                            "data": std::env::var_os("CHADEX_DATA_DIR").unwrap().to_string_lossy(),
                            "runtime": std::env::var_os("CHADEX_RUNTIME_BIN_DIR").unwrap().to_string_lossy(),
                            "resources": std::env::var_os("CHADEX_RESOURCE_DIR").unwrap().to_string_lossy(),
                            "legacy_removed": std::env::var_os("WEBCODEX_DESKTOP_BIN_DIR").is_none(),
                            "python": std::env::var("PYTHONDONTWRITEBYTECODE").unwrap(),
                        }}),
                        &stdout,
                    );
                    return;
                }
                "spawn_descendant" => {
                    let child = Command::new(std::env::current_exe().unwrap())
                        .arg("--descendant")
                        .stdin(Stdio::null())
                        .stdout(Stdio::inherit())
                        .stderr(Stdio::null())
                        .spawn()
                        .unwrap();
                    let pid = child.id();
                    children.lock().unwrap().push(child);
                    emit(
                        json!({"protocol_version": 1, "request_id": id, "result": {"pid": pid}}),
                        &stdout,
                    );
                    return;
                }
                "unknown-id" => emit(
                    json!({"protocol_version": 1, "request_id": "desktop-unknown", "result": {"wrong": true}}),
                    &stdout,
                ),
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(
                params["delay_ms"].as_u64().unwrap_or(0),
            ));
            emit(
                json!({"protocol_version": 1, "request_id": id, "result": {"tag": params["tag"], "method": request["method"]}}),
                &stdout,
            );
        });
    }
    if behavior == "ignore-shutdown" {
        forever();
    }
}

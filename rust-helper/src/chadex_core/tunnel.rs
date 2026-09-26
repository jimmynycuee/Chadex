use super::credentials::CredentialStore;
use super::performance::PerformanceTraceStore;
use super::verification::{McpIngress, VerificationTracker};
use super::{ChadexError, ChadexResult};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

const TUNNEL_CLIENT_VERSION: &str = "0.0.12";
const RELEASE_BASE: &str = "https://github.com/openai/tunnel-client/releases/download/v0.0.12";
const READY_TIMEOUT: Duration = Duration::from_secs(90);
const READY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const READY_FAST_PROBE_INTERVAL: Duration = Duration::from_millis(25);
const READY_FAST_PROBE_WINDOW: Duration = Duration::from_secs(1);
const READY_STEADY_PROBE_INTERVAL: Duration = Duration::from_millis(100);
const INGRESS_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ENV_BYTES: u64 = 256 * 1024;
const MAX_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Unconfigured,
    Stopped,
    Starting,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TunnelSnapshot {
    pub state: TunnelState,
    pub configured: bool,
    pub tunnel_id: Option<String>,
    pub epoch: u64,
    pub last_error: Option<String>,
}

impl Default for TunnelSnapshot {
    fn default() -> Self {
        Self {
            state: TunnelState::Unconfigured,
            configured: false,
            tunnel_id: None,
            epoch: 0,
            last_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTunnelTarget {
    pub server_url: String,
    pub server_env_file: PathBuf,
    pub bootstrap_token_env: String,
}

struct TunnelSession {
    directory: PathBuf,
    authorization_file: PathBuf,
    health_url_file: PathBuf,
    log_file: PathBuf,
}

impl TunnelSession {
    fn create(root: &Path, bootstrap_token: &str) -> ChadexResult<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = root
            .join("tunnel-sessions")
            .join(format!("{}-{nonce}", std::process::id()));
        create_private_dir(&directory)?;
        let authorization_file = directory.join("mcp-authorization");
        write_private_file(
            &authorization_file,
            Zeroizing::new(format!("Bearer {}", bootstrap_token.trim())).as_bytes(),
        )?;
        Ok(Self {
            health_url_file: directory.join("health-url"),
            log_file: directory.join("tunnel.log"),
            authorization_file,
            directory,
        })
    }
}

impl Drop for TunnelSession {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

struct TunnelInner {
    credentials: CredentialStore,
    child: Option<Child>,
    session: Option<TunnelSession>,
    ingress: Option<Arc<McpIngress>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TunnelClientResolutionContext {
    direct_override: Option<PathBuf>,
    env_override: Option<PathBuf>,
    path_env: Option<OsString>,
}

impl TunnelClientResolutionContext {
    fn capture(direct_override: Option<&PathBuf>) -> Self {
        if let Some(path) = direct_override {
            return Self {
                direct_override: Some(path.clone()),
                env_override: None,
                path_env: None,
            };
        }
        let env_override = std::env::var_os("CHADEX_TUNNEL_CLIENT_BIN").map(PathBuf::from);
        let path_env = if env_override.is_none() {
            std::env::var_os("PATH")
        } else {
            None
        };
        Self {
            direct_override: None,
            env_override,
            path_env,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TunnelClientFileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[derive(Debug, Clone)]
struct VerifiedTunnelClient {
    context: TunnelClientResolutionContext,
    path: PathBuf,
    identity: TunnelClientFileIdentity,
}

pub struct TunnelManager {
    root: PathBuf,
    tunnel_client_override: Option<PathBuf>,
    verification: Arc<VerificationTracker>,
    performance: Arc<PerformanceTraceStore>,
    inner: Mutex<TunnelInner>,
    lifecycle: Mutex<()>,
    published: RwLock<TunnelSnapshot>,
    verified_tunnel_client: RwLock<Option<VerifiedTunnelClient>>,
    stop_requested: AtomicBool,
    next_epoch: AtomicU64,
}

impl TunnelManager {
    pub fn new(
        root: PathBuf,
        verification: Arc<VerificationTracker>,
        performance: Arc<PerformanceTraceStore>,
    ) -> Self {
        Self {
            root,
            tunnel_client_override: None,
            verification,
            performance,
            inner: Mutex::new(TunnelInner {
                credentials: CredentialStore::default(),
                child: None,
                session: None,
                ingress: None,
            }),
            lifecycle: Mutex::new(()),
            published: RwLock::new(TunnelSnapshot::default()),
            verified_tunnel_client: RwLock::new(None),
            stop_requested: AtomicBool::new(false),
            next_epoch: AtomicU64::new(1),
        }
    }

    #[cfg(test)]
    fn new_with_tunnel_client(
        root: PathBuf,
        verification: Arc<VerificationTracker>,
        performance: Arc<PerformanceTraceStore>,
        tunnel_client: PathBuf,
    ) -> Self {
        let mut manager = Self::new(root, verification, performance);
        manager.tunnel_client_override = Some(tunnel_client);
        manager
    }

    pub fn snapshot(&self) -> TunnelSnapshot {
        self.published
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub async fn provide_credentials(
        &self,
        tunnel_id: &str,
        api_key: Zeroizing<String>,
    ) -> ChadexResult<TunnelSnapshot> {
        self.stop_requested.store(true, Ordering::SeqCst);
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_locked().await?;
        let mut inner = self.inner.lock().await;
        inner.credentials.provide(tunnel_id, api_key)?;
        let epoch = self.next_epoch.fetch_add(1, Ordering::SeqCst);
        let snapshot = TunnelSnapshot {
            state: TunnelState::Stopped,
            configured: true,
            tunnel_id: inner.credentials.tunnel_id(),
            epoch,
            last_error: None,
        };
        drop(inner);
        self.publish(snapshot.clone());
        Ok(snapshot)
    }

    pub async fn clear_credentials(&self) -> ChadexResult<TunnelSnapshot> {
        self.stop_requested.store(true, Ordering::SeqCst);
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_locked().await?;
        let mut inner = self.inner.lock().await;
        inner.credentials.clear();
        let epoch = self.next_epoch.fetch_add(1, Ordering::SeqCst);
        let snapshot = TunnelSnapshot {
            state: TunnelState::Unconfigured,
            configured: false,
            tunnel_id: None,
            epoch,
            last_error: None,
        };
        drop(inner);
        self.publish(snapshot.clone());
        Ok(snapshot)
    }

    pub async fn refresh(&self) -> TunnelSnapshot {
        let mut inner = self.inner.lock().await;
        if let Some(child) = inner.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    inner.child = None;
                    inner.session = None;
                    inner.ingress = None;
                    let mut snapshot = self.snapshot();
                    snapshot.state = TunnelState::Error;
                    snapshot.last_error = Some(format!(
                        "OpenAI tunnel-client exited unexpectedly ({status})"
                    ));
                    self.publish(snapshot.clone());
                    return snapshot;
                }
                Ok(None) => {
                    // The live child is installed only after the tunnel has passed
                    // /readyz. If an older snapshot was published later, the live
                    // child is the stronger source of truth and can self-heal it.
                    let mut snapshot = self.snapshot();
                    if snapshot.state != TunnelState::Ready {
                        snapshot.state = TunnelState::Ready;
                        snapshot.last_error = None;
                        self.publish(snapshot);
                    }
                }
                Err(_) => {
                    inner.child = None;
                    inner.session = None;
                    inner.ingress = None;
                    let mut snapshot = self.snapshot();
                    snapshot.state = TunnelState::Error;
                    snapshot.last_error =
                        Some("OpenAI tunnel-client could not be supervised".to_string());
                    self.publish(snapshot.clone());
                    return snapshot;
                }
            }
        }
        self.snapshot()
    }

    pub async fn start(
        &self,
        target: RuntimeTunnelTarget,
        proxy_url: Option<&str>,
    ) -> ChadexResult<TunnelSnapshot> {
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_requested.store(false, Ordering::SeqCst);
        let (tunnel_id, api_key) = {
            let inner = self.inner.lock().await;
            if inner.child.is_some() {
                return Err(ChadexError::new(
                    "regular_tunnel_already_running",
                    "OpenAI Secure MCP Tunnel is already running",
                    "Stop the current tunnel before starting another one.",
                ));
            }
            inner.credentials.for_start()?
        };
        validate_loopback_server_url(&target.server_url)?;
        let bootstrap_token =
            read_bootstrap_token(&target.server_env_file, &target.bootstrap_token_env)?;
        let backend_mcp_url = format!("{}/mcp", target.server_url.trim_end_matches('/'));
        let epoch = self.next_epoch.fetch_add(1, Ordering::SeqCst);
        self.publish(TunnelSnapshot {
            state: TunnelState::Starting,
            configured: true,
            tunnel_id: Some(tunnel_id.clone()),
            epoch,
            last_error: None,
        });

        let result = self
            .start_inner(
                &tunnel_id,
                &api_key,
                &backend_mcp_url,
                &bootstrap_token,
                &target.bootstrap_token_env,
                proxy_url,
            )
            .await;
        match result {
            Ok((child, session, ingress)) => {
                let mut inner = self.inner.lock().await;
                inner.child = Some(child);
                inner.session = Some(session);
                inner.ingress = Some(Arc::new(ingress));
                let snapshot = TunnelSnapshot {
                    state: TunnelState::Ready,
                    configured: true,
                    tunnel_id: Some(tunnel_id),
                    epoch,
                    last_error: None,
                };
                drop(inner);
                self.publish(snapshot.clone());
                Ok(snapshot)
            }
            Err(error) => {
                let snapshot = TunnelSnapshot {
                    state: TunnelState::Error,
                    configured: true,
                    tunnel_id: Some(tunnel_id),
                    epoch,
                    last_error: Some(error.message.clone()),
                };
                self.publish(snapshot);
                Err(error)
            }
        }
    }

    async fn start_inner(
        &self,
        tunnel_id: &str,
        api_key: &Zeroizing<String>,
        backend_mcp_url: &str,
        bootstrap_token: &Zeroizing<String>,
        bootstrap_token_env: &str,
        proxy_url: Option<&str>,
    ) -> ChadexResult<(Child, TunnelSession, McpIngress)> {
        let binary = self.resolve_tunnel_client_for_start().await?;
        let session = TunnelSession::create(&self.root, bootstrap_token.as_str())?;
        let ingress = McpIngress::start(
            backend_mcp_url.to_string(),
            Arc::clone(&self.verification),
            Arc::clone(&self.performance),
        )
        .await?;
        let mcp_url = ingress.mcp_url().to_string();
        if self.stop_requested.load(Ordering::SeqCst) {
            return Err(cancelled_error());
        }
        let helper = std::env::current_exe().map_err(|_| {
            ChadexError::new(
                "tunnel_supervisor_unavailable",
                "Chadex could not locate its tunnel supervisor",
                "Restart Chadex and retry the connection.",
            )
        })?;
        // The libtest executable replaces main(), so lifecycle unit tests use
        // the companion real binary built by `cargo test` for integration tests.
        #[cfg(test)]
        let helper = helper
            .parent()
            .and_then(Path::parent)
            .expect("test executable lives under target/profile/deps")
            .join("chadex-helper");
        let mut command = Command::new(helper);
        command.arg("--supervise-tunnel").arg(&binary);
        configure_tunnel_command(
            &mut command,
            tunnel_id,
            api_key.as_str(),
            &mcp_url,
            &session.authorization_file,
            bootstrap_token_env,
            proxy_url,
        );
        command
            .arg("run")
            .arg("--health.listen-addr")
            .arg("127.0.0.1:0")
            .arg("--health.url-file")
            .arg(&session.health_url_file)
            .arg("--log.file")
            .arg(&session.log_file)
            .arg("--log.format")
            .arg("json")
            .arg("--log.level")
            .arg("info")
            // The supervisor owns the tunnel's process tree. Dropping this
            // writer (including helper process death) revokes its lease.
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(false);
        let mut child = command.spawn().map_err(|_| {
            ChadexError::new(
                "tunnel_unavailable",
                "OpenAI tunnel-client could not start",
                "Check the Tunnel configuration and retry.",
            )
        })?;
        if let Err(error) =
            wait_until_ready(&mut child, &session.health_url_file, &self.stop_requested).await
        {
            let _ = stop_tunnel_child(&mut child).await;
            return Err(error);
        }
        ingress.arm_verification();
        Ok((child, session, ingress))
    }

    async fn resolve_tunnel_client_for_start(&self) -> ChadexResult<PathBuf> {
        let context = TunnelClientResolutionContext::capture(self.tunnel_client_override.as_ref());
        let cached = self
            .verified_tunnel_client
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(cached) = cached {
            if cached.context == context
                && tunnel_client_file_identity(&cached.path).ok().as_ref() == Some(&cached.identity)
            {
                return Ok(cached.path);
            }
            *self
                .verified_tunnel_client
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        }

        let path = if let Some(path) = self.tunnel_client_override.as_ref() {
            verify_tunnel_client(path).await?;
            path.clone()
        } else {
            resolve_tunnel_client(&self.root).await?
        };
        let identity = tunnel_client_file_identity(&path)?;
        *self
            .verified_tunnel_client
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(VerifiedTunnelClient {
            context,
            path: path.clone(),
            identity,
        });
        Ok(path)
    }

    pub async fn pause_ingress(&self) -> ChadexResult<bool> {
        self.pause_ingress_with_timeout(INGRESS_DRAIN_TIMEOUT).await
    }

    async fn pause_ingress_with_timeout(&self, timeout: Duration) -> ChadexResult<bool> {
        // Never hold the tunnel mutex while waiting for in-flight MCP requests.
        // A stalled request must not block stop/restart or project-switch rollback.
        let ingress = {
            let inner = self.inner.lock().await;
            inner.ingress.as_ref().map(Arc::clone)
        };
        let Some(ingress) = ingress else {
            return Ok(false);
        };
        match tokio::time::timeout(timeout, ingress.pause_and_drain()).await {
            Ok(()) => Ok(true),
            Err(_) => {
                // Activation has not started yet, so restoring admission returns
                // the current project to its pre-switch behavior safely.
                ingress.resume();
                Err(ChadexError::new(
                    "ingress_drain_timeout",
                    "Chadex could not quiesce active ChatGPT requests in time",
                    "Retry the project switch after the current ChatGPT request finishes.",
                ))
            }
        }
    }

    pub async fn resume_ingress(&self) -> bool {
        let ingress = {
            let inner = self.inner.lock().await;
            inner.ingress.as_ref().map(Arc::clone)
        };
        let Some(ingress) = ingress else {
            return false;
        };
        ingress.resume();
        true
    }

    pub async fn stop(&self) -> ChadexResult<TunnelSnapshot> {
        self.stop_requested.store(true, Ordering::SeqCst);
        let _lifecycle = self.lifecycle.lock().await;
        self.stop_locked().await
    }

    async fn stop_locked(&self) -> ChadexResult<TunnelSnapshot> {
        let mut inner = self.inner.lock().await;
        if let Some(child) = inner.child.as_mut() {
            stop_tunnel_child(child).await?;
        }
        inner.child = None;
        inner.session = None;
        inner.ingress = None;
        let configured = inner.credentials.configured();
        let snapshot = TunnelSnapshot {
            state: if configured {
                TunnelState::Stopped
            } else {
                TunnelState::Unconfigured
            },
            configured,
            tunnel_id: inner.credentials.tunnel_id(),
            epoch: self.next_epoch.fetch_add(1, Ordering::SeqCst),
            last_error: None,
        };
        drop(inner);
        self.publish(snapshot.clone());
        Ok(snapshot)
    }

    pub async fn shutdown(&self) {
        let _ = self.stop().await;
        let mut inner = self.inner.lock().await;
        inner.credentials.clear();
    }

    fn publish(&self, snapshot: TunnelSnapshot) {
        *self
            .published
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot;
    }
}

fn validate_loopback_server_url(value: &str) -> ChadexResult<()> {
    let parsed = url::Url::parse(value).map_err(|_| runtime_target_error())?;
    let host = parsed.host_str().unwrap_or("");
    if parsed.scheme() == "http"
        && matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
        && parsed.port().is_some()
    {
        Ok(())
    } else {
        Err(runtime_target_error())
    }
}

fn runtime_target_error() -> ChadexError {
    ChadexError::new(
        "runtime_not_ready",
        "The Chadex local MCP runtime is not available on loopback",
        "Restore the local service before starting the Secure MCP Tunnel.",
    )
}

fn read_bootstrap_token(path: &Path, bootstrap_token_env: &str) -> ChadexResult<Zeroizing<String>> {
    let metadata = fs::symlink_metadata(path).map_err(|_| runtime_target_error())?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_ENV_BYTES {
        return Err(runtime_target_error());
    }
    let content = fs::read_to_string(path).map_err(|_| runtime_target_error())?;
    for line in content.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == bootstrap_token_env {
            let value = value.trim();
            if !value.is_empty()
                && value.len() <= 8192
                && !value.contains('\r')
                && !value.contains('\n')
            {
                return Ok(Zeroizing::new(value.to_string()));
            }
        }
    }
    Err(ChadexError::new(
        "tunnel_auth_invalid",
        "The local MCP bootstrap credential is unavailable",
        "Restart the local service and retry the Secure MCP Tunnel.",
    ))
}

async fn stop_tunnel_child(child: &mut Child) -> ChadexResult<()> {
    // Never kill the supervisor first: it must reap its owned tunnel tree.
    // On timeout retain the Child/session in TunnelInner for a later retry.
    drop(child.stdin.take());
    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        _ => Err(ChadexError::new(
            "tunnel_stop_incomplete",
            "Chadex is still waiting for its tunnel supervisor to stop",
            "Retry disconnecting after the current cleanup completes.",
        )),
    }
}

fn configure_tunnel_command(
    command: &mut Command,
    tunnel_id: &str,
    api_key: &str,
    mcp_url: &str,
    authorization_file: &Path,
    bootstrap_token_env: &str,
    proxy_url: Option<&str>,
) {
    command
        .env("CONTROL_PLANE_TUNNEL_ID", tunnel_id)
        .env("CONTROL_PLANE_API_KEY", api_key)
        .env_remove("OPENAI_ADMIN_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove(bootstrap_token_env)
        .arg("--mcp.server-url")
        .arg(format!("url={mcp_url},channel=main"))
        .arg("--mcp.extra-headers")
        .arg(format!(
            "Authorization: file:{}",
            authorization_file.to_string_lossy()
        ));
    if let Some(proxy) = proxy_url.filter(|value| !value.trim().is_empty()) {
        command.env("HTTPS_PROXY", proxy).env("HTTP_PROXY", proxy);
    }
}

async fn wait_until_ready(
    child: &mut Child,
    health_url_file: &Path,
    stop_requested: &AtomicBool,
) -> ChadexResult<()> {
    let started = tokio::time::Instant::now();
    let deadline = started + READY_TIMEOUT;
    let client = Client::builder()
        .connect_timeout(READY_PROBE_TIMEOUT)
        .timeout(READY_PROBE_TIMEOUT)
        .no_proxy()
        .build()
        .map_err(|_| tunnel_runtime_error("Could not initialize the Tunnel readiness probe"))?;
    let mut health_base = None;
    loop {
        if stop_requested.load(Ordering::SeqCst) {
            return Err(cancelled_error());
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|_| tunnel_runtime_error("OpenAI tunnel-client could not be supervised"))?
        {
            return Err(tunnel_runtime_error(&format!(
                "OpenAI tunnel-client exited before becoming ready ({status})"
            )));
        }
        if health_base.is_none() && health_url_file.is_file() {
            health_base = read_health_url(health_url_file).ok();
        }
        if let Some(base) = health_base.as_ref() {
            if let Ok(response) = client.get(format!("{base}/readyz")).send().await {
                if response.status().is_success() {
                    return Ok(());
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(tunnel_runtime_error(
                "OpenAI Secure MCP Tunnel did not become ready before the startup timeout",
            ));
        }
        tokio::time::sleep(readiness_probe_interval(started.elapsed())).await;
    }
}

fn readiness_probe_interval(elapsed: Duration) -> Duration {
    if elapsed < READY_FAST_PROBE_WINDOW {
        READY_FAST_PROBE_INTERVAL
    } else {
        READY_STEADY_PROBE_INTERVAL
    }
}

fn read_health_url(path: &Path) -> ChadexResult<String> {
    let value = fs::read_to_string(path)
        .map_err(|_| tunnel_runtime_error("Tunnel health URL is unavailable"))?;
    let value = value.trim();
    let parsed =
        url::Url::parse(value).map_err(|_| tunnel_runtime_error("Tunnel health URL is invalid"))?;
    let host = parsed.host_str().unwrap_or("");
    if parsed.scheme() != "http"
        || !matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
        || parsed.port().is_none()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(tunnel_runtime_error(
            "Tunnel health URL is not loopback HTTP",
        ));
    }
    Ok(value.trim_end_matches('/').to_string())
}

struct TunnelClientAsset {
    target: &'static str,
    file_name: &'static str,
    archive_sha256: &'static str,
    binary_sha256: &'static str,
}

fn tunnel_client_asset() -> ChadexResult<TunnelClientAsset> {
    match std::env::consts::ARCH {
        "aarch64" => Ok(TunnelClientAsset {
            target: "darwin-arm64",
            file_name: "tunnel-client-v0.0.12-darwin-arm64.zip",
            archive_sha256: "42fb3138dc9c081d5777cb7e8bd1e041cc48b67c4978dbab3c5167ca1aabca02",
            binary_sha256: "b1757220cf4722cec9085ee4a908cf0ee4c1a499a33bd99979b9a9c7669e29b1",
        }),
        "x86_64" => Ok(TunnelClientAsset {
            target: "darwin-amd64",
            file_name: "tunnel-client-v0.0.12-darwin-amd64.zip",
            archive_sha256: "33de53aec680faafedc795f8f8268d6861577bddb871cb2d49529c91f88c2009",
            binary_sha256: "4133dab2575223252732a998210c34b7ed96a51765cf5ea835a8e24cf2be1272",
        }),
        other => Err(ChadexError::new(
            "tunnel_unavailable",
            format!("Chadex does not support OpenAI tunnel-client on macOS/{other}"),
            "Use an Apple Silicon or Intel Mac supported by Chadex.",
        )),
    }
}

async fn resolve_tunnel_client(root: &Path) -> ChadexResult<PathBuf> {
    if let Some(override_path) = std::env::var_os("CHADEX_TUNNEL_CLIENT_BIN").map(PathBuf::from) {
        verify_tunnel_client(&override_path).await?;
        return Ok(override_path);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join("tunnel-client");
            if candidate.is_file() && verify_tunnel_client(&candidate).await.is_ok() {
                return Ok(candidate);
            }
        }
    }
    let asset = tunnel_client_asset()?;
    let destination = root
        .join("tools")
        .join("tunnel-client")
        .join(TUNNEL_CLIENT_VERSION)
        .join(asset.target)
        .join("tunnel-client");
    if destination.is_file()
        && sha256_file(&destination).ok().as_deref() == Some(asset.binary_sha256)
        && verify_tunnel_client(&destination).await.is_ok()
    {
        return Ok(destination);
    }
    install_tunnel_client(root, &asset, &destination).await?;
    Ok(destination)
}

async fn install_tunnel_client(
    root: &Path,
    asset: &TunnelClientAsset,
    destination: &Path,
) -> ChadexResult<()> {
    let install_dir = destination
        .parent()
        .ok_or_else(|| tunnel_runtime_error("Invalid managed Tunnel client path"))?;
    create_private_dir(install_dir)?;
    let temporary = root
        .join("tools")
        .join(format!(".tunnel-install-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temporary);
    create_private_dir(&temporary)?;
    let archive = temporary.join(asset.file_name);
    let result = async {
        let url = format!("{RELEASE_BASE}/{}", asset.file_name);
        let response = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| tunnel_runtime_error("Could not initialize the Tunnel client downloader"))?
            .get(url)
            .header("User-Agent", "chadex/0.2.2")
            .send()
            .await
            .map_err(|_| tunnel_runtime_error("Could not download OpenAI tunnel-client"))?;
        if !response.status().is_success() {
            return Err(tunnel_runtime_error("OpenAI tunnel-client download failed"));
        }
        let bytes = bounded_download(response, MAX_DOWNLOAD_BYTES).await?;
        write_private_file(&archive, &bytes)?;
        verify_sha256(&archive, asset.archive_sha256)?;
        let candidate = temporary.join("tunnel-client");
        extract_tunnel_client(&archive, &candidate)?;
        verify_sha256(&candidate, asset.binary_sha256)?;
        make_private_executable(&candidate)?;
        verify_tunnel_client(&candidate).await?;
        fs::rename(&candidate, destination)
            .map_err(|_| tunnel_runtime_error("Could not install OpenAI tunnel-client"))?;
        Ok(())
    }
    .await;
    let _ = fs::remove_dir_all(&temporary);
    result
}

async fn bounded_download(mut response: reqwest::Response, limit: usize) -> ChadexResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(tunnel_runtime_error(
            "Tunnel client download exceeded the safety limit",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| tunnel_runtime_error("Could not read OpenAI tunnel-client download"))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(tunnel_runtime_error(
                "Tunnel client download exceeded the safety limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn extract_tunnel_client(archive_path: &Path, destination: &Path) -> ChadexResult<()> {
    let file = File::open(archive_path)
        .map_err(|_| tunnel_runtime_error("Could not open Tunnel client archive"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|_| tunnel_runtime_error("Tunnel client archive is invalid"))?;
    let entry = archive
        .by_name("tunnel-client")
        .map_err(|_| tunnel_runtime_error("Tunnel client archive is missing its executable"))?;
    if !entry.is_file() || entry.size() > MAX_BINARY_BYTES {
        return Err(tunnel_runtime_error(
            "Tunnel client archive member is invalid",
        ));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .take(MAX_BINARY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| tunnel_runtime_error("Could not extract Tunnel client"))?;
    if bytes.len() as u64 > MAX_BINARY_BYTES {
        return Err(tunnel_runtime_error(
            "Tunnel client executable exceeded the safety limit",
        ));
    }
    write_private_file(destination, &bytes)
}

fn tunnel_client_file_identity(path: &Path) -> ChadexResult<TunnelClientFileIdentity> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| tunnel_runtime_error("Tunnel client is unavailable"))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_BINARY_BYTES {
        return Err(tunnel_runtime_error("Tunnel client is invalid"));
    }
    Ok(TunnelClientFileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        length: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    })
}

async fn verify_tunnel_client(path: &Path) -> ChadexResult<()> {
    let _ = tunnel_client_file_identity(path)?;
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        Command::new(path).arg("--version").output(),
    )
    .await
    .map_err(|_| tunnel_runtime_error("Tunnel client version check timed out"))?
    .map_err(|_| tunnel_runtime_error("Tunnel client version check failed"))?;
    let version = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() && version.starts_with(TUNNEL_CLIENT_VERSION) {
        Ok(())
    } else {
        Err(tunnel_runtime_error(
            "Tunnel client version does not match Chadex's pinned version",
        ))
    }
}

fn sha256_file(path: &Path) -> ChadexResult<String> {
    let mut file =
        File::open(path).map_err(|_| tunnel_runtime_error("Could not verify Tunnel client"))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| tunnel_runtime_error("Could not verify Tunnel client"))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify_sha256(path: &Path, expected: &str) -> ChadexResult<()> {
    if sha256_file(path)?.as_str() == expected {
        Ok(())
    } else {
        Err(tunnel_runtime_error(
            "Tunnel client integrity verification failed",
        ))
    }
}

fn create_private_dir(path: &Path) -> ChadexResult<()> {
    fs::create_dir_all(path)
        .map_err(|_| tunnel_runtime_error("Could not create Chadex private Tunnel state"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| tunnel_runtime_error("Could not protect Chadex private Tunnel state"))?;
    }
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> ChadexResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| tunnel_runtime_error("Could not write Chadex private Tunnel state"))?;
        file.write_all(bytes)
            .map_err(|_| tunnel_runtime_error("Could not write Chadex private Tunnel state"))?;
        return Ok(());
    }
    #[allow(unreachable_code)]
    Err(tunnel_runtime_error("Unsupported platform"))
}

fn make_private_executable(path: &Path) -> ChadexResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| tunnel_runtime_error("Could not make Tunnel client executable"))?;
    }
    Ok(())
}

fn tunnel_runtime_error(message: &str) -> ChadexError {
    ChadexError::new(
        "tunnel_unavailable",
        message,
        "Check the Tunnel ID, restricted API key, network access, and local MCP runtime, then retry.",
    )
}

fn cancelled_error() -> ChadexError {
    ChadexError::new(
        "tunnel_cancelled",
        "Secure MCP Tunnel startup was cancelled",
        "Start the connection again when ready.",
    )
}

#[cfg(test)]
mod download_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_token_parser_never_needs_to_persist_the_secret() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join("server.env");
        fs::write(
            &env,
            "RUNTIME_ADDR=127.0.0.1:12345\nCHADEX_TEST_BOOTSTRAP_TOKEN=wc_test_secret\n",
        )
        .unwrap();
        let token = read_bootstrap_token(&env, "CHADEX_TEST_BOOTSTRAP_TOKEN").unwrap();
        assert_eq!(token.as_str(), "wc_test_secret");
    }

    #[test]
    fn runtime_target_must_be_loopback_http() {
        assert!(validate_loopback_server_url("http://127.0.0.1:12345").is_ok());
        assert!(validate_loopback_server_url("https://example.com").is_err());
        assert!(validate_loopback_server_url("http://0.0.0.0:12345").is_err());
    }

    #[test]
    fn managed_asset_is_pinned_for_supported_mac_architecture() {
        let asset = tunnel_client_asset().unwrap();
        assert!(asset.target.starts_with("darwin-"));
        assert_eq!(asset.binary_sha256.len(), 64);
        assert_eq!(asset.archive_sha256.len(), 64);
    }

    #[test]
    fn readiness_polling_is_front_loaded_then_returns_to_steady_cadence() {
        assert_eq!(
            readiness_probe_interval(Duration::from_millis(0)),
            Duration::from_millis(25)
        );
        assert_eq!(
            readiness_probe_interval(Duration::from_millis(999)),
            Duration::from_millis(25)
        );
        assert_eq!(
            readiness_probe_interval(Duration::from_secs(1)),
            Duration::from_millis(100)
        );
    }

    #[tokio::test]
    async fn stalled_ingress_drain_is_bounded_and_does_not_hold_tunnel_mutex() {
        let fixture = tempfile::tempdir().unwrap();
        let tracker = Arc::new(VerificationTracker::default());
        let performance = Arc::new(PerformanceTraceStore::default());
        let ingress = Arc::new(
            McpIngress::start(
                "http://127.0.0.1:9/mcp".to_string(),
                Arc::clone(&tracker),
                Arc::clone(&performance),
            )
            .await
            .unwrap(),
        );
        let held = ingress.hold_admission_for_test().await;
        let manager = Arc::new(TunnelManager::new(
            fixture.path().join("managed"),
            tracker,
            performance,
        ));
        manager.inner.lock().await.ingress = Some(Arc::clone(&ingress));

        let pausing = {
            let manager = Arc::clone(&manager);
            tokio::spawn(async move {
                manager
                    .pause_ingress_with_timeout(Duration::from_secs(1))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;

        let stopped = tokio::time::timeout(Duration::from_millis(200), manager.stop())
            .await
            .expect("stop must not wait for the ingress drain mutex")
            .unwrap();
        assert_eq!(stopped.state, TunnelState::Unconfigured);
        drop(held);
        assert!(pausing.await.unwrap().is_ok());

        // Reinstall the ingress and prove the drain itself is bounded too.
        manager.inner.lock().await.ingress = Some(Arc::clone(&ingress));
        let held = ingress.hold_admission_for_test().await;
        let error = manager
            .pause_ingress_with_timeout(Duration::from_millis(20))
            .await
            .unwrap_err();
        assert_eq!(error.code, "ingress_drain_timeout");
        drop(held);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn verified_tunnel_client_cache_reuses_unchanged_binary_and_invalidates_on_change() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir().unwrap();
        let fake_client = fixture.path().join("fake-tunnel-client");
        let version_calls = fixture.path().join("version-calls");
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  echo x >> \"{}\"\n  echo 0.0.12\n  exit 0\nfi\nexit 0\n",
            version_calls.display()
        );
        fs::write(&fake_client, &script).unwrap();
        fs::set_permissions(&fake_client, fs::Permissions::from_mode(0o700)).unwrap();

        let tracker = Arc::new(VerificationTracker::default());
        let performance = Arc::new(PerformanceTraceStore::default());
        let manager = TunnelManager::new_with_tunnel_client(
            fixture.path().join("managed"),
            tracker,
            performance,
            fake_client.clone(),
        );

        let first = manager.resolve_tunnel_client_for_start().await.unwrap();
        let second = manager.resolve_tunnel_client_for_start().await.unwrap();
        assert_eq!(first, fake_client);
        assert_eq!(second, fake_client);
        assert_eq!(
            fs::read_to_string(&version_calls).unwrap().lines().count(),
            1
        );

        let mut changed_script = script;
        changed_script.push_str("# changed\n");
        fs::write(&fake_client, changed_script).unwrap();
        let third = manager.resolve_tunnel_client_for_start().await.unwrap();
        assert_eq!(third, fake_client);
        assert_eq!(
            fs::read_to_string(&version_calls).unwrap().lines().count(),
            2
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn manager_owns_full_fake_tunnel_lifecycle() {
        use std::os::unix::fs::PermissionsExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let health_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let health_address = health_listener.local_addr().unwrap();
        let health_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = health_listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request).await;
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                        )
                        .await;
                });
            }
        });

        let fixture = tempfile::tempdir().unwrap();
        let fake_client = fixture.path().join("fake-tunnel-client");
        let script = format!(
            r#"#!/bin/sh
mode=""
for arg in "$@"; do
  case "$arg" in
    --version)
      echo "0.0.12"
      exit 0
      ;;
    doctor)
      exit 42
      ;;
    admin)
      exit 42
      ;;
    run)
      mode="run"
      ;;
  esac
done
if [ "$mode" = "run" ]; then
  previous=""
  health_file=""
  for arg in "$@"; do
    if [ "$previous" = "--health.url-file" ]; then
      health_file="$arg"
      break
    fi
    previous="$arg"
  done
  if [ -z "$health_file" ]; then
    exit 2
  fi
  printf '%s' 'http://{health_address}' > "$health_file"
  exec /usr/bin/tail -f /dev/null
fi
exit 3
"#
        );
        fs::write(&fake_client, script).unwrap();
        fs::set_permissions(&fake_client, fs::Permissions::from_mode(0o700)).unwrap();

        let server_env = fixture.path().join("server.env");
        fs::write(
            &server_env,
            "RUNTIME_ADDR=127.0.0.1:43123\nCHADEX_TEST_BOOTSTRAP_TOKEN=wc_fake_bootstrap\n",
        )
        .unwrap();

        let tracker = Arc::new(VerificationTracker::default());
        tracker.reset(Some("/tmp/chadex-test-project".to_string()));
        let performance = Arc::new(PerformanceTraceStore::default());
        let managed_root = fixture.path().join("managed");
        let manager = TunnelManager::new_with_tunnel_client(
            managed_root.clone(),
            tracker,
            performance,
            fake_client,
        );
        manager
            .provide_credentials(
                "tunnel_00000000000000000000000000000000",
                Zeroizing::new("sk-test-only-not-real".to_string()),
            )
            .await
            .unwrap();

        let ready = manager
            .start(
                RuntimeTunnelTarget {
                    server_url: "http://127.0.0.1:43123".to_string(),
                    server_env_file: server_env,
                    bootstrap_token_env: "CHADEX_TEST_BOOTSTRAP_TOKEN".to_string(),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(ready.state, TunnelState::Ready);
        assert!(ready.configured);

        manager.publish(TunnelSnapshot {
            state: TunnelState::Stopped,
            configured: true,
            tunnel_id: ready.tunnel_id.clone(),
            epoch: ready.epoch,
            last_error: None,
        });
        assert_eq!(manager.snapshot().state, TunnelState::Stopped);
        let reconciled = manager.refresh().await;
        assert_eq!(reconciled.state, TunnelState::Ready);

        let session_root = managed_root.join("tunnel-sessions");
        let session_entries = fs::read_dir(&session_root).unwrap().count();
        assert_eq!(session_entries, 1);

        let stopped = manager.stop().await.unwrap();
        assert_eq!(stopped.state, TunnelState::Stopped);
        assert!(stopped.configured);
        assert!(fs::read_dir(&session_root)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true));

        health_task.abort();
    }
}

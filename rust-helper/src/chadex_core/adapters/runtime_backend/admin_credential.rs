//! Local-only Desktop admin credential.
//!
//! Skill management (`skill_inventory`, `skill_versions`, `skill_install`,
//! `skill_activate`, `skill_deactivate`, `skill_remove_revision`), the admin
//! Memory tools (`memory_scope_list`, `memory_scope_purge`) and, by product
//! decision, the Desktop's Project Memory tools (`memory_search`, `memory_read`,
//! `memory_set`, `memory_delete`: the pairing user token lacks the `memory:*`
//! scopes they need) all go through the runtime `admin` scope. The pairing user token deliberately does not carry it, and the
//! bootstrap token is what the ChatGPT tunnel uses, so the helper keeps a third,
//! separate credential: a managed `wc_pat_*` token with only the `admin` scope.
//!
//! The token is generated here, only its SHA-256 is registered with the local
//! Server (using the bootstrap token from the private server env file), and the
//! plaintext lives solely in an owner-only file beside that env file. It is
//! never put in the tunnel authorization file, returned over the helper
//! protocol, written to the saved desktop config (only its path is) or logged.
//!
//! Creation is lazy (first admin call) and self-healing: a missing file or a 401
//! from the Server mints a replacement and revokes older tokens of the same
//! name, so repeated failures do not accumulate usable admin tokens.

use super::{bounded_json_response, local_runtime_url, read_probe_token};
use crate::chadex_core::tunnel::{read_bootstrap_token, write_private_secret_file_atomic};
use crate::chadex_core::{ChadexError, ChadexResult};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

pub(super) const ADMIN_TOKEN_FILE_NAME: &str = "chadex-desktop-admin-token";
const ADMIN_TOKEN_NAME: &str = "chadex-desktop-admin";
const ADMIN_TOKEN_PREFIX: &str = "wc_pat_";
const BOOTSTRAP_TOKEN_ENV: &str = "WEBCODEX_TOKEN";
const ADMIN_SCOPE: &str = "admin";
/// Lifetime of a minted token. After it lapses the Server answers 401 and the
/// normal 401 -> re-mint path rotates it.
const ADMIN_TOKEN_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How long a persistent failure that has side effects (a registration that
/// could not be stored, or a fresh token that was rejected) fails fast instead
/// of minting and revoking again on every call.
const FAILURE_BACKOFF: Duration = Duration::from_secs(60);

/// Tools that need the admin credential. Every other Desktop call keeps using
/// the pairing user token. Mirrors the `ToolOperatorExtensionFamily::
/// SkillManagement` tools and the Memory tools in
/// `runtime-engine/crates/chadex-runtime-tool-contracts`.
const ADMIN_TOOLS: &[&str] = &[
    "skill_inventory",
    "skill_versions",
    "skill_install",
    "skill_activate",
    "skill_deactivate",
    "skill_remove_revision",
    "memory_scope_list",
    "memory_scope_purge",
    "memory_search",
    "memory_read",
    "memory_set",
    "memory_delete",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolCredential {
    /// The pairing user token (`wc_pat_*` named `chatgpt-action`, no admin).
    User,
    /// The separate local admin token managed by this module.
    Admin,
}

pub(super) fn tool_credential(tool_name: &str) -> ToolCredential {
    if ADMIN_TOOLS.contains(&tool_name) {
        ToolCredential::Admin
    } else {
        ToolCredential::User
    }
}

/// Everything needed to find, mint and use the admin token. Paths and the
/// Server URL only; no secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AdminCredentialSource {
    pub server_url: String,
    pub server_env_file: PathBuf,
    pub admin_token_file: PathBuf,
    pub username: String,
}

/// The admin token path: always the fixed file name beside the server env
/// file. A path recorded in the saved config is never trusted for reads or
/// writes (it could name `webcodex.env` or the user token file); it is only a
/// breadcrumb.
pub(super) fn admin_token_path(server_env_file: &Path) -> Option<PathBuf> {
    let directory = server_env_file
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())?;
    let path = directory.join(ADMIN_TOKEN_FILE_NAME);
    (path != server_env_file).then_some(path)
}

pub(super) struct AdminCredentialStore {
    /// Serialises mint/rotate so concurrent first calls register one token and
    /// a stale-token retry cannot revoke a token another call just minted.
    mint_lock: Mutex<()>,
    backoff: Duration,
    failure: StdMutex<Option<(Instant, ChadexError)>>,
}

impl Default for AdminCredentialStore {
    fn default() -> Self {
        Self::with_backoff(FAILURE_BACKOFF)
    }
}

impl AdminCredentialStore {
    fn with_backoff(backoff: Duration) -> Self {
        Self {
            mint_lock: Mutex::new(()),
            backoff,
            failure: StdMutex::new(None),
        }
    }

    /// The remembered persistent failure, while it is still inside the backoff
    /// window. Lets a broken setup fail fast with the same stable error instead
    /// of minting and revoking a token on every call.
    pub(super) fn recent_failure(&self) -> Option<ChadexError> {
        let mut failure = self.failure.lock().unwrap_or_else(|p| p.into_inner());
        match failure.as_ref() {
            Some((at, error)) if at.elapsed() < self.backoff => Some(error.clone()),
            Some(_) => {
                *failure = None;
                None
            }
            None => None,
        }
    }

    pub(super) fn record_failure(&self, error: &ChadexError) {
        *self.failure.lock().unwrap_or_else(|p| p.into_inner()) =
            Some((Instant::now(), error.clone()));
    }

    pub(super) fn record_success(&self) {
        *self.failure.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// The current admin token, creating it when needed. `rejected` is a token
    /// the Server just answered 401 to: it is never returned again, and if the
    /// file still holds it a replacement is minted.
    pub(super) async fn token(
        &self,
        client: &Client,
        source: &AdminCredentialSource,
        rejected: Option<&str>,
    ) -> ChadexResult<Zeroizing<String>> {
        let _guard = self.mint_lock.lock().await;
        if let Some(existing) = read_admin_token(&source.admin_token_file).await {
            if Some(existing.as_str()) != rejected {
                return Ok(existing);
            }
        }
        if let Some(error) = self.recent_failure() {
            return Err(error);
        }
        let minted = mint_admin_token(client, source).await;
        if let Err(error) = &minted {
            // Only failures that already touched the Server's token table
            // back off; an unreachable Server or a missing bootstrap changes
            // nothing and must recover the moment the runtime is ready.
            if matches!(
                error
                    .details
                    .as_ref()
                    .and_then(|details| details["reason"].as_str()),
                Some("register_rejected" | "write_failed")
            ) {
                self.record_failure(error);
            }
        }
        minted
    }
}

async fn read_admin_token(path: &Path) -> Option<Zeroizing<String>> {
    // A token file readable by group/others may already have been read by
    // someone else: treat it as unusable so it is replaced (and revoked).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = tokio::fs::symlink_metadata(path).await.ok()?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    let token = read_probe_token(path).await?;
    (token.starts_with(ADMIN_TOKEN_PREFIX) && token.is_ascii()).then_some(token)
}

async fn mint_admin_token(
    client: &Client,
    source: &AdminCredentialSource,
) -> ChadexResult<Zeroizing<String>> {
    let bootstrap = read_bootstrap_token(&source.server_env_file, BOOTSTRAP_TOKEN_ENV)
        .map_err(|_| unavailable("bootstrap_unavailable", None))?;
    let token = generate_admin_token()?;
    let hash = format!("{:x}", sha2::Sha256::digest(token.as_bytes()));
    let prefix: String = token.chars().take(16).collect();
    let expires_at = (SystemTime::now() + ADMIN_TOKEN_TTL)
        .duration_since(UNIX_EPOCH)
        .map_err(|_| unavailable("clock_unavailable", None))?
        .as_secs();
    let registered = post_json(
        client,
        &source.server_url,
        "/api/tokens/register_hash",
        bootstrap.as_str(),
        json!({
            "username": source.username,
            "name": ADMIN_TOKEN_NAME,
            "token_hash": format!("sha256:{hash}"),
            "token_prefix": prefix,
            "scopes": [ADMIN_SCOPE],
            "expires_at": expires_at,
        }),
    )
    .await;
    let (status, body) = match registered {
        Some(response) => response,
        None => return Err(unavailable("runtime_unreachable", None)),
    };
    if !(200..300).contains(&status) {
        return Err(unavailable("register_rejected", Some(status)));
    }
    let new_id = body
        .as_ref()
        .and_then(|value| value.pointer("/token/id"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let path = source.admin_token_file.clone();
    let contents = Zeroizing::new(format!("{}\n", token.as_str()));
    let written = tokio::task::spawn_blocking(move || {
        write_private_secret_file_atomic(&path, contents.as_bytes())
    })
    .await;
    if !matches!(written, Ok(Ok(()))) {
        // The plaintext was not stored, so the registration is unusable.
        if let Some(id) = new_id.as_deref() {
            revoke_token(client, source, bootstrap.as_str(), id).await;
        }
        return Err(unavailable("write_failed", None));
    }
    revoke_stale_admin_tokens(client, source, bootstrap.as_str(), new_id.as_deref()).await;
    Ok(token)
}

/// Best effort: older tokens with this name are superseded by the one just
/// minted. A failure here only leaves an extra registered token behind.
async fn revoke_stale_admin_tokens(
    client: &Client,
    source: &AdminCredentialSource,
    bootstrap: &str,
    keep_id: Option<&str>,
) {
    let Some(keep_id) = keep_id else { return };
    let Some((status, Some(body))) = post_json(
        client,
        &source.server_url,
        "/api/tokens/list",
        bootstrap,
        json!({"username": source.username}),
    )
    .await
    else {
        return;
    };
    if !(200..300).contains(&status) {
        return;
    }
    let stale: Vec<String> = body
        .get("tokens")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|token| {
            token.get("name").and_then(Value::as_str) == Some(ADMIN_TOKEN_NAME)
                && token.get("scopes") == Some(&json!([ADMIN_SCOPE]))
                && token.get("revoked_at").is_none_or(Value::is_null)
                && token.get("id").and_then(Value::as_str) != Some(keep_id)
        })
        .filter_map(|token| token.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();
    for id in stale {
        revoke_token(client, source, bootstrap, &id).await;
    }
}

async fn revoke_token(client: &Client, source: &AdminCredentialSource, bootstrap: &str, id: &str) {
    let _ = post_json(
        client,
        &source.server_url,
        "/api/tokens/revoke",
        bootstrap,
        json!({"username": source.username, "token_id": id}),
    )
    .await;
}

async fn post_json(
    client: &Client,
    server_url: &str,
    endpoint: &str,
    bearer: &str,
    body: Value,
) -> Option<(u16, Option<Value>)> {
    let url = local_runtime_url(server_url, endpoint)?;
    let response = client
        .post(url)
        .bearer_auth(bearer)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&body).ok()?)
        .send()
        .await
        .ok()?;
    let status = response.status().as_u16();
    Some((status, bounded_json_response(response).await))
}

fn generate_admin_token() -> ChadexResult<Zeroizing<String>> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(bytes.as_mut_slice()).map_err(|_| unavailable("entropy_unavailable", None))?;
    let mut token = Zeroizing::new(String::with_capacity(ADMIN_TOKEN_PREFIX.len() + 64));
    token.push_str(ADMIN_TOKEN_PREFIX);
    for byte in bytes.iter() {
        token.push_str(&format!("{byte:02x}"));
    }
    Ok(token)
}

fn unavailable(reason: &'static str, status: Option<u16>) -> ChadexError {
    let mut details = json!({ "reason": reason });
    if let Some(status) = status {
        details["status"] = json!(status);
    }
    ChadexError::new(
        "skill_management_credential_unavailable",
        "Chadex could not prepare the local administrator credential for Skills and Project Memory",
        "Restart the local runtime from Chadex, then retry.",
    )
    .with_details(details)
}

pub(super) fn credential_rejected_error() -> ChadexError {
    ChadexError::new(
        "skill_management_credential_rejected",
        "The local runtime rejected Chadex's administrator credential",
        "Restart the local runtime from Chadex, then retry.",
    )
}

pub(super) fn requires_local_runtime_error() -> ChadexError {
    ChadexError::new(
        "skill_management_requires_local_runtime",
        "Managing Skills and Project Memory requires the local runtime on this computer",
        "Connect Chadex to a local runtime to manage Skills and Project Memory; a remote Server cannot be managed from here.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Json, State},
        http::{HeaderMap, StatusCode},
        routing::post,
        Router,
    };
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;

    const BOOTSTRAP: &str = "wc_boot_test_bootstrap";

    #[derive(Default)]
    struct FakeServer {
        registered: Vec<Value>,
        listed: Vec<Value>,
        revoked: Vec<String>,
        bearers: Vec<(String, String)>,
        register_status: Option<u16>,
    }

    type Shared = Arc<StdMutex<FakeServer>>;

    fn bearer(headers: &HeaderMap) -> String {
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    async fn spawn_server() -> (String, Shared, tokio::task::JoinHandle<()>) {
        let state: Shared = Arc::default();
        let router = Router::new()
            .route(
                "/api/tokens/register_hash",
                post(
                    |State(state): State<Shared>, headers: HeaderMap, Json(body): Json<Value>| async move {
                        let mut server = state.lock().unwrap();
                        server
                            .bearers
                            .push(("register_hash".into(), bearer(&headers)));
                        if let Some(status) = server.register_status {
                            return (StatusCode::from_u16(status).unwrap(), Json(json!({})));
                        }
                        let id = format!("tok-{}", server.registered.len() + 1);
                        server.listed.push(json!({
                            "id": id, "name": body["name"], "scopes": body["scopes"],
                            "revoked_at": null
                        }));
                        server.registered.push(body);
                        (StatusCode::OK, Json(json!({"success": true, "token": {"id": id}})))
                    },
                ),
            )
            .route(
                "/api/tokens/list",
                post(|State(state): State<Shared>, headers: HeaderMap| async move {
                    let mut server = state.lock().unwrap();
                    server.bearers.push(("list".into(), bearer(&headers)));
                    Json(json!({"success": true, "tokens": server.listed.clone()}))
                }),
            )
            .route(
                "/api/tokens/revoke",
                post(
                    |State(state): State<Shared>, headers: HeaderMap, Json(body): Json<Value>| async move {
                        let mut server = state.lock().unwrap();
                        server.bearers.push(("revoke".into(), bearer(&headers)));
                        let id = body["token_id"].as_str().unwrap().to_string();
                        for token in server.listed.iter_mut() {
                            if token["id"] == id.as_str() {
                                token["revoked_at"] = json!(1);
                            }
                        }
                        server.revoked.push(id);
                        Json(json!({"success": true}))
                    },
                ),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (url, state, handle)
    }

    fn client() -> Client {
        Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap()
    }

    fn source(dir: &Path, url: &str) -> AdminCredentialSource {
        let env = dir.join("webcodex.env");
        std::fs::write(&env, format!("OTHER=1\n{BOOTSTRAP_TOKEN_ENV}={BOOTSTRAP}\n")).unwrap();
        AdminCredentialSource {
            server_url: url.to_string(),
            server_env_file: env.clone(),
            admin_token_file: admin_token_path(&env).unwrap(),
            username: "tester".to_string(),
        }
    }

    #[test]
    fn admin_credential_is_selected_only_for_skill_management_and_admin_memory() {
        for tool in [
            "skill_inventory",
            "skill_versions",
            "skill_install",
            "skill_activate",
            "skill_deactivate",
            "skill_remove_revision",
            "memory_scope_list",
            "memory_scope_purge",
            "memory_search",
            "memory_read",
            "memory_set",
            "memory_delete",
        ] {
            assert_eq!(tool_credential(tool), ToolCredential::Admin, "{tool}");
        }
        for tool in [
            "skill_list",
            "skill_read_file",
            "skill_load",
            "cancel_task",
            "list_runners",
            "list_jobs",
            "write_file",
        ] {
            assert_eq!(tool_credential(tool), ToolCredential::User, "{tool}");
        }
    }

    #[test]
    fn admin_token_path_is_always_the_fixed_file_beside_the_server_env_file() {
        let env = Path::new("/data/runtime/local/webcodex.env");
        assert_eq!(
            admin_token_path(env),
            Some(PathBuf::from("/data/runtime/local").join(ADMIN_TOKEN_FILE_NAME))
        );
        assert_eq!(admin_token_path(Path::new("webcodex.env")), None);
        // Never the env file itself, even if it were given the token's name.
        let odd = Path::new("/data/runtime/local").join(ADMIN_TOKEN_FILE_NAME);
        assert_eq!(admin_token_path(&odd), None);
    }

    #[test]
    fn generated_tokens_match_the_managed_token_shape() {
        let first = generate_admin_token().unwrap();
        let second = generate_admin_token().unwrap();
        assert_ne!(first.as_str(), second.as_str());
        let body = first.strip_prefix("wc_pat_").unwrap();
        assert_eq!(body.len(), 64);
        assert!(body.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn first_use_mints_registers_only_the_hash_and_reuses_the_file() {
        let (url, state, server) = spawn_server().await;
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path(), &url);
        let store = AdminCredentialStore::default();

        let token = store.token(&client(), &source, None).await.unwrap();
        assert!(token.starts_with("wc_pat_"));
        let again = store.token(&client(), &source, None).await.unwrap();
        assert_eq!(token.as_str(), again.as_str());

        let server_state = state.lock().unwrap();
        assert_eq!(server_state.registered.len(), 1, "valid file must be reused");
        let request = &server_state.registered[0];
        assert_eq!(request["scopes"], json!(["admin"]));
        assert_eq!(request["name"], "chadex-desktop-admin");
        assert_eq!(request["username"], "tester");
        let expires = request["expires_at"].as_u64().expect("expires_at is sent");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let ttl = ADMIN_TOKEN_TTL.as_secs();
        assert!(expires + 60 >= now + ttl && expires <= now + ttl + 60, "{expires}");
        let expected_hash = format!("sha256:{:x}", sha2::Sha256::digest(token.as_bytes()));
        assert_eq!(request["token_hash"], expected_hash.as_str());
        assert_eq!(request["token_prefix"], &token[..16]);
        // The plaintext token is never sent; registration uses the bootstrap.
        assert!(!request.to_string().contains(token.as_str()));
        assert_eq!(
            server_state.bearers[0],
            ("register_hash".into(), format!("Bearer {BOOTSTRAP}"))
        );
        drop(server_state);
        server.abort();
    }

    #[tokio::test]
    async fn rejected_token_is_replaced_once_and_older_registrations_are_revoked() {
        let (url, state, server) = spawn_server().await;
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path(), &url);
        let store = AdminCredentialStore::default();

        let first = store.token(&client(), &source, None).await.unwrap();
        let second = store
            .token(&client(), &source, Some(first.as_str()))
            .await
            .unwrap();
        assert_ne!(first.as_str(), second.as_str());
        assert_eq!(
            read_admin_token(&source.admin_token_file)
                .await
                .unwrap()
                .as_str(),
            second.as_str()
        );
        // A caller still holding the old rejected token gets the file's new one
        // instead of triggering a third registration.
        let third = store
            .token(&client(), &source, Some(first.as_str()))
            .await
            .unwrap();
        assert_eq!(third.as_str(), second.as_str());

        let server_state = state.lock().unwrap();
        assert_eq!(server_state.registered.len(), 2);
        assert_eq!(server_state.revoked, vec!["tok-1".to_string()]);
        // Revocation also authenticates with the bootstrap, never the admin token.
        assert!(server_state
            .bearers
            .iter()
            .all(|(_, bearer)| bearer == &format!("Bearer {BOOTSTRAP}")));
        drop(server_state);
        server.abort();
    }

    #[tokio::test]
    async fn missing_or_invalid_file_triggers_a_fresh_mint() {
        let (url, state, server) = spawn_server().await;
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path(), &url);
        let store = AdminCredentialStore::default();

        std::fs::write(&source.admin_token_file, "not-a-managed-token\n").unwrap();
        let minted = store.token(&client(), &source, None).await.unwrap();
        assert!(minted.starts_with("wc_pat_"));
        assert_eq!(state.lock().unwrap().registered.len(), 1);

        std::fs::remove_file(&source.admin_token_file).unwrap();
        let remint = store.token(&client(), &source, None).await.unwrap();
        assert_ne!(minted.as_str(), remint.as_str());
        let server_state = state.lock().unwrap();
        assert_eq!(server_state.registered.len(), 2);
        assert_eq!(server_state.revoked, vec!["tok-1".to_string()]);
        drop(server_state);
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn admin_token_file_is_owner_only_and_symlinks_are_not_followed() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (url, _state, server) = spawn_server().await;
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path(), &url);
        let store = AdminCredentialStore::default();

        let token = store.token(&client(), &source, None).await.unwrap();
        let mode = std::fs::metadata(&source.admin_token_file)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(
            std::fs::read_to_string(&source.admin_token_file).unwrap(),
            format!("{}\n", token.as_str())
        );
        // No temporary files are left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        // A symlink at the token path is never read, and minting replaces the
        // link itself instead of writing through to its target.
        std::fs::remove_file(&source.admin_token_file).unwrap();
        let target = dir.path().join("attacker-owned");
        std::fs::write(&target, "wc_pat_attackerchosen\n").unwrap();
        symlink(&target, &source.admin_token_file).unwrap();
        assert!(read_admin_token(&source.admin_token_file).await.is_none());
        let replaced = store.token(&client(), &source, None).await.unwrap();
        assert_ne!(replaced.as_str(), "wc_pat_attackerchosen");
        assert!(!std::fs::symlink_metadata(&source.admin_token_file)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "wc_pat_attackerchosen\n"
        );
        server.abort();
    }

    #[tokio::test]
    async fn registration_failures_are_stable_errors_that_leave_no_token_file() {
        let (url, state, server) = spawn_server().await;
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path(), &url);
        let store = AdminCredentialStore::default();

        state.lock().unwrap().register_status = Some(403);
        let error = store.token(&client(), &source, None).await.unwrap_err();
        assert_eq!(error.code, "skill_management_credential_unavailable");
        assert_eq!(error.details.as_ref().unwrap()["status"], 403);
        assert!(!source.admin_token_file.exists());

        // Missing bootstrap credential (fresh store: the 403 above backs off).
        let store = AdminCredentialStore::default();
        std::fs::write(&source.server_env_file, "OTHER=1\n").unwrap();
        let error = store.token(&client(), &source, None).await.unwrap_err();
        assert_eq!(error.code, "skill_management_credential_unavailable");
        assert_eq!(error.details.as_ref().unwrap()["reason"], "bootstrap_unavailable");

        // Unreachable Server.
        let store = AdminCredentialStore::default();
        server.abort();
        let _ = server.await;
        std::fs::write(
            &source.server_env_file,
            format!("{BOOTSTRAP_TOKEN_ENV}={BOOTSTRAP}\n"),
        )
        .unwrap();
        let error = store.token(&client(), &source, None).await.unwrap_err();
        assert_eq!(error.details.as_ref().unwrap()["reason"], "runtime_unreachable");
        assert!(!source.admin_token_file.exists());
    }

    #[test]
    fn remote_topology_error_is_a_stable_machine_readable_code() {
        let error = requires_local_runtime_error();
        assert_eq!(error.code, "skill_management_requires_local_runtime");
        assert!(!error.message.is_empty());
        assert!(!error.recovery.is_empty());
    }

    /// Manual end-to-end check against a real local runtime started from this
    /// checkout. Run with `CHADEX_E2E_SERVER_URL` and `CHADEX_E2E_ENV_FILE`
    /// set (the env file holds `WEBCODEX_TOKEN`) and `-- --ignored`.
    #[tokio::test]
    #[ignore = "needs a running local runtime; see doc comment"]
    async fn real_runtime_accepts_the_minted_admin_token_and_denies_the_user_token() {
        use crate::chadex_core::runtime_compat::operation::{
            CancellationContext, CancellationSignal,
        };
        let url = std::env::var("CHADEX_E2E_SERVER_URL").unwrap();
        let env_file = PathBuf::from(std::env::var("CHADEX_E2E_ENV_FILE").unwrap());
        let dir = tempfile::tempdir().unwrap();
        let source = AdminCredentialSource {
            server_url: url.clone(),
            server_env_file: env_file.clone(),
            admin_token_file: dir.path().join(ADMIN_TOKEN_FILE_NAME),
            username: "e2e-user".to_string(),
        };
        let client = client();
        let bootstrap = read_bootstrap_token(&env_file, BOOTSTRAP_TOKEN_ENV).unwrap();
        let (status, _) = post_json(
            &client,
            &url,
            "/api/users/create",
            bootstrap.as_str(),
            json!({"username": "e2e-user"}),
        )
        .await
        .unwrap();
        assert!(status == 200 || status == 409, "create user: {status}");

        // A pairing-shaped user token: same scopes `pairing enroll` issues.
        let user_token = generate_admin_token().unwrap();
        let (status, _) = post_json(
            &client,
            &url,
            "/api/tokens/register_hash",
            bootstrap.as_str(),
            json!({
                "username": "e2e-user", "name": "chatgpt-action",
                "token_hash": format!("sha256:{:x}", sha2::Sha256::digest(user_token.as_bytes())),
                "token_prefix": user_token.chars().take(16).collect::<String>(),
                "scopes": ["runtime:read", "runner:manage", "session:collaborate",
                    "project:read", "project:write", "job:run"],
            }),
        )
        .await
        .unwrap();
        assert_eq!(status, 200);

        let store = AdminCredentialStore::default();
        let admin = store.token(&client, &source, None).await.unwrap();
        let cancellation =
            CancellationContext::new(CancellationSignal::new(), CancellationSignal::new());
        let call = |token: String| {
            let client = client.clone();
            let url = url.clone();
            let cancellation = cancellation.clone();
            async move {
                super::super::call_local_runtime_tool_with_status(
                    &client,
                    &url,
                    &token,
                    "skill_inventory",
                    json!({"project": "agent:none:none"}),
                    &[],
                    super::super::McpEra::Stateless2026,
                    &cancellation,
                )
                .await
                .unwrap()
            }
        };
        // The Server answers 403 "missing required scope: admin" for the user
        // token: no tool result, and not a 401 (so no re-mint is triggered).
        let (user_result, user_401) = call(user_token.to_string()).await;
        println!("user token -> unauthorized={user_401} {user_result:?}");
        assert!(user_result.is_none() && !user_401);
        let (admin_result, admin_401) = call(admin.to_string()).await;
        let admin_result = admin_result.expect("admin token reaches the tool gateway");
        println!("admin token -> unauthorized={admin_401} {admin_result:?}");
        assert!(!admin_401);
        assert!(!format!("{admin_result:?}").contains("missing required scope"));
        let (_, bogus_401) = call("wc_pat_not_registered".to_string()).await;
        assert!(bogus_401, "unknown token must answer 401");
    }

    #[tokio::test]
    async fn only_admin_scoped_tokens_with_our_name_are_revoked_on_rotation() {
        let (url, state, server) = spawn_server().await;
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path(), &url);
        let store = AdminCredentialStore::default();
        {
            let mut fake = state.lock().unwrap();
            // Same name but broader scopes, and a different name with admin.
            fake.listed.push(json!({"id": "other-1", "name": "chadex-desktop-admin",
                "scopes": ["admin", "project:read"], "revoked_at": null}));
            fake.listed.push(json!({"id": "other-2", "name": "someone-elses-admin",
                "scopes": ["admin"], "revoked_at": null}));
            fake.listed.push(json!({"id": "old-ours", "name": "chadex-desktop-admin",
                "scopes": ["admin"], "revoked_at": null}));
        }
        store.token(&client(), &source, None).await.unwrap();
        assert_eq!(state.lock().unwrap().revoked, vec!["old-ours".to_string()]);
        server.abort();
    }

    #[tokio::test]
    async fn persistent_registration_failure_backs_off_instead_of_hammering_the_server() {
        let (url, state, server) = spawn_server().await;
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path(), &url);
        let store = AdminCredentialStore::with_backoff(Duration::from_millis(300));
        state.lock().unwrap().register_status = Some(403);

        let first = store.token(&client(), &source, None).await.unwrap_err();
        let requests = state.lock().unwrap().bearers.len();
        assert_eq!(requests, 1);
        let second = store.token(&client(), &source, None).await.unwrap_err();
        assert_eq!(second.code, first.code);
        assert_eq!(state.lock().unwrap().bearers.len(), requests, "fail fast, no request");
        assert!(store.recent_failure().is_some());

        tokio::time::sleep(Duration::from_millis(350)).await;
        state.lock().unwrap().register_status = None;
        store.token(&client(), &source, None).await.unwrap();
        assert_eq!(state.lock().unwrap().registered.len(), 1);
        assert!(store.recent_failure().is_none());
        server.abort();
    }

    #[tokio::test]
    async fn unreachable_runtime_never_backs_off() {
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path(), "http://127.0.0.1:9");
        let store = AdminCredentialStore::with_backoff(Duration::from_secs(60));
        let error = store.token(&client(), &source, None).await.unwrap_err();
        assert_eq!(error.details.as_ref().unwrap()["reason"], "runtime_unreachable");
        assert!(store.recent_failure().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn token_file_readable_by_others_is_replaced_and_stale_temporaries_are_removed() {
        use std::os::unix::fs::PermissionsExt;
        let (url, state, server) = spawn_server().await;
        let dir = tempfile::tempdir().unwrap();
        let source = source(dir.path(), &url);
        let store = AdminCredentialStore::default();

        let loose = "wc_pat_looselypermissionedtoken\n";
        std::fs::write(&source.admin_token_file, loose).unwrap();
        std::fs::set_permissions(&source.admin_token_file, std::fs::Permissions::from_mode(0o644))
            .unwrap();
        let stale_tmp = dir.path().join(".chadex-desktop-admin-token.123-456.tmp");
        let unrelated_tmp = dir.path().join(".something-else.tmp");
        std::fs::write(&stale_tmp, "leftover").unwrap();
        std::fs::write(&unrelated_tmp, "keep").unwrap();

        let token = store.token(&client(), &source, None).await.unwrap();
        assert_ne!(token.as_str(), "wc_pat_looselypermissionedtoken");
        assert_eq!(state.lock().unwrap().registered.len(), 1);
        let mode = std::fs::metadata(&source.admin_token_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(!stale_tmp.exists());
        assert!(unrelated_tmp.exists());
        server.abort();
    }
}

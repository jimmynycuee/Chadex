//! `/api/*` Host allow-list (DNS-rebinding guard) and CORS origin policy,
//! exercised through the production router from `crate::build_http_router`.

use std::sync::Arc;

use salvo::prelude::*;
use salvo::test::{ResponseExt, TestClient};

use super::cors_origin_allowed;
use crate::route_metadata::{iter_routes, RouteMethod};
use crate::test_support::TestEnvGuard;

const UNTRUSTED: &str = "untrusted_request_authority";

fn config(addr: &str, token: Option<&str>) -> Arc<crate::Config> {
    let mut config = (*crate::test_support::test_config(token)).clone();
    config.addr = addr.to_string();
    Arc::new(config)
}

struct ProductionService {
    _tmp: tempfile::TempDir,
    service: Service,
}

fn production_service(config: Arc<crate::Config>) -> ProductionService {
    let (tmp, db) = crate::test_support::test_db();
    let router = crate::build_http_router(crate::HttpRouterState {
        config,
        db,
        authorize_session_store: Arc::new(crate::oauth_http::AuthorizeSessionStore::new()),
        runner_registry: Arc::new(crate::runner_http::RunnerRegistry::default()),
        tool_runtime: Arc::new(crate::tool_runtime::ToolRuntime::new_for_tests()),
        project_auth: Arc::new(super::ProjectAuthState::default()),
        console_asset_source: Arc::new(crate::console_web::ConsoleAssetSource::Embedded),
        shutdown_coordinator: Arc::new(crate::server_shutdown::ShutdownCoordinator::default()),
    });
    ProductionService {
        _tmp: tmp,
        service: Service::new(router),
    }
}

/// Environment shared by every test here: no public URL, no anonymous mode,
/// unless a test sets them explicitly.
fn clean_env() -> TestEnvGuard {
    let mut env = TestEnvGuard::new();
    env.remove("WEBCODEX_PUBLIC_URL");
    env.remove("WEBCODEX_ALLOW_ANONYMOUS");
    env
}

fn api_specs() -> Vec<(RouteMethod, String)> {
    iter_routes()
        .filter(|spec| spec.path.starts_with("/api/"))
        .map(|spec| (spec.method, spec.path.replace("{tool_name}", "probe")))
        .collect()
}

/// Status and error `code` (if the body is JSON) for one request with `host`.
async fn probe(
    service: &Service,
    method: RouteMethod,
    path: &str,
    host: &str,
) -> (StatusCode, Option<String>) {
    let url = format!("http://127.0.0.1{path}");
    let request = match method {
        RouteMethod::Get => TestClient::get(url),
        RouteMethod::Post => TestClient::post(url).json(&serde_json::json!({})),
    };
    let mut response = request.add_header("host", host, true).send(service).await;
    let status = response.status_code.unwrap_or(StatusCode::OK);
    let code = response
        .take_json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| body.get("code").and_then(|code| code.as_str().map(str::to_string)));
    (status, code)
}

async fn assert_rejected(service: &Service, host: &str) {
    let (status, code) = probe(service, RouteMethod::Post, "/api/runtime/status", host).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "Host {host} must be rejected");
    assert_eq!(code.as_deref(), Some(UNTRUSTED), "Host {host}");
}

async fn assert_not_host_rejected(service: &Service, host: &str) {
    let (status, code) = probe(service, RouteMethod::Post, "/api/runtime/status", host).await;
    assert_ne!(code.as_deref(), Some(UNTRUSTED), "Host {host} must pass the guard");
    assert_ne!(status, StatusCode::BAD_REQUEST, "Host {host} must parse");
}

#[tokio::test]
async fn every_api_route_rejects_untrusted_host_on_loopback_bind() {
    let _env = clean_env();
    let server = production_service(config("127.0.0.1:8080", Some("test-token")));
    let specs = api_specs();
    assert!(specs.len() > 50, "route_metadata should list the /api surface");

    for (method, path) in &specs {
        let (status, code) = probe(&server.service, *method, path, "evil.example").await;
        assert_eq!(
            (status, code.as_deref()),
            (StatusCode::FORBIDDEN, Some(UNTRUSTED)),
            "{method:?} {path} must sit behind the /api Host guard"
        );
    }
    for (method, path) in &specs {
        let (_, code) = probe(&server.service, *method, path, "127.0.0.1:8080").await;
        assert_ne!(
            code.as_deref(),
            Some(UNTRUSTED),
            "{method:?} {path} must accept a loopback Host"
        );
    }
}

#[tokio::test]
async fn api_host_guard_rejects_rebinding_variants_with_json_error_body() {
    let _env = clean_env();
    let server = production_service(config("127.0.0.1:8080", Some("test-token")));

    for host in ["evil.example:8080", "127.0.0.1.evil.example", "localhost.evil.example"] {
        assert_rejected(&server.service, host).await;
    }
    let mut response = TestClient::post("http://127.0.0.1/api/runtime/status")
        .add_header("host", "evil.example", true)
        .add_header("x-forwarded-host", "localhost", true)
        .json(&serde_json::json!({}))
        .send(&server.service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::FORBIDDEN));
    let body: serde_json::Value = response.take_json().await.unwrap();
    assert_eq!(body["status"], 403, "body follows json_error");
    assert_eq!(body["code"], UNTRUSTED);
    assert!(body["error"].is_string());

    let (status, code) = probe(
        &server.service,
        RouteMethod::Post,
        "/api/runtime/status",
        "localhost:notaport",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code.as_deref(), Some("invalid_request_authority"));
}

#[tokio::test]
async fn api_accepts_loopback_hosts_on_loopback_binds() {
    let _env = clean_env();
    for addr in ["127.0.0.1:8080", "localhost:8080", "[::1]:8080"] {
        let server = production_service(config(addr, Some("test-token")));
        for host in [
            "localhost",
            "localhost:8080",
            "127.0.0.1",
            "127.0.0.1:8080",
            "[::1]",
            "[::1]:8080",
        ] {
            assert_not_host_rejected(&server.service, host).await;
        }
    }
}

#[tokio::test]
async fn api_accepts_configured_public_host_on_loopback_bind() {
    let mut env = clean_env();
    for public_url in [
        "https://share.example.test",
        "https://share.example.test/webcodex",
    ] {
        env.set("WEBCODEX_PUBLIC_URL", public_url);
        let server = production_service(config("127.0.0.1:8080", Some("test-token")));
        for host in ["share.example.test", "share.example.test:443"] {
            assert_not_host_rejected(&server.service, host).await;
        }
        assert_rejected(&server.service, "share.example.test:8443").await;
        assert_rejected(&server.service, "evil.example").await;
    }
}

#[tokio::test]
async fn api_host_guard_leaves_token_required_non_loopback_binds_unchanged() {
    let _env = clean_env();
    for addr in ["0.0.0.0:8080", "[::]:8080", "192.168.1.20:8080"] {
        let server = production_service(config(addr, Some("test-token")));
        for host in ["192.168.1.20:8080", "nas.local:8080"] {
            assert_not_host_rejected(&server.service, host).await;
        }
    }
}

#[tokio::test]
async fn api_host_guard_applies_to_non_loopback_binds_without_token_requirement() {
    let mut env = clean_env();
    // `--open --listen 0.0.0.0`: anonymous callers get job:run with no token.
    env.set("WEBCODEX_ALLOW_ANONYMOUS", "true");
    env.set("WEBCODEX_PUBLIC_URL", "https://share.example.test");
    let open = production_service(config("0.0.0.0:8080", Some("test-token")));
    // Auth disabled entirely (no WEBCODEX_TOKEN): every caller is bootstrap.
    let unauthenticated = production_service(config("0.0.0.0:8080", None));

    for server in [&open, &unauthenticated] {
        for host in ["evil.example", "evil.example:8080", "nas.local:8080"] {
            assert_rejected(&server.service, host).await;
        }
        for host in [
            "192.168.1.20:8080",
            "10.0.0.5",
            "[fe80::1]:8080",
            "127.0.0.1:8080",
            "localhost:8080",
            "share.example.test",
        ] {
            assert_not_host_rejected(&server.service, host).await;
        }
    }
}

async fn allow_origin(
    service: &Service,
    method: &str,
    path: &str,
    origin: &str,
) -> (Option<StatusCode>, Option<String>) {
    let request = match method {
        "OPTIONS" => TestClient::options(format!("http://127.0.0.1:8080{path}"))
            .add_header("access-control-request-method", "POST", true)
            .add_header("access-control-request-headers", "content-type,authorization", true),
        _ => TestClient::get(format!("http://127.0.0.1:8080{path}")),
    };
    let response = request
        .add_header("host", "127.0.0.1:8080", true)
        .add_header("origin", origin, true)
        .send(service)
        .await;
    let allow_origin = response
        .headers()
        .get("access-control-allow-origin")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (response.status_code, allow_origin)
}

const ALLOWED_ORIGINS: [&str; 4] = [
    "http://127.0.0.1:8080",
    "http://localhost:5173",
    "http://[::1]:8080",
    "https://share.example.test",
];

const DENIED_ORIGINS: [&str; 5] = [
    "https://evil.example",
    "http://share.example.test",
    "https://share.example.test:8443",
    "tauri://localhost",
    "null",
];

#[tokio::test]
async fn production_cors_echoes_only_loopback_and_public_origins() {
    let mut env = clean_env();
    env.set("WEBCODEX_PUBLIC_URL", "https://share.example.test");
    let server = production_service(config("127.0.0.1:8080", Some("test-token")));

    for allowed in ALLOWED_ORIGINS {
        assert_eq!(
            allow_origin(&server.service, "GET", "/openapi.json", allowed).await.1.as_deref(),
            Some(allowed),
            "allowed origin {allowed} must be echoed"
        );
    }
    for denied in DENIED_ORIGINS {
        assert_eq!(
            allow_origin(&server.service, "GET", "/openapi.json", denied).await.1,
            None,
            "{denied} must not get Access-Control-Allow-Origin"
        );
    }

    // Pre-existing behavior kept on purpose: CORS is a router hoop, so a
    // preflight to a POST-only API route matches no route and is answered
    // without CORS headers, even for an allowed origin.
    let (status, allow) = allow_origin(
        &server.service,
        "OPTIONS",
        "/api/runtime/status",
        "http://localhost:5173",
    )
    .await;
    assert!(!status.is_none_or(|status| status.is_success()));
    assert_eq!(allow, None);
}

#[handler]
async fn echo_ok(res: &mut Response) {
    res.render(Json(serde_json::json!({"ok": true})));
}

/// Preflight decisions of the policy itself, on a route that answers OPTIONS
/// (production has none, see above).
#[tokio::test]
async fn cors_policy_preflight_decisions() {
    let mut env = clean_env();
    env.set("WEBCODEX_PUBLIC_URL", "https://share.example.test");
    let service = Service::new(
        Router::new()
            .hoop(crate::runtime_cors().into_handler())
            .push(Router::with_path("cors-probe").options(echo_ok)),
    );

    for allowed in ALLOWED_ORIGINS {
        let (status, allow) = allow_origin(&service, "OPTIONS", "/cors-probe", allowed).await;
        assert_eq!(allow.as_deref(), Some(allowed));
        assert!(status.is_none_or(|status| status.is_success()));
    }
    for denied in DENIED_ORIGINS {
        assert_eq!(
            allow_origin(&service, "OPTIONS", "/cors-probe", denied).await.1,
            None,
            "preflight from {denied} must not be allowed"
        );
    }
}

#[tokio::test]
async fn cors_origin_allow_list_without_and_with_path_public_url() {
    let mut env = clean_env();
    assert!(cors_origin_allowed("http://127.0.0.1:1420"));
    assert!(cors_origin_allowed("https://localhost"));
    assert!(!cors_origin_allowed("https://share.example.test"));
    assert!(!cors_origin_allowed("http://tauri.localhost"));
    assert!(!cors_origin_allowed("not a url"));

    env.set("WEBCODEX_PUBLIC_URL", "https://share.example.test/webcodex/");
    assert!(cors_origin_allowed("https://share.example.test"));
    assert!(!cors_origin_allowed("https://evil.example"));
}

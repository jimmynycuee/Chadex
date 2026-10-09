//! `/api/*` Host allow-list (DNS-rebinding guard) and CORS origin policy.

use std::sync::Arc;

use salvo::prelude::*;
use salvo::test::{ResponseExt, TestClient};

use super::{cors_origin_allowed, ApiRequestAuthorityGuard};
use crate::test_support::TestEnvGuard;

#[handler]
async fn echo_ok(res: &mut Response) {
    res.render(Json(serde_json::json!({"ok": true})));
}

fn config_bound_to(addr: &str) -> Arc<crate::Config> {
    let mut config = (*crate::test_support::test_config(Some("test-token"))).clone();
    config.addr = addr.to_string();
    Arc::new(config)
}

/// Mirrors the production wiring in `lib.rs`: CORS as a root hoop and the
/// Host guard on the `/api` router, ahead of the pairing enroll route and the
/// authenticated routes.
fn api_service(config: Arc<crate::Config>) -> Service {
    Service::new(
        Router::new()
            .hoop(affix_state::inject(config))
            .hoop(crate::runtime_cors().into_handler())
            .push(
                Router::with_path("api")
                    .hoop(ApiRequestAuthorityGuard)
                    .push(Router::with_path("pairing/enroll").post(echo_ok))
                    .push(Router::with_path("runtime/status").post(echo_ok)),
            )
            .push(Router::with_path("openapi.json").get(echo_ok))
            .push(Router::with_path("cors-probe").get(echo_ok).options(echo_ok)),
    )
}

async fn status_with_host(service: &Service, path: &str, host: &str) -> StatusCode {
    TestClient::post(format!("http://127.0.0.1{path}"))
        .add_header("host", host, true)
        .json(&serde_json::json!({}))
        .send(service)
        .await
        .status_code
        .unwrap_or(StatusCode::OK)
}

#[tokio::test]
async fn api_rejects_untrusted_host_on_loopback_bound_server() {
    let mut env = TestEnvGuard::new();
    env.remove("WEBCODEX_PUBLIC_URL");
    let service = api_service(config_bound_to("127.0.0.1:8080"));

    for host in ["evil.example", "evil.example:8080", "127.0.0.1.evil.example"] {
        assert_eq!(
            status_with_host(&service, "/api/runtime/status", host).await,
            StatusCode::FORBIDDEN,
            "DNS-rebound Host {host} must not reach /api/runtime/status"
        );
    }
    assert_eq!(
        status_with_host(&service, "/api/pairing/enroll", "evil.example").await,
        StatusCode::FORBIDDEN,
        "the unauthenticated pairing route sits behind the same guard"
    );
    assert_eq!(
        status_with_host(&service, "/api/runtime/status", "localhost:notaport").await,
        StatusCode::BAD_REQUEST
    );
    let mut response = TestClient::post("http://127.0.0.1/api/runtime/status")
        .add_header("host", "evil.example", true)
        .add_header("x-forwarded-host", "localhost", true)
        .json(&serde_json::json!({}))
        .send(&service)
        .await;
    assert_eq!(response.status_code, Some(StatusCode::FORBIDDEN));
    let body: serde_json::Value = response.take_json().await.unwrap();
    assert_eq!(body["code"], "untrusted_request_authority");
}

#[tokio::test]
async fn api_accepts_loopback_hosts_on_loopback_bound_server() {
    let mut env = TestEnvGuard::new();
    env.remove("WEBCODEX_PUBLIC_URL");
    for addr in ["127.0.0.1:8080", "localhost:8080", "[::1]:8080"] {
        let service = api_service(config_bound_to(addr));
        for host in [
            "localhost",
            "localhost:8080",
            "127.0.0.1",
            "127.0.0.1:8080",
            "[::1]",
            "[::1]:8080",
        ] {
            assert_eq!(
                status_with_host(&service, "/api/runtime/status", host).await,
                StatusCode::OK,
                "loopback Host {host} must pass on a server bound to {addr}"
            );
        }
    }
}

#[tokio::test]
async fn api_accepts_configured_public_host_on_loopback_bound_server() {
    let mut env = TestEnvGuard::new();
    env.set("WEBCODEX_PUBLIC_URL", "https://share.example.test");
    let service = api_service(config_bound_to("127.0.0.1:8080"));

    for host in ["share.example.test", "share.example.test:443"] {
        assert_eq!(
            status_with_host(&service, "/api/runtime/status", host).await,
            StatusCode::OK,
            "configured public Host {host} (tunnel) must pass"
        );
    }
    assert_eq!(
        status_with_host(&service, "/api/runtime/status", "share.example.test:8443").await,
        StatusCode::FORBIDDEN,
        "a different port is a different authority"
    );
    assert_eq!(
        status_with_host(&service, "/api/runtime/status", "evil.example").await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn api_host_guard_leaves_non_loopback_binds_unchanged() {
    let mut env = TestEnvGuard::new();
    env.remove("WEBCODEX_PUBLIC_URL");
    for addr in ["0.0.0.0:8080", "[::]:8080", "192.168.1.20:8080"] {
        let service = api_service(config_bound_to(addr));
        for host in ["192.168.1.20:8080", "nas.local:8080"] {
            assert_eq!(
                status_with_host(&service, "/api/runtime/status", host).await,
                StatusCode::OK,
                "remote pairing/Runner via {host} must keep working on a server bound to {addr}"
            );
        }
    }
}

async fn allow_origin_for(
    service: &Service,
    method: &str,
    origin: &str,
) -> (Option<StatusCode>, Option<String>) {
    let request = match method {
        "OPTIONS" => TestClient::options("http://127.0.0.1:8080/cors-probe")
            .add_header("access-control-request-method", "POST", true)
            .add_header("access-control-request-headers", "content-type,authorization", true),
        _ => TestClient::get("http://127.0.0.1:8080/openapi.json"),
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

#[tokio::test]
async fn cors_echoes_only_loopback_and_public_origins() {
    let mut env = TestEnvGuard::new();
    env.set("WEBCODEX_PUBLIC_URL", "https://share.example.test");
    let service = api_service(config_bound_to("127.0.0.1:8080"));

    for method in ["OPTIONS", "GET"] {
        for allowed in [
            "http://127.0.0.1:8080",
            "http://localhost:5173",
            "http://[::1]:8080",
            "https://share.example.test",
        ] {
            assert_eq!(
                allow_origin_for(&service, method, allowed).await.1.as_deref(),
                Some(allowed),
                "{method} from allowed origin {allowed} must be echoed"
            );
        }
        for denied in [
            "https://evil.example",
            "http://share.example.test",
            "https://share.example.test:8443",
            "tauri://localhost",
            "null",
        ] {
            assert_eq!(
                allow_origin_for(&service, method, denied).await.1,
                None,
                "{method} from {denied} must not get Access-Control-Allow-Origin"
            );
        }
    }
    let (status, _) = allow_origin_for(&service, "OPTIONS", "http://127.0.0.1:8080").await;
    assert!(
        status.is_none_or(|status| status.is_success()),
        "preflight on a route that answers OPTIONS succeeds for an allowed origin"
    );

    // Pre-existing behavior kept on purpose: CORS is a router hoop, so a
    // preflight to a POST-only API route matches no route and is answered
    // without CORS headers, even for an allowed origin.
    let response = TestClient::options("http://127.0.0.1:8080/api/runtime/status")
        .add_header("host", "127.0.0.1:8080", true)
        .add_header("origin", "http://localhost:5173", true)
        .add_header("access-control-request-method", "POST", true)
        .send(&service)
        .await;
    assert!(!response.status_code.is_none_or(|status| status.is_success()));
    assert!(response.headers().get("access-control-allow-origin").is_none());
}

#[tokio::test]
async fn cors_without_public_url_allows_only_loopback() {
    let mut env = TestEnvGuard::new();
    env.remove("WEBCODEX_PUBLIC_URL");
    assert!(cors_origin_allowed("http://127.0.0.1:1420"));
    assert!(cors_origin_allowed("https://localhost"));
    assert!(!cors_origin_allowed("https://share.example.test"));
    assert!(!cors_origin_allowed("http://tauri.localhost"));
    assert!(!cors_origin_allowed("not a url"));
}

use super::{
    classify_session_error, concise_log_error, non_empty_token, server_log_label,
    RunnerTransportError, RunnerWebSocket,
};

/// HTTP CONNECT response headers are read one byte at a time so no tunneled
/// WebSocket bytes can be over-read or lost before tungstenite takes ownership.
pub(super) const WS_PROXY_CONNECT_HEADER_MAX_BYTES: usize = 16 * 1024;

/// Convert an `http(s)://` server URL into a `ws(s)://` URL plus path.
pub(crate) fn server_url_to_ws(server_url: &str, path: &str) -> Result<String, String> {
    let base = server_url.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{}{}", rest, path)
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{}{}", rest, path)
    } else if base.starts_with("ws://") || base.starts_with("wss://") {
        format!("{}{}", base, path)
    } else {
        return Err(format!(
            "server_url must be http(s)://... for websocket transport; got {}",
            server_log_label(server_url)
        ));
    };
    Ok(ws)
}

/// Build a WebSocket handshake request, carrying a Bearer token only when the
/// configured token is non-empty. Open-mode agents intentionally send no
/// credential so the server must have explicit anonymous mode enabled.
pub(crate) fn build_ws_request(
    ws_url: &str,
    token: &str,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = ws_url.into_client_request().map_err(|e| {
        format!(
            "invalid websocket url for {}: {}",
            server_log_label(ws_url),
            e
        )
    })?;
    if let Some(token) = non_empty_token(token) {
        let value = format!("Bearer {}", token);
        let header_value = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&value)
            .map_err(|e| format!("invalid token header value: {}", e))?;
        request.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
            header_value,
        );
    }
    Ok(request)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HttpProxyEndpoint {
    pub(super) host: String,
    pub(super) port: u16,
}

fn first_nonempty_env_value_with<F>(names: &[&str], get_env: &mut F) -> Option<std::ffi::OsString>
where
    F: FnMut(&str) -> Option<std::ffi::OsString>,
{
    names
        .iter()
        .find_map(|name| get_env(name).filter(|value| !value.as_os_str().is_empty()))
}

fn split_no_proxy_host_port(entry: &str) -> (&str, Option<u16>) {
    if let Some(rest) = entry.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let host = &rest[..end];
            let suffix = &rest[end + 1..];
            if suffix.is_empty() {
                return (host, None);
            }
            if let Some(port) = suffix
                .strip_prefix(':')
                .and_then(|value| value.parse().ok())
            {
                return (host, Some(port));
            }
            return (entry, None);
        }
    }
    if entry.bytes().filter(|byte| *byte == b':').count() == 1 {
        if let Some((host, port)) = entry.rsplit_once(':') {
            if let Ok(port) = port.parse::<u16>() {
                return (host, Some(port));
            }
        }
    }
    (entry, None)
}

fn no_proxy_entry_matches(entry: &str, target_host: &str, target_port: u16) -> bool {
    let entry = entry.trim();
    if entry.is_empty() {
        return false;
    }
    if entry == "*" {
        return true;
    }
    let (pattern, port) = split_no_proxy_host_port(entry);
    if port.is_some_and(|port| port != target_port) {
        return false;
    }
    let pattern = pattern
        .trim()
        .trim_end_matches('.')
        .strip_prefix("*.")
        .unwrap_or(pattern.trim().trim_end_matches('.'))
        .trim_start_matches('.');
    if pattern.is_empty() {
        return false;
    }
    let target_host = target_host.trim_end_matches('.');
    if let Ok(pattern_ip) = pattern.parse::<std::net::IpAddr>() {
        return target_host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|target_ip| target_ip == pattern_ip);
    }
    if pattern.eq_ignore_ascii_case("localhost") {
        return target_host.eq_ignore_ascii_case("localhost");
    }
    let target = target_host.to_ascii_lowercase();
    let pattern = pattern.to_ascii_lowercase();
    target == pattern
        || target
            .strip_suffix(&pattern)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn no_proxy_matches(no_proxy: &str, target_host: &str, target_port: u16) -> bool {
    no_proxy
        .split(',')
        .any(|entry| no_proxy_entry_matches(entry, target_host, target_port))
}

fn canonical_url_host(parsed: &url::Url) -> Option<String> {
    match parsed.host()? {
        url::Host::Domain(host) => (!host.is_empty()).then(|| host.to_string()),
        url::Host::Ipv4(host) => Some(host.to_string()),
        url::Host::Ipv6(host) => Some(host.to_string()),
    }
}

pub(super) fn parse_http_proxy_endpoint(
    raw: &str,
) -> Result<HttpProxyEndpoint, RunnerTransportError> {
    let parsed = url::Url::parse(raw.trim()).map_err(|_| {
        RunnerTransportError::proxy_configuration(
            "websocket connect failed: proxy configuration is invalid",
        )
    })?;
    if parsed.scheme() != "http" {
        return Err(RunnerTransportError::proxy_configuration(
            "websocket connect failed: proxy scheme is unsupported",
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(RunnerTransportError::proxy_configuration(
            "websocket connect failed: proxy authentication is unsupported",
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() || !matches!(parsed.path(), "" | "/")
    {
        return Err(RunnerTransportError::proxy_configuration(
            "websocket connect failed: proxy URL options are unsupported",
        ));
    }
    let host = canonical_url_host(&parsed).ok_or_else(|| {
        RunnerTransportError::proxy_configuration(
            "websocket connect failed: proxy configuration is invalid",
        )
    })?;
    let port = parsed.port_or_known_default().ok_or_else(|| {
        RunnerTransportError::proxy_configuration(
            "websocket connect failed: proxy configuration is invalid",
        )
    })?;
    Ok(HttpProxyEndpoint { host, port })
}

pub(super) fn websocket_proxy_from_env_with<F>(
    ws_url: &str,
    mut get_env: F,
) -> Result<Option<HttpProxyEndpoint>, RunnerTransportError>
where
    F: FnMut(&str) -> Option<std::ffi::OsString>,
{
    let target = url::Url::parse(ws_url).map_err(|_| {
        RunnerTransportError::fatal("websocket connect failed: websocket target is invalid")
    })?;
    let proxy_names: &[&str] = match target.scheme() {
        "wss" => &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"],
        "ws" => &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"],
        _ => {
            return Err(RunnerTransportError::fatal(
                "websocket connect failed: websocket target scheme is invalid",
            ));
        }
    };
    let Some(proxy_raw) = first_nonempty_env_value_with(proxy_names, &mut get_env) else {
        return Ok(None);
    };
    let target_host = canonical_url_host(&target).ok_or_else(|| {
        RunnerTransportError::fatal("websocket connect failed: websocket target is invalid")
    })?;
    let target_port = target.port_or_known_default().ok_or_else(|| {
        RunnerTransportError::fatal("websocket connect failed: websocket target is invalid")
    })?;
    if let Some(no_proxy_raw) =
        first_nonempty_env_value_with(&["NO_PROXY", "no_proxy"], &mut get_env)
    {
        let no_proxy = no_proxy_raw.into_string().map_err(|_| {
            RunnerTransportError::proxy_configuration(
                "websocket connect failed: NO_PROXY configuration is invalid",
            )
        })?;
        if no_proxy_matches(&no_proxy, &target_host, target_port) {
            return Ok(None);
        }
    }
    let proxy = proxy_raw.into_string().map_err(|_| {
        RunnerTransportError::proxy_configuration(
            "websocket connect failed: proxy configuration is invalid",
        )
    })?;
    parse_http_proxy_endpoint(&proxy).map(Some)
}

fn websocket_proxy_from_env(
    ws_url: &str,
) -> Result<Option<HttpProxyEndpoint>, RunnerTransportError> {
    websocket_proxy_from_env_with(ws_url, |name| std::env::var_os(name))
}

pub(super) fn target_authority(host: &str, port: u16) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub(super) fn websocket_target_endpoint(
    ws_url: &str,
) -> Result<(String, u16), RunnerTransportError> {
    let target = url::Url::parse(ws_url).map_err(|_| {
        RunnerTransportError::fatal("websocket connect failed: websocket target is invalid")
    })?;
    let host = canonical_url_host(&target).ok_or_else(|| {
        RunnerTransportError::fatal("websocket connect failed: websocket target is invalid")
    })?;
    let port = target.port_or_known_default().ok_or_else(|| {
        RunnerTransportError::fatal("websocket connect failed: websocket target is invalid")
    })?;
    Ok((host, port))
}

pub(super) async fn http_proxy_connect_tunnel(
    proxy: &HttpProxyEndpoint,
    target_host: &str,
    target_port: u16,
) -> Result<tokio::net::TcpStream, RunnerTransportError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|_| {
            RunnerTransportError::transient("websocket connect failed: proxy TCP connect failed")
        })?;
    let authority = target_authority(target_host, target_port);
    let request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nConnection: keep-alive\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.map_err(|_| {
        RunnerTransportError::transient("websocket connect failed: proxy CONNECT write failed")
    })?;

    let mut headers = Vec::with_capacity(1024);
    loop {
        if headers.len() >= WS_PROXY_CONNECT_HEADER_MAX_BYTES {
            return Err(RunnerTransportError::transient(format!(
                "websocket connect failed: proxy CONNECT response headers exceeded {} bytes",
                WS_PROXY_CONNECT_HEADER_MAX_BYTES
            )));
        }
        let mut byte = [0u8; 1];
        let read = stream.read(&mut byte).await.map_err(|_| {
            RunnerTransportError::transient("websocket connect failed: proxy CONNECT read failed")
        })?;
        if read == 0 {
            return Err(RunnerTransportError::transient(
                "websocket connect failed: proxy CONNECT response was malformed",
            ));
        }
        headers.push(byte[0]);
        if headers.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let headers = std::str::from_utf8(&headers).map_err(|_| {
        RunnerTransportError::transient(
            "websocket connect failed: proxy CONNECT response was malformed",
        )
    })?;
    let status_line = headers.split("\r\n").next().ok_or_else(|| {
        RunnerTransportError::transient(
            "websocket connect failed: proxy CONNECT response was malformed",
        )
    })?;
    let mut parts = status_line.split_whitespace();
    let version = parts
        .next()
        .filter(|version| version.starts_with("HTTP/"))
        .ok_or_else(|| {
            RunnerTransportError::transient(
                "websocket connect failed: proxy CONNECT response was malformed",
            )
        })?;
    let _ = version;
    let status = parts
        .next()
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| {
            RunnerTransportError::transient(
                "websocket connect failed: proxy CONNECT response was malformed",
            )
        })?;
    if status == 407 {
        return Err(RunnerTransportError::proxy_configuration(
            "websocket connect failed: proxy CONNECT returned HTTP 407",
        ));
    }
    if !(200..300).contains(&status) {
        return Err(RunnerTransportError::transient(format!(
            "websocket connect failed: proxy CONNECT returned HTTP {status}"
        )));
    }
    Ok(stream)
}

pub(super) async fn connect_websocket_request_with_proxy(
    request: tokio_tungstenite::tungstenite::http::Request<()>,
    ws_url: &str,
    proxy: Option<&HttpProxyEndpoint>,
    token: &str,
) -> Result<RunnerWebSocket, RunnerTransportError> {
    let Some(proxy) = proxy else {
        return tokio_tungstenite::connect_async(request)
            .await
            .map(|(stream, _)| stream)
            .map_err(|error| {
                classify_session_error(format!(
                    "websocket connect failed: {}",
                    concise_log_error(&error.to_string(), token)
                ))
            });
    };
    let (target_host, target_port) = websocket_target_endpoint(ws_url)?;
    let stream = http_proxy_connect_tunnel(proxy, &target_host, target_port).await?;
    tokio_tungstenite::client_async_tls_with_config(request, stream, None, None)
        .await
        .map(|(stream, _)| stream)
        .map_err(|error| {
            classify_session_error(format!(
                "websocket connect failed: {}",
                concise_log_error(&error.to_string(), token)
            ))
        })
}

pub(super) async fn connect_websocket_request(
    request: tokio_tungstenite::tungstenite::http::Request<()>,
    ws_url: &str,
    token: &str,
) -> Result<RunnerWebSocket, RunnerTransportError> {
    let proxy = websocket_proxy_from_env(ws_url)?;
    connect_websocket_request_with_proxy(request, ws_url, proxy.as_ref(), token).await
}

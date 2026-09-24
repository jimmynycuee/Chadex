//! Passive bounded evidence: never delays or changes bytes delivered to the client.
use super::{header, HeaderMap, Value, VerificationTracker};
use std::sync::Arc;

const MAX_EVIDENCE_BYTES: usize = 256 * 1024;

#[derive(Debug)]
struct RequestEvidence {
    id: Value,
    method: String,
    tool_name: Option<String>,
}

pub(super) struct ResponseEvidence {
    requests: Vec<RequestEvidence>,
    tracker: Arc<VerificationTracker>,
    epoch: u64,
    sse: bool,
    buffer: Vec<u8>,
    event: Vec<u8>,
    disabled: bool,
}

impl ResponseEvidence {
    pub(super) fn new(
        body: &[u8],
        headers: &HeaderMap,
        tracker: Arc<VerificationTracker>,
        epoch: u64,
    ) -> Option<Self> {
        let content_type = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
        let mime = content_type.split(';').next()?.trim();
        let sse = mime.eq_ignore_ascii_case("text/event-stream");
        if !sse && !mime.eq_ignore_ascii_case("application/json") {
            return None;
        }
        let request: Value = serde_json::from_slice(body).ok()?;
        let items = match &request {
            Value::Array(items) => items.as_slice(),
            _ => std::slice::from_ref(&request),
        };
        let requests = items
            .iter()
            .take(16)
            .filter_map(|item| {
                if item.get("jsonrpc")?.as_str()? != "2.0" {
                    return None;
                }
                let id = item.get("id")?;
                if !(id.is_string() || id.is_number()) || id.to_string().len() > 1024 {
                    return None;
                }
                let method = item.get("method")?.as_str()?;
                if !matches!(method, "initialize" | "tools/list" | "tools/call") {
                    return None;
                }
                let tool_name = (method == "tools/call")
                    .then(|| item.pointer("/params/name").and_then(Value::as_str))
                    .flatten()
                    .map(str::to_owned);
                Some(RequestEvidence {
                    id: id.clone(),
                    method: method.to_owned(),
                    tool_name,
                })
            })
            .collect::<Vec<_>>();
        if requests.is_empty()
            || requests
                .iter()
                .enumerate()
                .any(|(i, request)| requests[..i].iter().any(|other| request.id == other.id))
        {
            return None;
        }
        Some(Self {
            requests,
            tracker,
            epoch,
            sse,
            buffer: Vec::new(),
            event: Vec::new(),
            disabled: false,
        })
    }

    fn disable(&mut self) {
        self.disabled = true;
        self.buffer.clear();
        self.event.clear();
    }

    pub(super) fn push(&mut self, bytes: &[u8]) {
        if self.disabled {
            return;
        }
        if !self.sse {
            if self.buffer.len().saturating_add(bytes.len()) > MAX_EVIDENCE_BYTES {
                self.disable();
            } else {
                self.buffer.extend_from_slice(bytes);
            }
            return;
        }
        // Line framing handles arbitrary network chunk boundaries and CRLF.
        for &byte in bytes {
            if byte == b'\n' {
                let mut line = std::mem::take(&mut self.buffer);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if line.is_empty() {
                    let event = std::mem::take(&mut self.event);
                    self.observe(&event);
                } else if let Some(data) = line.strip_prefix(b"data:") {
                    let data = data.strip_prefix(b" ").unwrap_or(data);
                    if self.event.len().saturating_add(data.len() + 1) > MAX_EVIDENCE_BYTES {
                        self.disable();
                        return;
                    }
                    self.event.extend_from_slice(data);
                    self.event.push(b'\n');
                }
            } else {
                if self.buffer.len() >= MAX_EVIDENCE_BYTES {
                    self.disable();
                    return;
                }
                self.buffer.push(byte);
            }
        }
    }

    pub(super) fn finish(&mut self) {
        if !self.disabled && !self.sse {
            let bytes = std::mem::take(&mut self.buffer);
            self.observe(&bytes);
        }
        // SSE evidence requires a complete event, not a truncated final line.
    }

    fn observe(&mut self, bytes: &[u8]) {
        let Ok(response) = serde_json::from_slice::<Value>(bytes) else {
            return;
        };
        let items = match &response {
            Value::Array(items) => items.as_slice(),
            _ => std::slice::from_ref(&response),
        };
        for item in items {
            if item.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
                || item.get("error").is_some()
            {
                continue;
            }
            let Some(result) = item.get("result").filter(|v| v.is_object()) else {
                continue;
            };
            let Some(request) = self
                .requests
                .iter()
                .find(|request| item.get("id") == Some(&request.id))
            else {
                continue;
            };
            if request.method == "tools/call" && !result.get("content").is_some_and(Value::is_array)
            {
                continue;
            }
            // A valid RPC error/tool error cannot verify the selected project.
            if result
                .get("isError")
                .is_some_and(|v| v != &Value::Bool(false))
                || result.pointer("/structuredContent/success") == Some(&Value::Bool(false))
            {
                continue;
            }
            self.tracker
                .observe_request(self.epoch, std::slice::from_ref(&request.method));
            if request.tool_name.as_deref() == Some("read_files") {
                let structured = result.get("structuredContent");
                let output = structured.and_then(|value| value.get("output"));
                let project_path = output
                    .and_then(|value| value.get("project_path"))
                    .and_then(Value::as_str);
                let read_succeeded = output
                    .and_then(|value| value.get("items"))
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| item.get("success").and_then(Value::as_bool) == Some(true))
                    });
                if structured
                    .and_then(|value| value.get("success"))
                    .and_then(Value::as_bool)
                    == Some(true)
                    && read_succeeded
                {
                    if let Some(project_path) = project_path {
                        self.tracker.verify_project_read(self.epoch, project_path);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observer(sse: bool) -> (ResponseEvidence, Arc<VerificationTracker>) {
        let tracker = Arc::new(VerificationTracker::default());
        let epoch = tracker.reset(Some("/tmp/test".into())).epoch;
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            if sse {
                "text/event-stream"
            } else {
                "application/json"
            }
            .parse()
            .unwrap(),
        );
        (
            ResponseEvidence::new(
                br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"read_files"}}"#,
                &headers,
                tracker.clone(),
                epoch,
            )
            .unwrap(),
            tracker,
        )
    }
    #[test]
    fn only_matching_successful_project_read_verifies() {
        for body in [
            r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32603}}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":{"content":[],"isError":true}}"#,
            r#"{"jsonrpc":"2.0","id":8,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":{"content":[],"structuredContent":{"success":false}}}"#,
            r#"{"jsonrpc":"2.0","result":{}}"#,
            r#"{"id":7,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":null}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":{}}"#,
        ] {
            let (mut evidence, tracker) = observer(false);
            evidence.push(body.as_bytes());
            evidence.finish();
            assert!(!tracker.snapshot().verified, "{body}");
            assert!(!tracker.snapshot().chatgpt_connected);
        }
        let (mut evidence, tracker) = observer(false);
        evidence.push(br#"{"jsonrpc":"2.0","id":7,"result":{"content":[]}}"#);
        assert!(!tracker.snapshot().verified);
        evidence.finish();
        assert!(tracker.snapshot().chatgpt_connected);
        assert!(!tracker.snapshot().verified);

        let (mut evidence, tracker) = observer(false);
        evidence.push(br#"{"jsonrpc":"2.0","id":7,"result":{"content":[],"structuredContent":{"success":true,"output":{"project_path":"/tmp/test","items":[{"success":true}]}}}}"#);
        evidence.finish();
        assert!(tracker.snapshot().verified);

        let (mut evidence, tracker) = observer(false);
        evidence.push(br#"{"jsonrpc":"2.0","id":7,"result":{"content":[],"structuredContent":{"success":true,"output":{"project_path":"/tmp/other","items":[{"success":true}]}}}}"#);
        evidence.finish();
        assert!(tracker.snapshot().chatgpt_connected);
        assert!(!tracker.snapshot().verified);
    }
    #[test]
    fn successful_non_read_tool_only_proves_connection_activity() {
        let tracker = Arc::new(VerificationTracker::default());
        let epoch = tracker.reset(Some("/tmp/test".into())).epoch;
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        let mut evidence = ResponseEvidence::new(
            br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"runtime_status"}}"#,
            &headers,
            tracker.clone(),
            epoch,
        )
        .unwrap();
        evidence.push(
            br#"{"jsonrpc":"2.0","id":9,"result":{"content":[],"structuredContent":{"success":true,"output":{"project_path":"/tmp/test","items":[{"success":true}]}}}}"#,
        );
        evidence.finish();
        assert!(tracker.snapshot().chatgpt_connected);
        assert!(!tracker.snapshot().verified);
    }

    #[test]
    fn fragmented_sse_and_epoch_fence() {
        let bytes = b": keepalive\r\ndata: {\r\ndata: \"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"content\":[],\"structuredContent\":{\"success\":true,\"output\":{\"project_path\":\"/tmp/test\",\"items\":[{\"success\":true}]}}}}\r\n\r\n";
        for chunk_size in 1..bytes.len() {
            let (mut evidence, tracker) = observer(true);
            for chunk in bytes.chunks(chunk_size) {
                evidence.push(chunk);
            }
            assert!(tracker.snapshot().verified);
        }
        let (mut evidence, tracker) = observer(true);
        tracker.reset(Some("/tmp/other".into()));
        evidence.push(bytes);
        assert!(!tracker.snapshot().verified);
    }
    #[test]
    fn oversized_and_truncated_observation_fails_closed() {
        for sse in [false, true] {
            let (mut evidence, tracker) = observer(sse);
            evidence.push(&vec![b'x'; MAX_EVIDENCE_BYTES + 1]);
            assert!(evidence.disabled);
            assert!(evidence.buffer.is_empty());
            evidence.finish();
            assert!(!tracker.snapshot().verified);
        }
        let (mut evidence, tracker) = observer(true);
        evidence.push(b"data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"content\":[]}}\n");
        evidence.finish();
        assert!(!tracker.snapshot().verified);
    }
}

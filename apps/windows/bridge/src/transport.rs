use super::*;
use std::io::{self, BufRead, BufReader, Write};
use std::process::{ChildStdin, ChildStdout};
use std::thread;
use zeroize::{Zeroize, Zeroizing};

pub(super) enum WriteJob {
    Frame {
        id: String,
        frame: Zeroizing<Vec<u8>>,
        deadline: Instant,
    },
    Close,
}

struct FrameWriter(Zeroizing<Vec<u8>>);
impl Write for FrameWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_REQUEST_FRAME_BYTES {
            return Err(io::Error::other("request_too_large"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn encode_request(
    id: &str,
    method: &str,
    mut params: Value,
) -> Result<Zeroizing<Vec<u8>>, BridgeError> {
    #[derive(Serialize)]
    struct Request<'a> {
        protocol_version: u64,
        request_id: &'a str,
        method: &'a str,
        params: &'a Value,
    }
    let mut writer = FrameWriter(Zeroizing::new(Vec::with_capacity(4096)));
    let encoded = serde_json::to_writer(
        &mut writer,
        &Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: id,
            method,
            params: &params,
        },
    );
    wipe_value(&mut params);
    encoded.map_err(|_| BridgeError::new(ErrorCode::RequestTooLarge))?;
    writer.0.push(b'\n');
    Ok(writer.0)
}

// Params/response metadata live only for transport; avoid leaving copied
// credential strings in our buffers after a request. Results are transferred
// directly to the caller, whose lifecycle/storage policy remains its own.
fn wipe_value(value: &mut Value) {
    match value {
        Value::String(string) => string.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(wipe_value),
        Value::Object(values) => {
            for (mut key, mut value) in std::mem::take(values) {
                key.zeroize();
                wipe_value(&mut value);
            }
        }
        _ => {}
    }
}

pub(super) fn start(
    shared: &Arc<Shared>,
    stdin: ChildStdin,
    stdout: ChildStdout,
    receiver: mpsc::Receiver<WriteJob>,
) -> io::Result<()> {
    let writer_shared = Arc::clone(shared);
    thread::Builder::new()
        .name("chadex-bridge-write".into())
        .spawn(move || write_loop(writer_shared, stdin, receiver))?;
    let reader_shared = Arc::clone(shared);
    thread::Builder::new()
        .name("chadex-bridge-read".into())
        .spawn(move || read_loop(reader_shared, stdout))?;
    let monitor_shared = Arc::clone(shared);
    thread::Builder::new()
        .name("chadex-bridge-owner".into())
        .spawn(move || monitor(monitor_shared))?;
    Ok(())
}

fn write_loop(shared: Arc<Shared>, mut stdin: ChildStdin, receiver: mpsc::Receiver<WriteJob>) {
    while let Ok(job) = receiver.recv() {
        let WriteJob::Frame {
            id,
            frame,
            deadline,
        } = job
        else {
            return;
        };
        {
            let mut state = lock(&shared.state);
            if !state.pending.contains_key(&id) {
                continue;
            }
            if deadline <= Instant::now() {
                drop(state);
                shared.resolve(&id, Err(BridgeError::new(ErrorCode::RequestTimeout)));
                continue;
            }
            state.writing = Some((id, deadline));
        }
        let result = stdin.write_all(&frame).and_then(|()| stdin.flush());
        lock(&shared.state).writing = None;
        if result.is_err() {
            shared.invalidate(BridgeError::new(ErrorCode::WriteFailed));
            return;
        }
    }
}

fn read_frame(reader: &mut impl BufRead) -> Result<Option<Zeroizing<Vec<u8>>>, BridgeError> {
    let mut frame = Zeroizing::new(Vec::with_capacity(4096));
    loop {
        let buffer = reader
            .fill_buf()
            .map_err(|_| BridgeError::new(ErrorCode::ReadFailed))?;
        if buffer.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(BridgeError::new(ErrorCode::InvalidResponse))
            };
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let count = newline.unwrap_or(buffer.len());
        if frame.len().saturating_add(count) > MAX_RESPONSE_FRAME_BYTES {
            return Err(BridgeError::new(ErrorCode::ResponseTooLarge));
        }
        frame.extend_from_slice(&buffer[..count]);
        reader.consume(count + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(Some(frame));
        }
    }
}

fn decode_response(frame: &[u8]) -> Result<(String, Result<Value, BridgeError>), BridgeError> {
    let mut payload: Value =
        serde_json::from_slice(frame).map_err(|_| BridgeError::new(ErrorCode::InvalidResponse))?;
    let decoded = (|| {
        let object = payload
            .as_object_mut()
            .ok_or_else(|| BridgeError::new(ErrorCode::InvalidResponse))?;
        let version = object
            .get("protocol_version")
            .and_then(Value::as_u64)
            .ok_or_else(|| BridgeError::new(ErrorCode::InvalidResponse))?;
        if version != PROTOCOL_VERSION {
            let mut error = BridgeError::new(ErrorCode::ProtocolMismatch);
            error.received_protocol = Some(version);
            return Err(error);
        }
        let id = object
            .get("request_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| BridgeError::new(ErrorCode::InvalidResponse))?
            .to_owned();
        let has_error = object.get("error").is_some_and(|error| !error.is_null());
        if has_error == object.contains_key("result") {
            return Err(BridgeError::new(ErrorCode::InvalidResponse));
        }
        let result = if has_error {
            let code = object["error"]
                .as_object()
                .and_then(|error| error.get("code"))
                .and_then(Value::as_str)
                .ok_or_else(|| BridgeError::new(ErrorCode::InvalidResponse))?;
            Err(BridgeError::backend(code))
        } else {
            Ok(object.remove("result").expect("checked result presence"))
        };
        Ok((id, result))
    })();
    wipe_value(&mut payload);
    decoded
}

fn read_loop(shared: Arc<Shared>, stdout: ChildStdout) {
    let mut reader = BufReader::with_capacity(4096, stdout);
    loop {
        match read_frame(&mut reader) {
            Ok(Some(frame)) if frame.iter().all(u8::is_ascii_whitespace) => continue,
            Ok(Some(frame)) => match decode_response(&frame) {
                Ok((id, response)) => shared.resolve(&id, response),
                Err(error) => {
                    shared.invalidate(error);
                    return;
                }
            },
            Ok(None) => {
                let mut state = lock(&shared.state);
                if state.health.state == HelperState::Stopping {
                    Shared::fail_pending(&mut state, &BridgeError::new(ErrorCode::Disconnected));
                } else if state.health.state == HelperState::Running {
                    drop(state);
                    shared.invalidate(BridgeError::new(ErrorCode::HelperDied));
                }
                return;
            }
            Err(error) => {
                shared.invalidate(error);
                return;
            }
        }
    }
}

fn monitor(shared: Arc<Shared>) {
    loop {
        let expired = {
            let state = lock(&shared.state);
            if state.health.state == HelperState::Stopping
                && state
                    .shutdown_deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
            {
                Some(ErrorCode::ShutdownForced)
            } else if state
                .writing
                .as_ref()
                .is_some_and(|(_, deadline)| Instant::now() >= *deadline)
            {
                Some(ErrorCode::RequestTimeout)
            } else {
                None
            }
        };
        if let Some(code) = expired {
            shared.invalidate(BridgeError::new(code));
        }

        let mut owned = lock(&shared.process);
        let Some(process) = owned.as_mut() else {
            return;
        };
        match process.try_wait() {
            Ok(Some(status)) => {
                {
                    let mut state = lock(&shared.state);
                    if state.health.state == HelperState::Running
                        || (state.health.state == HelperState::Stopping && !status.success())
                    {
                        let mut error = BridgeError::new(ErrorCode::HelperDied);
                        error.exit_code = status.code();
                        state.health.state = HelperState::Failed;
                        state.health.error = Some(error.clone());
                        Shared::fail_pending(&mut state, &error);
                    }
                    if state.health.state == HelperState::Failed {
                        let _ = process.terminate_tree();
                    }
                }
                if process.try_tree_exit().unwrap_or(false) {
                    owned.take();
                    drop(owned);
                    let mut state = lock(&shared.state);
                    state.health.pid = None;
                    if state.health.state == HelperState::Stopping {
                        state.health.state = HelperState::Stopped;
                    }
                    drop(state);
                    shared.exit.notify_waiters();
                    return;
                }
            }
            Ok(None) => {}
            Err(_) => {
                drop(owned);
                shared.invalidate(BridgeError::new(ErrorCode::ProcessWaitFailed));
                thread::sleep(Duration::from_millis(20));
                continue;
            }
        }
        // Never sleep holding the process mutex: reader/writer faults and
        // Bridge::drop must be able to terminate immediately, without mutex
        // starvation from the polling owner thread.
        drop(owned);
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn bounded_encoding_and_response_schema() {
        assert_eq!(
            encode_request(
                "id",
                "provideCredential",
                json!({"api_key": "x".repeat(MAX_REQUEST_FRAME_BYTES)})
            )
            .unwrap_err()
            .code,
            ErrorCode::RequestTooLarge
        );
        assert_eq!(
            decode_response(br#"{"protocol_version":1,"request_id":"id","result":null}"#)
                .unwrap()
                .1
                .unwrap(),
            Value::Null
        );
        for frame in [br#"{"protocol_version":1,"request_id":"id"}"#.as_slice(), br#"{"protocol_version":1,"request_id":"id","result":{},"error":{"code":"invalid_params"}}"#] {
            assert_eq!(decode_response(frame).unwrap_err().code, ErrorCode::InvalidResponse);
        }
    }
}

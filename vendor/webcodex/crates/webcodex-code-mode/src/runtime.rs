// Portions of the V8 integration pattern are adapted from OpenAI Codex's
// Apache-2.0 licensed codex-rs/code-mode implementation. WebCodex intentionally
// keeps only the one-shot runtime/thread, Promise callback, value conversion,
// microtask checkpoint, and isolate-termination ideas needed for E1.

use crate::{
    normalized_max_concurrent_executions, normalized_timeout_ms, CodeModeError, CodeModeErrorKind,
    CodeModeExecuteRequest, CodeModeExecution, CodeModeHost, CodeModeStats,
    CodeModeTerminationMode, CodeModeToolRequest, CodeModeToolResponse,
    MAX_CONCURRENT_EXECUTIONS_ENV, MAX_CONCURRENT_TOOL_CALLS, MAX_OUTPUT_BYTES, MAX_OUTPUT_ITEMS,
    MAX_SOURCE_BYTES, MAX_TOOL_CALLS,
};
use serde_json::{json, Value as JsonValue};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Semaphore};

struct V8Initialization {
    _platform: v8::SharedRef<v8::Platform>,
}

static V8_INITIALIZATION: OnceLock<Result<V8Initialization, String>> = OnceLock::new();
static EXECUTION_SLOTS: OnceLock<Semaphore> = OnceLock::new();

fn execution_slots() -> &'static Semaphore {
    // Process configuration is sampled once, before the first V8 cell acquires a
    // slot. Later environment mutation cannot silently resize a live semaphore.
    EXECUTION_SLOTS.get_or_init(|| {
        let configured = std::env::var(MAX_CONCURRENT_EXECUTIONS_ENV).ok();
        Semaphore::new(normalized_max_concurrent_executions(configured.as_deref()))
    })
}

fn ensure_v8_initialized() -> Result<(), String> {
    match V8_INITIALIZATION.get_or_init(|| {
        v8::icu::set_common_data_77(deno_core_icudata::ICU_DATA)
            .map_err(|error_code| format!("failed to initialize ICU data: {error_code}"))?;
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform.clone());
        v8::V8::initialize();
        Ok(V8Initialization {
            _platform: platform,
        })
    }) {
        Ok(_) => Ok(()),
        Err(error) => Err(error.clone()),
    }
}

#[derive(Debug)]
enum RuntimeCommand {
    ToolResponse { id: String, result: JsonValue },
    ToolError { id: String, error: String },
    Terminate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeFailureKind {
    Runtime,
    ToolCallBudgetExceeded,
    OutputLimitExceeded,
}

#[derive(Debug, Clone)]
struct RuntimeFailure {
    kind: RuntimeFailureKind,
    message: String,
}

#[derive(Debug)]
enum RuntimeEvent {
    ToolCall {
        id: String,
        tool_name: String,
        arguments: JsonValue,
    },
    Text(String),
    Finished(Option<RuntimeFailure>),
}

struct SpawnedRuntime {
    command_tx: std_mpsc::Sender<RuntimeCommand>,
    event_rx: mpsc::UnboundedReceiver<RuntimeEvent>,
    isolate_handle: v8::IsolateHandle,
    join: thread::JoinHandle<()>,
}

struct RuntimeState {
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    pending_tool_calls: HashMap<String, v8::Global<v8::PromiseResolver>>,
    allowed_tools: Vec<String>,
    next_tool_call_id: u64,
    tool_calls: usize,
    emitted_bytes: usize,
    emitted_items: usize,
    fatal_error: Option<RuntimeFailure>,
}

pub async fn execute(
    host: Arc<dyn CodeModeHost>,
    request: CodeModeExecuteRequest,
) -> Result<CodeModeExecution, CodeModeError> {
    execute_with_termination_mode(
        host,
        request,
        CodeModeTerminationMode::ReturnAtFrontendDeadline,
    )
    .await
}

pub async fn execute_with_termination_mode(
    host: Arc<dyn CodeModeHost>,
    request: CodeModeExecuteRequest,
    termination_mode: CodeModeTerminationMode,
) -> Result<CodeModeExecution, CodeModeError> {
    let started_at = Instant::now();
    validate_request(&request).map_err(|message| CodeModeError {
        kind: CodeModeErrorKind::InvalidRequest,
        message,
        stats: CodeModeStats::default(),
    })?;
    let timeout_ms = normalized_timeout_ms(request.timeout_ms);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let slot_wait_started_at = Instant::now();
    let execution_slot = tokio::select! {
        permit = execution_slots().acquire() => permit.map_err(|_| CodeModeError {
            kind: CodeModeErrorKind::Runtime,
            message: "code mode execution slots are unavailable".to_string(),
            stats: CodeModeStats {
                slot_wait_ms: elapsed_ms(slot_wait_started_at),
                ..CodeModeStats::default()
            },
        })?,
        _ = tokio::time::sleep_until(deadline) => {
            if termination_mode.drains_started_children() {
                host.stop_accepting_calls();
            }
            return Err(CodeModeError {
                kind: CodeModeErrorKind::Timeout,
                message: format!("code mode execution exceeded {timeout_ms} ms while waiting for a runtime slot"),
                stats: CodeModeStats {
                    slot_wait_ms: elapsed_ms(slot_wait_started_at),
                    ..CodeModeStats::default()
                },
            });
        }
    };
    let slot_wait_ms = elapsed_ms(slot_wait_started_at);
    let mut execution_slot = Some(execution_slot);
    let mut runtime =
        spawn_runtime(request.source, request.allowed_tools).map_err(|message| CodeModeError {
            kind: CodeModeErrorKind::Runtime,
            message,
            stats: CodeModeStats {
                slot_wait_ms,
                ..CodeModeStats::default()
            },
        })?;

    let mut deadline_sleep = Box::pin(tokio::time::sleep_until(deadline));
    let mut content = Vec::new();
    let mut returned_bytes = 0usize;
    let mut tool_calls = 0usize;
    let mut in_flight_count = 0usize;
    let mut max_in_flight = 0usize;
    let mut pending_calls = VecDeque::new();
    let mut in_flight = tokio::task::JoinSet::new();
    let mut runtime_finished: Option<Option<RuntimeFailure>> = None;

    loop {
        while in_flight_count < MAX_CONCURRENT_TOOL_CALLS {
            let Some((id, request)) = pending_calls.pop_front() else {
                break;
            };
            let host = Arc::clone(&host);
            in_flight.spawn(async move {
                let result = host
                    .invoke_tool(request)
                    .await
                    .map_err(|error| error.to_string());
                (id, result)
            });
            in_flight_count += 1;
            max_in_flight = max_in_flight.max(in_flight_count);
        }

        if runtime_finished.is_some() && in_flight_count == 0 && pending_calls.is_empty() {
            break;
        }

        tokio::select! {
            _ = &mut deadline_sleep => {
                if termination_mode.drains_started_children() {
                    // Frontend failure closes admission before V8 termination. Runtime-queued
                    // requests are discarded, while host tasks already started are drained
                    // without attempting to resolve their Promises back into the terminated isolate.
                    host.stop_accepting_calls();
                    pending_calls.clear();
                }
                let _ = runtime.isolate_handle.terminate_execution();
                let _ = runtime.command_tx.send(RuntimeCommand::Terminate);
                join_runtime(runtime.join).await;
                // The process-wide permit bounds active V8 cells, not post-frontend
                // host reconciliation. Release it before the bounded child drain.
                drop(execution_slot.take());
                if let Some(max_drain_ms) = termination_mode.drain_timeout_ms() {
                    let _ = drain_in_flight(
                        &mut in_flight,
                        &mut in_flight_count,
                        Duration::from_millis(max_drain_ms),
                    )
                    .await;
                }
                let stats = finish_stats(started_at, tool_calls, max_in_flight, returned_bytes, slot_wait_ms);
                return Err(CodeModeError {
                    kind: CodeModeErrorKind::Timeout,
                    message: format!("code mode execution exceeded {timeout_ms} ms"),
                    stats,
                });
            }
            event = runtime.event_rx.recv(), if runtime_finished.is_none() => {
                match event {
                    Some(RuntimeEvent::ToolCall { id, tool_name, arguments }) => {
                        tool_calls += 1;
                        pending_calls.push_back((id, CodeModeToolRequest { tool_name, arguments }));
                    }
                    Some(RuntimeEvent::Text(text)) => {
                        returned_bytes = returned_bytes.saturating_add(text.len());
                        content.push(text);
                    }
                    Some(RuntimeEvent::Finished(failure)) => {
                        if termination_mode.drains_started_children() {
                            host.stop_accepting_calls();
                            pending_calls.clear();
                        }
                        runtime_finished = Some(failure);
                        // RuntimeEvent::Finished means the V8 decision phase has
                        // ended. Host reconciliation must not consume V8 capacity.
                        drop(execution_slot.take());
                        if let Some(max_drain_ms) = termination_mode.drain_timeout_ms() {
                            let drained = drain_in_flight(
                                &mut in_flight,
                                &mut in_flight_count,
                                Duration::from_millis(max_drain_ms),
                            )
                            .await;
                            if !drained
                                && runtime_finished
                                    .as_ref()
                                    .is_some_and(|failure| failure.is_none())
                            {
                                runtime_finished = Some(Some(RuntimeFailure {
                                    kind: RuntimeFailureKind::Runtime,
                                    message: format!(
                                        "code mode started-child drain exceeded {max_drain_ms} ms"
                                    ),
                                }));
                            }
                        }
                    }
                    None => {
                        if termination_mode.drains_started_children() {
                            host.stop_accepting_calls();
                            pending_calls.clear();
                        }
                        runtime_finished = Some(Some(RuntimeFailure {
                            kind: RuntimeFailureKind::Runtime,
                            message: "code mode runtime thread ended without a terminal result".to_string(),
                        }));
                        drop(execution_slot.take());
                        if let Some(max_drain_ms) = termination_mode.drain_timeout_ms() {
                            let _ = drain_in_flight(
                                &mut in_flight,
                                &mut in_flight_count,
                                Duration::from_millis(max_drain_ms),
                            )
                            .await;
                        }
                    }
                }
            }
            completed = next_in_flight(&mut in_flight), if in_flight_count > 0 => {
                let Some((id, result)) = completed else {
                    in_flight_count = 0;
                    continue;
                };
                in_flight_count -= 1;
                if runtime_finished.is_none() {
                    match result {
                        Ok(response) => {
                            let response = serde_json::to_value(response).unwrap_or_else(|_| json!({
                                "success": false,
                                "output": null,
                                "error": "failed to serialize nested tool response",
                            }));
                            let _ = runtime.command_tx.send(RuntimeCommand::ToolResponse { id, result: response });
                        }
                        Err(error) => {
                            let _ = runtime.command_tx.send(RuntimeCommand::ToolError { id, error });
                        }
                    }
                }
            }
        }
    }

    let failure = runtime_finished.flatten();
    drop(execution_slot.take());
    join_runtime(runtime.join).await;
    let stats = finish_stats(
        started_at,
        tool_calls,
        max_in_flight,
        returned_bytes,
        slot_wait_ms,
    );
    if let Some(failure) = failure {
        let kind = match failure.kind {
            RuntimeFailureKind::Runtime => CodeModeErrorKind::Runtime,
            RuntimeFailureKind::ToolCallBudgetExceeded => CodeModeErrorKind::ToolCallBudgetExceeded,
            RuntimeFailureKind::OutputLimitExceeded => CodeModeErrorKind::OutputLimitExceeded,
        };
        return Err(CodeModeError {
            kind,
            message: failure.message,
            stats,
        });
    }

    Ok(CodeModeExecution { content, stats })
}

fn elapsed_ms(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn finish_stats(
    started_at: Instant,
    tool_calls: usize,
    max_in_flight: usize,
    returned_bytes: usize,
    slot_wait_ms: u64,
) -> CodeModeStats {
    CodeModeStats {
        tool_calls,
        max_in_flight,
        duration_ms: elapsed_ms(started_at),
        returned_bytes,
        slot_wait_ms,
    }
}

fn validate_request(request: &CodeModeExecuteRequest) -> Result<(), String> {
    if request.source.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "code mode source exceeds the {MAX_SOURCE_BYTES}-byte limit"
        ));
    }
    let mut seen = HashSet::new();
    for tool in &request.allowed_tools {
        let mut chars = tool.chars();
        let valid_first = chars
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic());
        let valid_rest = chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric());
        if tool.is_empty() || tool.len() > 128 || !valid_first || !valid_rest {
            return Err(format!("invalid code mode tool name `{tool}`"));
        }
        if !seen.insert(tool.as_str()) {
            return Err(format!("duplicate code mode tool name `{tool}`"));
        }
    }
    Ok(())
}

async fn next_in_flight(
    in_flight: &mut tokio::task::JoinSet<(String, Result<CodeModeToolResponse, String>)>,
) -> Option<(String, Result<CodeModeToolResponse, String>)> {
    match in_flight.join_next().await {
        Some(Ok(completed)) => Some(completed),
        Some(Err(error)) => Some((
            "<host-task>".to_string(),
            Err(format!("nested tool host task failed: {error}")),
        )),
        None => None,
    }
}

async fn drain_in_flight(
    in_flight: &mut tokio::task::JoinSet<(String, Result<CodeModeToolResponse, String>)>,
    in_flight_count: &mut usize,
    max_drain: Duration,
) -> bool {
    let drain = async {
        while *in_flight_count > 0 {
            if next_in_flight(in_flight).await.is_none() {
                *in_flight_count = 0;
                break;
            }
            *in_flight_count -= 1;
        }
    };
    if tokio::time::timeout(max_drain, drain).await.is_ok() {
        return true;
    }

    // Frontend termination must remain bounded even if an admitted read or
    // consequential host future stalls. Cancelling a consequential host future
    // leaves the host's pre-dispatch receipt at outcome_unknown; domain cleanup
    // guards retain their own canonical cancellation/reconciliation semantics.
    in_flight.abort_all();
    while in_flight.join_next().await.is_some() {}
    *in_flight_count = 0;
    false
}

async fn join_runtime(join: thread::JoinHandle<()>) {
    let _ = tokio::task::spawn_blocking(move || join.join()).await;
}

fn spawn_runtime(source: String, allowed_tools: Vec<String>) -> Result<SpawnedRuntime, String> {
    ensure_v8_initialized()?;
    let (command_tx, command_rx) = std_mpsc::channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (handle_tx, handle_rx) = std_mpsc::sync_channel(1);
    let join = thread::Builder::new()
        .name("webcodex-code-mode-v8".to_string())
        .spawn(move || run_runtime(source, allowed_tools, command_rx, event_tx, handle_tx))
        .map_err(|error| format!("failed to spawn code mode runtime thread: {error}"))?;
    let isolate_handle = handle_rx
        .recv()
        .map_err(|_| "code mode runtime failed before isolate initialization".to_string())?;
    Ok(SpawnedRuntime {
        command_tx,
        event_rx,
        isolate_handle,
        join,
    })
}

fn run_runtime(
    source: String,
    allowed_tools: Vec<String>,
    command_rx: std_mpsc::Receiver<RuntimeCommand>,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    handle_tx: std_mpsc::SyncSender<v8::IsolateHandle>,
) {
    let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
    let isolate_handle = isolate.thread_safe_handle();
    if handle_tx.send(isolate_handle).is_err() {
        return;
    }
    isolate.set_host_import_module_dynamically_callback(dynamic_import_callback);

    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    scope.set_slot(RuntimeState {
        event_tx: event_tx.clone(),
        pending_tool_calls: HashMap::new(),
        allowed_tools,
        next_tool_call_id: 1,
        tool_calls: 0,
        emitted_bytes: 0,
        emitted_items: 0,
        fatal_error: None,
    });

    if let Err(message) = install_globals(scope) {
        let _ = event_tx.send(RuntimeEvent::Finished(Some(runtime_failure(message))));
        return;
    }

    let promise = match evaluate_source(scope, &source) {
        Ok(promise) => promise,
        Err(failure) => {
            let _ = event_tx.send(RuntimeEvent::Finished(Some(failure)));
            return;
        }
    };
    if let Some(finished) = completion_state(scope, &promise) {
        let _ = event_tx.send(RuntimeEvent::Finished(finished));
        return;
    }

    loop {
        match command_rx.recv() {
            Ok(RuntimeCommand::ToolResponse { id, result }) => {
                if let Err(message) = resolve_tool_response(scope, &id, Ok(result)) {
                    let _ = event_tx.send(RuntimeEvent::Finished(Some(runtime_failure(message))));
                    return;
                }
            }
            Ok(RuntimeCommand::ToolError { id, error }) => {
                if let Err(message) = resolve_tool_response(scope, &id, Err(error)) {
                    let _ = event_tx.send(RuntimeEvent::Finished(Some(runtime_failure(message))));
                    return;
                }
            }
            Ok(RuntimeCommand::Terminate) | Err(_) => return,
        }
        scope.perform_microtask_checkpoint();
        if let Some(finished) = completion_state(scope, &promise) {
            let _ = event_tx.send(RuntimeEvent::Finished(finished));
            return;
        }
    }
}

fn install_globals(scope: &mut v8::PinScope<'_, '_>) -> Result<(), String> {
    let global = scope.get_current_context().global(scope);
    for name in ["console", "Atomics", "SharedArrayBuffer", "WebAssembly"] {
        delete_global(scope, global, name)?;
    }
    let tools = v8::Object::new(scope);
    let allowed_tools = scope
        .get_slot::<RuntimeState>()
        .map(|state| state.allowed_tools.clone())
        .unwrap_or_default();
    for (index, tool) in allowed_tools.iter().enumerate() {
        let key = v8::String::new(scope, tool)
            .ok_or_else(|| format!("failed to allocate tool name `{tool}`"))?;
        let data = v8::Integer::new(scope, index as i32);
        let function = v8::FunctionTemplate::builder(tool_callback)
            .data(data.into())
            .build(scope)
            .get_function(scope)
            .ok_or_else(|| format!("failed to create tool function `{tool}`"))?;
        if tools.set(scope, key.into(), function.into()) != Some(true) {
            return Err(format!("failed to install tool function `{tool}`"));
        }
    }
    set_global(scope, global, "tools", tools.into())?;
    let text = v8::FunctionTemplate::new(scope, text_callback)
        .get_function(scope)
        .ok_or_else(|| "failed to create text helper".to_string())?;
    set_global(scope, global, "text", text.into())?;
    Ok(())
}

fn evaluate_source(
    scope: &mut v8::PinScope<'_, '_>,
    source: &str,
) -> Result<v8::Global<v8::Promise>, RuntimeFailure> {
    let wrapped = format!("(async () => {{\n{source}\n}})()");
    let tc = std::pin::pin!(v8::TryCatch::new(scope));
    let mut tc = tc.init();
    let source = v8::String::new(&tc, &wrapped)
        .ok_or_else(|| runtime_failure("failed to allocate code mode source"))?;
    let script = v8::Script::compile(&tc, source, None).ok_or_else(|| {
        let message = tc
            .exception()
            .map(|exception| value_to_error_text(&mut tc, exception))
            .unwrap_or_else(|| "failed to compile code mode source".to_string());
        runtime_failure(message)
    })?;
    let value = script.run(&tc).ok_or_else(|| {
        let message = tc
            .exception()
            .map(|exception| value_to_error_text(&mut tc, exception))
            .unwrap_or_else(|| "failed to evaluate code mode source".to_string());
        runtime_failure(message)
    })?;
    tc.perform_microtask_checkpoint();
    let promise = v8::Local::<v8::Promise>::try_from(value)
        .map_err(|_| runtime_failure("code mode wrapper did not return a Promise"))?;
    Ok(v8::Global::new(&tc, promise))
}

fn completion_state(
    scope: &mut v8::PinScope<'_, '_>,
    promise: &v8::Global<v8::Promise>,
) -> Option<Option<RuntimeFailure>> {
    let promise = v8::Local::new(scope, promise);
    match promise.state() {
        v8::PromiseState::Pending => None,
        v8::PromiseState::Fulfilled => Some(
            scope
                .get_slot::<RuntimeState>()
                .and_then(|state| state.fatal_error.clone()),
        ),
        v8::PromiseState::Rejected => {
            if let Some(failure) = scope
                .get_slot::<RuntimeState>()
                .and_then(|state| state.fatal_error.clone())
            {
                return Some(Some(failure));
            }
            let result = promise.result(scope);
            Some(Some(runtime_failure(value_to_error_text(scope, result))))
        }
    }
}

fn tool_callback(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue<v8::Value>,
) {
    let tool_index = args.data().integer_value(scope).unwrap_or(-1);
    let Ok(tool_index) = usize::try_from(tool_index) else {
        throw_error(scope, "invalid tool callback identity");
        return;
    };
    let arguments = if args.length() == 0 || args.get(0).is_undefined() {
        json!({})
    } else {
        match v8_value_to_json(scope, args.get(0)) {
            Ok(Some(JsonValue::Object(arguments))) => JsonValue::Object(arguments),
            Ok(_) => {
                throw_error(scope, "tool arguments must be a JSON object");
                return;
            }
            Err(message) => {
                throw_error(scope, &message);
                return;
            }
        }
    };

    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        throw_error(scope, "failed to create nested tool Promise");
        return;
    };
    let promise = resolver.get_promise(scope);
    let resolver = v8::Global::new(scope, resolver);

    let (event_tx, tool_name, id) = {
        let Some(state) = scope.get_slot_mut::<RuntimeState>() else {
            throw_error(scope, "code mode runtime state unavailable");
            return;
        };
        if state.tool_calls >= MAX_TOOL_CALLS {
            state.fatal_error = Some(RuntimeFailure {
                kind: RuntimeFailureKind::ToolCallBudgetExceeded,
                message: format!("code mode nested tool call limit ({MAX_TOOL_CALLS}) exceeded"),
            });
            throw_error(scope, "code mode nested tool call limit exceeded");
            return;
        }
        let Some(tool_name) = state.allowed_tools.get(tool_index).cloned() else {
            throw_error(scope, "tool callback identity is out of range");
            return;
        };
        state.tool_calls += 1;
        let id = format!("nested-{}", state.next_tool_call_id);
        state.next_tool_call_id = state.next_tool_call_id.saturating_add(1);
        state.pending_tool_calls.insert(id.clone(), resolver);
        (state.event_tx.clone(), tool_name, id)
    };
    let _ = event_tx.send(RuntimeEvent::ToolCall {
        id,
        tool_name,
        arguments,
    });
    retval.set(promise.into());
}

fn text_callback(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue<v8::Value>,
) {
    let value = if args.length() == 0 {
        v8::undefined(scope).into()
    } else {
        args.get(0)
    };
    let text = match serialize_text(scope, value) {
        Ok(text) => text,
        Err(message) => {
            throw_error(scope, &message);
            return;
        }
    };
    let Some(state) = scope.get_slot_mut::<RuntimeState>() else {
        throw_error(scope, "code mode runtime state unavailable");
        return;
    };
    if state.emitted_items >= MAX_OUTPUT_ITEMS {
        state.fatal_error = Some(RuntimeFailure {
            kind: RuntimeFailureKind::OutputLimitExceeded,
            message: format!("code mode text output exceeds the {MAX_OUTPUT_ITEMS}-emission limit"),
        });
        throw_error(scope, "code mode text emission limit exceeded");
        return;
    }
    let next_bytes = state.emitted_bytes.saturating_add(text.len());
    if next_bytes > MAX_OUTPUT_BYTES {
        state.fatal_error = Some(RuntimeFailure {
            kind: RuntimeFailureKind::OutputLimitExceeded,
            message: format!("code mode text output exceeds the {MAX_OUTPUT_BYTES}-byte limit"),
        });
        throw_error(scope, "code mode text output limit exceeded");
        return;
    }
    state.emitted_bytes = next_bytes;
    state.emitted_items += 1;
    let _ = state.event_tx.send(RuntimeEvent::Text(text));
    retval.set(v8::undefined(scope).into());
}

fn resolve_tool_response(
    scope: &mut v8::PinScope<'_, '_>,
    id: &str,
    response: Result<JsonValue, String>,
) -> Result<(), String> {
    let resolver = scope
        .get_slot_mut::<RuntimeState>()
        .and_then(|state| state.pending_tool_calls.remove(id))
        .ok_or_else(|| format!("unknown nested tool call `{id}`"))?;
    let tc = std::pin::pin!(v8::TryCatch::new(scope));
    let mut tc = tc.init();
    let resolver = v8::Local::new(&tc, &resolver);
    match response {
        Ok(response) => {
            let value = json_to_v8(&mut tc, &response)
                .ok_or_else(|| "failed to serialize nested tool response".to_string())?;
            resolver.resolve(&tc, value);
        }
        Err(error) => {
            let error = v8::String::new(&tc, &error)
                .ok_or_else(|| "failed to allocate nested tool host error".to_string())?;
            resolver.reject(&tc, error.into());
        }
    }
    if tc.has_caught() {
        let message = tc
            .exception()
            .map(|exception| value_to_error_text(&mut tc, exception))
            .unwrap_or_else(|| "nested tool Promise resolution failed".to_string());
        return Err(message);
    }
    Ok(())
}

fn serialize_text(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
) -> Result<String, String> {
    if value.is_string() {
        return Ok(value.to_rust_string_lossy(scope));
    }
    let tc = std::pin::pin!(v8::TryCatch::new(scope));
    let mut tc = tc.init();
    let Some(stringified) = v8::json::stringify(&tc, value) else {
        let message = tc
            .exception()
            .map(|exception| value_to_error_text(&mut tc, exception))
            .unwrap_or_else(|| "text(value) requires a string or JSON-safe value".to_string());
        return Err(message);
    };
    Ok(stringified.to_rust_string_lossy(&tc))
}

fn v8_value_to_json(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
) -> Result<Option<JsonValue>, String> {
    let tc = std::pin::pin!(v8::TryCatch::new(scope));
    let mut tc = tc.init();
    let Some(stringified) = v8::json::stringify(&tc, value) else {
        if tc.has_caught() {
            let message = tc
                .exception()
                .map(|exception| value_to_error_text(&mut tc, exception))
                .unwrap_or_else(|| "failed to serialize JavaScript value".to_string());
            return Err(message);
        }
        return Ok(None);
    };
    serde_json::from_str(&stringified.to_rust_string_lossy(&tc))
        .map(Some)
        .map_err(|error| format!("failed to serialize JavaScript value: {error}"))
}

fn json_to_v8<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &JsonValue,
) -> Option<v8::Local<'s, v8::Value>> {
    let json = serde_json::to_string(value).ok()?;
    let json = v8::String::new(scope, &json)?;
    v8::json::parse(scope, json)
}

fn value_to_error_text(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
) -> String {
    if value.is_object() {
        if let Ok(object) = v8::Local::<v8::Object>::try_from(value) {
            if let Some(key) = v8::String::new(scope, "stack") {
                if let Some(stack) = object.get(scope, key.into()) {
                    if stack.is_string() {
                        return stack.to_rust_string_lossy(scope);
                    }
                }
            }
        }
    }
    value.to_rust_string_lossy(scope)
}

fn throw_error(scope: &mut v8::PinScope<'_, '_>, message: &str) {
    if let Some(message) = v8::String::new(scope, message) {
        scope.throw_exception(message.into());
    }
}

fn set_global<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: v8::Local<'s, v8::Object>,
    name: &str,
    value: v8::Local<'s, v8::Value>,
) -> Result<(), String> {
    let key = v8::String::new(scope, name)
        .ok_or_else(|| format!("failed to allocate global `{name}`"))?;
    if global.set(scope, key.into(), value) == Some(true) {
        Ok(())
    } else {
        Err(format!("failed to set global `{name}`"))
    }
}

fn delete_global<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    global: v8::Local<'s, v8::Object>,
    name: &str,
) -> Result<(), String> {
    let key = v8::String::new(scope, name)
        .ok_or_else(|| format!("failed to allocate global `{name}`"))?;
    if global.delete(scope, key.into()) == Some(true) {
        Ok(())
    } else {
        Err(format!("failed to remove global `{name}`"))
    }
}

fn runtime_failure(message: impl Into<String>) -> RuntimeFailure {
    RuntimeFailure {
        kind: RuntimeFailureKind::Runtime,
        message: message.into(),
    }
}

fn dynamic_import_callback<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _host_defined_options: v8::Local<'s, v8::Data>,
    _resource_name: v8::Local<'s, v8::Value>,
    _specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
) -> Option<v8::Local<'s, v8::Promise>> {
    let resolver = v8::PromiseResolver::new(scope)?;
    let error = v8::String::new(scope, "imports are not available in WebCodex Code Mode")?;
    resolver.reject(scope, error.into());
    Some(resolver.get_promise(scope))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CodeModeHostError, CodeModeHostFuture};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::sync::{Barrier, Notify, Semaphore};

    #[derive(Default)]
    struct RecordingHost {
        calls: Mutex<Vec<String>>,
    }

    impl CodeModeHost for RecordingHost {
        fn invoke_tool(
            &self,
            request: CodeModeToolRequest,
        ) -> CodeModeHostFuture<'_, Result<CodeModeToolResponse, CodeModeHostError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(request.tool_name.clone());
                let output = match request.tool_name.as_str() {
                    "fake_search" => json!({"needs_detail": true}),
                    "fake_read" => json!({"detail": "found"}),
                    _ => json!({"ok": true}),
                };
                Ok(CodeModeToolResponse {
                    success: true,
                    output,
                    error: None,
                })
            })
        }
    }

    fn request(source: &str, allowed_tools: &[&str]) -> CodeModeExecuteRequest {
        CodeModeExecuteRequest {
            source: source.to_string(),
            allowed_tools: allowed_tools
                .iter()
                .map(|tool| (*tool).to_string())
                .collect(),
            timeout_ms: Some(2_000),
        }
    }

    #[test]
    fn slot_wait_is_diagnostic_only_and_not_serialized_in_model_stats() {
        let stats = finish_stats(Instant::now(), 2, 1, 3, 17);
        assert_eq!(stats.slot_wait_ms, 17);
        let serialized = serde_json::to_value(stats).unwrap();
        let mut keys = serialized
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "duration_ms",
                "max_in_flight",
                "returned_bytes",
                "tool_calls"
            ]
        );
    }

    #[tokio::test]
    async fn emits_basic_json_text() {
        let result = execute(
            Arc::new(RecordingHost::default()),
            request(r#"text({hello: "world"});"#, &[]),
        )
        .await
        .unwrap();
        assert_eq!(result.content, vec![r#"{"hello":"world"}"#]);
        assert_eq!(result.stats.tool_calls, 0);
    }

    #[tokio::test]
    async fn awaits_one_nested_tool_call() {
        let host = Arc::new(RecordingHost::default());
        let result = execute(
            host.clone(),
            request(
                r#"const r = await tools.fake_read({}); text(r);"#,
                &["fake_read"],
            ),
        )
        .await
        .unwrap();
        assert_eq!(host.calls.lock().unwrap().as_slice(), ["fake_read"]);
        assert_eq!(result.stats.tool_calls, 1);
        let emitted: JsonValue = serde_json::from_str(&result.content[0]).unwrap();
        assert_eq!(emitted["success"], true);
        assert_eq!(emitted["output"]["detail"], "found");
    }

    #[tokio::test]
    async fn adaptive_second_call_depends_on_first_result() {
        let host = Arc::new(RecordingHost::default());
        let result = execute(
            host.clone(),
            request(
                r#"
                const search = await tools.fake_search({});
                if (search.output.needs_detail) {
                    const detail = await tools.fake_read({path: "chosen-after-search"});
                    text(detail.output);
                }
                "#,
                &["fake_search", "fake_read"],
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            host.calls.lock().unwrap().as_slice(),
            ["fake_search", "fake_read"]
        );
        assert_eq!(result.content, vec![r#"{"detail":"found"}"#]);
    }

    struct ConcurrentHost {
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
        barrier: Barrier,
    }

    impl ConcurrentHost {
        fn new() -> Self {
            Self {
                in_flight: AtomicUsize::new(0),
                max_in_flight: AtomicUsize::new(0),
                barrier: Barrier::new(3),
            }
        }
    }

    impl CodeModeHost for ConcurrentHost {
        fn invoke_tool(
            &self,
            _request: CodeModeToolRequest,
        ) -> CodeModeHostFuture<'_, Result<CodeModeToolResponse, CodeModeHostError>> {
            Box::pin(async move {
                let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_in_flight.fetch_max(now, Ordering::SeqCst);
                self.barrier.wait().await;
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                Ok(CodeModeToolResponse {
                    success: true,
                    output: json!({"ok": true}),
                    error: None,
                })
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn promise_all_overlaps_independent_calls() {
        let host = Arc::new(ConcurrentHost::new());
        let result = execute(
            host.clone(),
            request(
                r#"
                const [a,b,c] = await Promise.all([
                    tools.a({}), tools.b({}), tools.c({})
                ]);
                text([a.output.ok, b.output.ok, c.output.ok]);
                "#,
                &["a", "b", "c"],
            ),
        )
        .await
        .unwrap();
        assert!(host.max_in_flight.load(Ordering::SeqCst) >= 2);
        assert!(result.stats.max_in_flight >= 2);
        assert_eq!(result.stats.tool_calls, 3);
    }

    #[tokio::test]
    async fn nested_tool_call_budget_fails_boundedly() {
        let error = execute(
            Arc::new(RecordingHost::default()),
            request(
                "await Promise.all(Array.from({length: 33}, () => tools.fake_read({})));",
                &["fake_read"],
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, CodeModeErrorKind::ToolCallBudgetExceeded);
        assert_eq!(error.stats.tool_calls, MAX_TOOL_CALLS);
    }

    #[tokio::test]
    async fn unknown_tool_is_not_callable() {
        let error = execute(
            Arc::new(RecordingHost::default()),
            request("await tools.not_allowed({});", &["fake_read"]),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, CodeModeErrorKind::Runtime);
        assert!(error.message.contains("not_allowed"));
    }

    #[tokio::test]
    async fn emitted_text_is_hard_bounded() {
        let error = execute(
            Arc::new(RecordingHost::default()),
            request(&format!("text('x'.repeat({}));", MAX_OUTPUT_BYTES + 1), &[]),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, CodeModeErrorKind::OutputLimitExceeded);
        assert_eq!(error.stats.returned_bytes, 0);
    }

    #[tokio::test]
    async fn emitted_text_item_count_is_hard_bounded() {
        let error = execute(
            Arc::new(RecordingHost::default()),
            request(
                &format!(
                    "for (let i = 0; i < {}; i++) text('');",
                    MAX_OUTPUT_ITEMS + 1
                ),
                &[],
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, CodeModeErrorKind::OutputLimitExceeded);
        assert_eq!(error.stats.returned_bytes, 0);
    }

    struct LifecycleHost {
        started: AtomicUsize,
        completed: AtomicUsize,
        stopped: AtomicBool,
        state_changed: Notify,
        release: Semaphore,
    }

    impl LifecycleHost {
        fn new() -> Self {
            Self {
                started: AtomicUsize::new(0),
                completed: AtomicUsize::new(0),
                stopped: AtomicBool::new(false),
                state_changed: Notify::new(),
                release: Semaphore::new(0),
            }
        }

        async fn wait_for_started(&self, expected: usize) {
            loop {
                let changed = self.state_changed.notified();
                if self.started.load(Ordering::SeqCst) >= expected {
                    return;
                }
                changed.await;
            }
        }

        async fn wait_for_stopped(&self) {
            loop {
                let changed = self.state_changed.notified();
                if self.stopped.load(Ordering::SeqCst) {
                    return;
                }
                changed.await;
            }
        }
    }

    impl CodeModeHost for LifecycleHost {
        fn invoke_tool(
            &self,
            _request: CodeModeToolRequest,
        ) -> CodeModeHostFuture<'_, Result<CodeModeToolResponse, CodeModeHostError>> {
            Box::pin(async move {
                self.started.fetch_add(1, Ordering::SeqCst);
                self.state_changed.notify_waiters();
                let permit = self.release.acquire().await.map_err(|_| {
                    CodeModeHostError::new("lifecycle test release semaphore closed")
                })?;
                permit.forget();
                self.completed.fetch_add(1, Ordering::SeqCst);
                self.state_changed.notify_waiters();
                Ok(CodeModeToolResponse {
                    success: true,
                    output: json!({"ok": true}),
                    error: None,
                })
            })
        }

        fn stop_accepting_calls(&self) {
            self.stopped.store(true, Ordering::SeqCst);
            self.state_changed.notify_waiters();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn effectful_timeout_closes_frontend_and_drains_started_host_call() {
        let host = Arc::new(LifecycleHost::new());
        let mut req = request(
            "const child = tools.effect({}); while (true) {}",
            &["effect"],
        );
        req.timeout_ms = Some(1_000);
        let host_for_execute = host.clone();
        let task = tokio::spawn(async move {
            execute_with_termination_mode(
                host_for_execute,
                req,
                CodeModeTerminationMode::DrainStartedChildren {
                    max_drain_ms: 5_000,
                },
            )
            .await
        });

        host.wait_for_started(1).await;
        host.wait_for_stopped().await;
        assert_eq!(host.started.load(Ordering::SeqCst), 1);
        assert!(
            !task.is_finished(),
            "effect-aware return must wait for started host work"
        );
        host.release.add_permits(1);
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("drain must finish after the started host call completes")
            .unwrap()
            .unwrap_err();
        assert_eq!(result.kind, CodeModeErrorKind::Timeout);
        assert_eq!(host.completed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn effectful_timeout_bounds_drain_and_cancels_stuck_host_work() {
        let host = Arc::new(LifecycleHost::new());
        let mut req = request(
            "const child = tools.effect({}); while (true) {}",
            &["effect"],
        );
        req.timeout_ms = Some(50);
        let host_for_execute = host.clone();
        let task = tokio::spawn(async move {
            execute_with_termination_mode(
                host_for_execute,
                req,
                CodeModeTerminationMode::DrainStartedChildren { max_drain_ms: 50 },
            )
            .await
        });

        host.wait_for_started(1).await;
        host.wait_for_stopped().await;
        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("bounded effect-aware drain must not wait forever")
            .unwrap()
            .unwrap_err();
        assert_eq!(result.kind, CodeModeErrorKind::Timeout);
        assert_eq!(host.completed.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn effectful_frontend_success_does_not_hide_stuck_host_work() {
        let host = Arc::new(LifecycleHost::new());
        let req = request("tools.effect({}); text('frontend done');", &["effect"]);
        let host_for_execute = host.clone();
        let task = tokio::spawn(async move {
            execute_with_termination_mode(
                host_for_execute,
                req,
                CodeModeTerminationMode::DrainStartedChildren { max_drain_ms: 50 },
            )
            .await
        });

        host.wait_for_started(1).await;
        host.wait_for_stopped().await;
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("bounded drain must terminate a frontend-success call with stuck host work")
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind, CodeModeErrorKind::Runtime);
        assert!(error.message.contains("started-child drain exceeded 50 ms"));
        assert_eq!(host.completed.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn effectful_timeout_discards_runtime_queue_before_new_host_calls_start() {
        let host = Arc::new(LifecycleHost::new());
        let mut req = request(
            r#"
            for (let i = 0; i < 9; i++) tools.effect({i});
            while (true) {}
            "#,
            &["effect"],
        );
        req.timeout_ms = Some(1_000);
        let host_for_execute = host.clone();
        let task = tokio::spawn(async move {
            execute_with_termination_mode(
                host_for_execute,
                req,
                CodeModeTerminationMode::DrainStartedChildren {
                    max_drain_ms: 5_000,
                },
            )
            .await
        });

        host.wait_for_started(MAX_CONCURRENT_TOOL_CALLS).await;
        host.wait_for_stopped().await;
        assert_eq!(
            host.started.load(Ordering::SeqCst),
            MAX_CONCURRENT_TOOL_CALLS,
            "the runtime-queued ninth call must not start after frontend closure"
        );
        host.release.add_permits(MAX_CONCURRENT_TOOL_CALLS);
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("started host calls must drain")
            .unwrap()
            .unwrap_err();
        assert_eq!(result.kind, CodeModeErrorKind::Timeout);
        assert_eq!(
            host.started.load(Ordering::SeqCst),
            MAX_CONCURRENT_TOOL_CALLS
        );
        assert_eq!(
            host.completed.load(Ordering::SeqCst),
            MAX_CONCURRENT_TOOL_CALLS
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn e1_timeout_does_not_wait_for_blocked_host_call() {
        let host = Arc::new(LifecycleHost::new());
        let mut req = request(
            "const child = tools.fake_read({}); while (true) {}",
            &["fake_read"],
        );
        req.timeout_ms = Some(1_000);
        let host_for_execute = host.clone();
        let task = tokio::spawn(async move { execute(host_for_execute, req).await });

        host.wait_for_started(1).await;
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("E1 timeout must remain the return boundary")
            .unwrap()
            .unwrap_err();
        assert_eq!(result.kind, CodeModeErrorKind::Timeout);
        assert!(!host.stopped.load(Ordering::SeqCst));
        assert_eq!(host.completed.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cpu_bound_infinite_loop_is_terminated_by_deadline() {
        let mut request = request("while (true) {}", &[]);
        request.timeout_ms = Some(50);
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            execute(Arc::new(RecordingHost::default()), request),
        )
        .await
        .expect("V8 isolate termination must stop the runtime thread")
        .unwrap_err();
        assert_eq!(result.kind, CodeModeErrorKind::Timeout);
    }

    #[tokio::test]
    async fn ambient_host_authority_is_absent() {
        let result = execute(
            Arc::new(RecordingHost::default()),
            request(
                r#"
                text({
                    console: typeof console,
                    wasm: typeof WebAssembly,
                    atomics: typeof Atomics,
                    shared: typeof SharedArrayBuffer,
                    process: typeof process,
                    deno: typeof Deno,
                    fetch: typeof fetch,
                    require: typeof require
                });
                "#,
                &[],
            ),
        )
        .await
        .unwrap();
        let emitted: JsonValue = serde_json::from_str(&result.content[0]).unwrap();
        for key in [
            "console", "wasm", "atomics", "shared", "process", "deno", "fetch", "require",
        ] {
            assert_eq!(emitted[key], "undefined", "{key}");
        }
    }
}

# Chadex W3 desktop helper bridge

Independent `chadex-desktop-bridge` Rust crate (`[workspace]`), usable from
Tauri without UI dependencies. It launches the **existing** `chadex-helper`;
it depends only on the existing process-ownership crate, not the runtime
execution core. No helper protocol, runtime implementation, or W2 semantics
are forked. Tauri owns its RPC whitelist and must discard any UI/backend
snapshot when `health().state` becomes `failed`.

```rust,ignore
use chadex_desktop_bridge::Bridge;
use serde_json::json;

let bridge = Bridge::launch(&helper_path, &runtime_dir, &data_dir)?;
let status = bridge.request("getStatus", json!({})).await?;
let health = bridge.health(); // Serialize: {state, pid, error}
bridge.shutdown().await?;
```

API:

- `Bridge::launch(&Path, &Path, &Path) -> Result<Bridge, BridgeError>` is
  synchronous; no Tokio runtime is required at launch. For UI callers, launch
  on their blocking executor. There is no automatic restart or request replay.
- `async request(&self, &str, Value) -> Result<Value, BridgeError>` requires a
  Tokio runtime with time enabled. It accepts existing RPCs, preserving result
  JSON and string `request_id` routing, including concurrent out-of-order replies.
- `health(&self) -> Health` is synchronous and serializable. States are
  `running`, `stopping`, `stopped`, `failed`. Running describes the transport,
  not runtime/tunnel/ChatGPT readiness. No backend snapshot is cached.
- `async shutdown(&self) -> Result<(), BridgeError>` reports graceful exit or
  a structured failure; `shutdown_forced` means the owned tree was reclaimed
  after grace expired and **does not pass** the graceful smoke gate.
- `request_timeout(&str) -> Duration` exposes the Swift/W2 budget policy:
  120s for connect/start/configure/resume, 30s for activate/switch/stop/disconnect,
  1.5s for shutdown, and 10s for other RPCs. Deadlines include queueing and
  writing. Cancellation removes pending requests; late/unknown IDs are discarded.

## Environment and ownership

The helper receives `CHADEX_DATA_DIR`, `CHADEX_RUNTIME_BIN_DIR`,
`CHADEX_RESOURCE_DIR`, and `PYTHONDONTWRITEBYTECODE=1`, with legacy
`WEBCODEX_DESKTOP_BIN_DIR` removed, matching W2 isolation. Paths are canonicalized
and cwd is the supplied data directory. Other caller environment variables,
including Graphify/proxy configuration, remain inherited without inspection.

For release helpers, pass the **packaged** `<resources>/chadex-runtime`
directory: `CHADEX_RESOURCE_DIR` becomes its parent. For unbundled debug
helpers it is `<data_dir>/bridge-resources`, preserving W2's explicit binary
override. A release helper still rejects an unbundled debug directory through
its existing fail-closed runtime resolution. This API does not change the
helper's Windows default data path; Tauri chooses the supplied data directory.

`chadex-runtime-process::ManagedChild` spawns the Windows helper suspended,
assigns its private Job Object, then resumes it. `KILL_ON_JOB_CLOSE` and
**no silent breakaway** cover helper descendants even when the desktop parent
is terminated without Rust destructors. `CREATE_NO_WINDOW` matches W2. Inner
W2 jobs may nest; ownership failures fail launch closed. Unix tests use the
existing private process-group backend; abrupt Unix parent death is not
claimed to have Windows kernel cleanup semantics.

Shutdown sends the existing RPC, closes stdin (W2 EOF cleanup), waits up to
5s total for helper **and tree** exit, then uses only the owned tree fallback
and observes/reaps exit. An ACK alone is not completion. A monitor retains the
cleanup deadline even if the shutdown future is cancelled. Owner drop also
terminates its owned tree. Death/EOF/protocol faults fail all pending requests,
invalidate health and reclaim descendants. No shared process is enumerated or
killed by name/PID.

## Bounds and sensitive data

At most 64 pending requests and 64 queued frames; request JSON is bounded to
256 KiB before newline (the helper's bound), response JSON to 1 MiB. Invalid,
truncated, oversized, and incompatible-version frames fail closed. A blocked
stdin writer is bounded by its method deadline; a partial frame timeout
invalidates the transport instead of corrupting the next request.

Stderr goes to the null device, so no raw logs are stored or printed. Transport
frames are transient zeroizing buffers; params and discarded metadata strings
are wiped. Returned `Value` belongs to the caller; it must avoid logging or
persisting sensitive results. `BridgeError`/health never contain raw paths,
params, credentials, helper output, or free-form backend error text. Backend
errors retain only finite known W2/helper codes (`unclassified` otherwise),
with safe scalar protocol/exit metadata. This error projection does not alter
RPC execution or results.

## Local checks and Windows CI

From the repository root (all bridge build outputs stay in this subtree):

```sh
cargo test --locked --manifest-path apps/windows/bridge/Cargo.toml --features test-helper
cargo clippy --locked --manifest-path apps/windows/bridge/Cargo.toml --all-targets --features test-helper -- -D warnings
cargo fmt --manifest-path apps/windows/bridge/Cargo.toml --check
cargo build --locked --manifest-path apps/windows/bridge/Cargo.toml --bin chadex-bridge-smoke
```

Tests launch a compiled fake helper over **real OS pipes**, exercising request
routing, protocol mismatch, real deadlines, death, bounds, cancellation,
secret-free errors/stderr, environment, graceful tree cleanup and forced
shutdown. On native Windows the same suite additionally runs
`windows_parent_crash_job_object_kills_helper_and_descendants`: it kills an
ordinary bridge-owner child with `TerminateProcess` (no outer ManagedChild),
and checks both helper and descendant with native process handles. A macOS
test pass does not certify that Windows-only test.

The production build does not include the fake-helper binary unless explicitly
enabled with `--features test-helper`.

After the existing W2 real-helper/runtime builds, the main agent can add this
native Windows smoke invocation without modifying shared CI in this task:

```powershell
cargo test --locked --manifest-path apps/windows/bridge/Cargo.toml --features test-helper
if ($LASTEXITCODE -ne 0) { throw "Bridge transport tests failed" }
cargo build --locked --manifest-path apps/windows/bridge/Cargo.toml --bin chadex-bridge-smoke
if ($LASTEXITCODE -ne 0) { throw "Bridge smoke build failed" }
& apps/windows/bridge/target/debug/chadex-bridge-smoke.exe `
  --helper rust-helper/target/debug/chadex-helper.exe `
  --runtime-dir chadex-runtime/target/debug `
  --output apps/windows/bridge/bridge-smoke.json
if ($LASTEXITCODE -ne 0) { throw "Bridge real-helper smoke failed" }
```

With shared `CARGO_TARGET_DIR`, point the executable/helper/runtime paths to
that target's debug directory. The smoke executable owns a temporary Unicode/
space-containing fixture and data directory and emits bounded, secret-free
JSON with exit 0 only after all stages and graceful cleanup pass. Stages are
`inspect` (`inspectProject`), `activate` (`activateProject`), `getStatus`,
`getActivity` (**existing `queryActivities`**), `disconnect` (`disconnectAI`),
and `shutdown`. It never configures a runtime, starts a tunnel, submits a
credential, calls model tools, or touches an existing project workflow.
It does not certify default desktop resource/data discovery, connected MCP,
Tauri UI integration, or Windows hardware.

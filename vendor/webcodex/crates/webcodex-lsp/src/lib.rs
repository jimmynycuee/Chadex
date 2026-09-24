//! Native bounded Language Server Protocol implementation for WebCodex Runners.
//!
//! Runner policy, Project registration, and model-facing tool authority remain
//! outside this crate. Callers provide an already authorized canonical Project
//! root; this crate owns language-server discovery, process lifecycle, protocol
//! framing, position conversion, and bounded semantic navigation.

mod language;
mod navigation;
mod position;
mod protocol;
mod supervisor;

pub(crate) use webcodex_core::lsp_bridge;

pub use navigation::execute_lsp_operation;
pub use position::MAX_LSP_DOCUMENT_BYTES;
pub use supervisor::{
    LspCommand, LspServerKind, LspShutdownOutcome, LspSupervisor, LspSupervisorConfig,
};

#[cfg(test)]
mod test_support;

// ManagedChild's documented Windows spawn path takes a system-wide Toolhelp
// thread snapshot. Fake LSP tests use tight protocol deadlines, so serialize
// those tests on Windows only. Linux/macOS test concurrency is unaffected.
#[cfg(all(test, windows))]
static FAKE_LSP_TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(all(test, windows))]
fn serialize_fake_lsp_test() -> std::sync::MutexGuard<'static, ()> {
    FAKE_LSP_TEST_SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(all(test, not(windows)))]
struct FakeLspTestSerialGuard;

#[cfg(all(test, not(windows)))]
fn serialize_fake_lsp_test() -> FakeLspTestSerialGuard {
    FakeLspTestSerialGuard
}

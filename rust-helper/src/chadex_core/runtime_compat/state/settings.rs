use std::time::Duration;

pub(super) const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const RUNNER_READY_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const PROJECT_READY_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const QUICK_SHARE_READY_TIMEOUT: Duration = Duration::from_secs(90);
pub(super) const REGULAR_TUNNEL_READY_TIMEOUT: Duration = Duration::from_secs(90);
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(300);
pub(super) const STARTUP_FAST_POLL_INTERVAL: Duration = Duration::from_millis(25);
pub(super) const STARTUP_MEDIUM_POLL_INTERVAL: Duration = Duration::from_millis(100);
pub(super) const STARTUP_FAST_POLL_WINDOW: Duration = Duration::from_secs(1);
pub(super) const STARTUP_MEDIUM_POLL_WINDOW: Duration = Duration::from_secs(3);
pub(super) const READINESS_CLEANUP_SLACK: Duration = Duration::from_secs(2);
pub(super) const SHUTDOWN_OPERATION_WAIT: Duration = Duration::from_secs(5);
pub(super) const DESKTOP_STATE_MAX_BYTES: u64 = 256 * 1024;
pub(super) const DESKTOP_SERVER_ENV_MAX_BYTES: u64 = 256 * 1024;
pub(super) const DESKTOP_MCP_COMPACT_SCHEMAS: &str = "true";

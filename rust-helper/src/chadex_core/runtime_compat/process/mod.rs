mod events;
mod registry;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

pub(crate) use events::MachineEventInbox as MachineEventReceiver;
pub use registry::LifecycleRegistry as ProcessSupervisor;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    LocalServer,
    LocalRunner,
    QuickShare,
    RegularTunnel,
}

impl ProcessKind {
    pub(super) fn activity_source(self) -> &'static str {
        match self {
            Self::LocalServer => "service",
            Self::LocalRunner => "runner",
            Self::QuickShare => "quick_share",
            Self::RegularTunnel => "regular_tunnel",
        }
    }

    pub(super) fn keeps_full_eof_grace(self) -> bool {
        matches!(self, Self::QuickShare | Self::RegularTunnel)
    }

    pub(super) fn allows_child_breakaway(self) -> bool {
        self == Self::LocalRunner
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessPhase {
    Starting,
    Running,
    Stopping,
    Exited,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub kind: ProcessKind,
    pub phase: ProcessPhase,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub owned_by_desktop: bool,
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn runtime_process_surface_is_intentionally_closed() {
        let kinds = [
            ProcessKind::LocalServer,
            ProcessKind::LocalRunner,
            ProcessKind::QuickShare,
            ProcessKind::RegularTunnel,
        ];
        assert_eq!(kinds.len(), 4);
        assert!(ProcessKind::LocalRunner.allows_child_breakaway());
        assert!(!ProcessKind::LocalServer.allows_child_breakaway());
    }
}

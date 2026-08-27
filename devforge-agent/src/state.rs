use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AgentState {
    #[default]
    Idle,
    Analyzing,
    Planning,
    WaitingPermission,
    ExecutingTool,
    Editing,
    Running,
    Verifying,
    Error,
    Completed,
    Cancelled,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Analyzing => "Analyzing",
            Self::Planning => "Planning",
            Self::WaitingPermission => "Waiting for permission",
            Self::ExecutingTool => "Executing tool",
            Self::Editing => "Editing",
            Self::Running => "Running",
            Self::Verifying => "Verifying",
            Self::Error => "Error",
            Self::Completed => "Completed",
            Self::Cancelled => "Cancelled",
        }
    }
}

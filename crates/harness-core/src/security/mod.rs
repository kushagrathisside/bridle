//! Security policy (§25–§27, §34): what a tool call may do, where it may touch, and what must
//! never reach a model or a log.
//!
//! - [`path_guard`]: keeps file operations inside the authorised workspace.
//! - [`redact`]: removes secrets from text and JSON before logging or model context.
//! - [`command_risk`]: classifies shell commands into [`ToolClass`]es by inspecting the actual
//!   command text, segment by segment.
//! - [`tool_policy`]: combines the three into an allow / ask / deny verdict for a tool call.

pub mod command_risk;
pub mod path_guard;
pub mod redact;
pub mod tool_policy;

use serde::{Deserialize, Serialize};

/// Tool classes (§25), ordered from least to most risky. A compound command takes the class of
/// its riskiest segment, so the ordering is load-bearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolClass {
    ReadOnly,
    Write,
    ExecuteSafe,
    ExecuteRisky,
    Network,
    Destructive,
    Privileged,
}

impl ToolClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "READ_ONLY",
            Self::Write => "WRITE",
            Self::ExecuteSafe => "EXECUTE_SAFE",
            Self::ExecuteRisky => "EXECUTE_RISKY",
            Self::Network => "NETWORK",
            Self::Destructive => "DESTRUCTIVE",
            Self::Privileged => "PRIVILEGED",
        }
    }
}

impl std::fmt::Display for ToolClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::ToolClass;

    #[test]
    fn ordering_is_by_risk() {
        assert!(ToolClass::ReadOnly < ToolClass::Write);
        assert!(ToolClass::ExecuteRisky < ToolClass::Network);
        assert!(ToolClass::Destructive < ToolClass::Privileged);
    }

    #[test]
    fn serialises_screaming_snake() {
        let s = serde_json::to_string(&ToolClass::ExecuteSafe).expect("serialise");
        assert_eq!(s, "\"EXECUTE_SAFE\"");
    }
}

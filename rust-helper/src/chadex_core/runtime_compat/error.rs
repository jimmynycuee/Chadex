use serde::Serialize;
use serde_json::Value;
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DesktopError {
    pub code: String,
    pub message: String,
    pub next_action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl DesktopError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        next_action: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            next_action: next_action.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl Display for DesktopError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DesktopError {}

pub type DesktopResult<T> = Result<T, DesktopError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_are_optional_and_display_is_stable() {
        let base = DesktopError::new("runtime_error", "Runtime failed", "Retry.");
        assert_eq!(base.to_string(), "runtime_error: Runtime failed");
        assert!(base.details.is_none());

        let detailed = base.with_details(serde_json::json!({"step": "probe"}));
        assert_eq!(detailed.details.unwrap()["step"], "probe");
    }
}

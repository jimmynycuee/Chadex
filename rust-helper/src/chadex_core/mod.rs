pub mod activity;
pub(crate) mod adapters;
pub(crate) mod backend;
pub mod credentials;
pub(crate) mod graphify;
pub mod performance;
pub mod runtime;
pub(crate) mod runtime_compat;
pub mod tunnel;
pub mod verification;

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ChadexError {
    pub code: String,
    pub message: String,
    pub recovery: String,
    pub details: Option<Value>,
}

impl ChadexError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        recovery: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            recovery: recovery.into(),
            details: None,
        }
    }
}

pub type ChadexResult<T> = Result<T, ChadexError>;

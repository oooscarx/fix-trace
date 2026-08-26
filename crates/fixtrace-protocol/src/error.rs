use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    NotInitialized,
    AlreadyInitialized,
    IncompatibleProtocol,
    NotFound,
    Conflict,
    OperationInProgress,
    InvalidTransition,
    ApprovalRequired,
    ApprovalResolved,
    Cancelled,
    BudgetExceeded,
    SandboxDenied,
    Unauthorized,
    EventGap,
    FrameTooLarge,
    Internal,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct AppErrorView {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub details: Option<Value>,
}

impl AppErrorView {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: false,
            details: None,
        }
    }
}

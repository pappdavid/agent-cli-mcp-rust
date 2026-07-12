use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidService,
    InvalidMode,
    UnsupportedCapability,
    CwdNotAllowed,
    NotAWorktree,
    BinaryNotFound,
    CommandBlocked,
    SecretRedactionTriggered,
    RunTimeout,
    SessionNotFound,
    Quarantined,
    BackendFailed,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::InvalidService => "INVALID_SERVICE",
            ErrorCode::InvalidMode => "INVALID_MODE",
            ErrorCode::UnsupportedCapability => "UNSUPPORTED_CAPABILITY",
            ErrorCode::CwdNotAllowed => "CWD_NOT_ALLOWED",
            ErrorCode::NotAWorktree => "NOT_A_WORKTREE",
            ErrorCode::BinaryNotFound => "BINARY_NOT_FOUND",
            ErrorCode::CommandBlocked => "COMMAND_BLOCKED",
            ErrorCode::SecretRedactionTriggered => "SECRET_REDACTION_TRIGGERED",
            ErrorCode::RunTimeout => "RUN_TIMEOUT",
            ErrorCode::SessionNotFound => "SESSION_NOT_FOUND",
            ErrorCode::Quarantined => "QUARANTINED",
            ErrorCode::BackendFailed => "BACKEND_FAILED",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub ok: bool,
    #[serde(rename = "errorCode")]
    pub error_code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct AgentCliError {
    pub error_code: ErrorCode,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl AgentCliError {
    pub fn new(error_code: ErrorCode, message: &str) -> Self {
        AgentCliError {
            error_code,
            message: message.to_string(),
            details: None,
        }
    }

    pub fn new_with_details(
        error_code: ErrorCode,
        message: &str,
        details: serde_json::Value,
    ) -> Self {
        AgentCliError {
            error_code,
            message: message.to_string(),
            details: Some(details),
        }
    }

    pub fn to_response(&self) -> ErrorResponse {
        ErrorResponse {
            ok: false,
            error_code: self.error_code.as_str().to_string(),
            message: format!("[{}] {}", self.error_code.as_str(), self.message),
            details: self.details.clone(),
        }
    }
}

impl fmt::Display for AgentCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.error_code.as_str(), self.message)
    }
}

impl std::error::Error for AgentCliError {}

pub fn to_mcp_error_content(err: &(dyn std::error::Error + 'static)) -> serde_json::Value {
    if let Some(cli_err) = err.downcast_ref::<AgentCliError>() {
        serde_json::to_value(cli_err.to_response()).unwrap_or_default()
    } else {
        serde_json::to_value(ErrorResponse {
            ok: false,
            error_code: "BACKEND_FAILED".to_string(),
            message: err.to_string(),
            details: None,
        })
        .unwrap_or_default()
    }
}

use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidArguments,
    InvalidPeriod,
    ConfigurationError,
    AuthenticationError,
    CollectionError,
    ReportError,
    OutputError,
}

#[derive(Debug, Serialize)]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
    pub details: Value,
}

impl CommandError {
    pub fn new(code: ErrorCode, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
            details: json!({}),
        }
    }

    pub fn source(mut self, source: &str) -> Self {
        self.details = json!({"source": source});
        self
    }

    pub fn exit_code(&self) -> u8 {
        match self.code {
            ErrorCode::OutputError => 1,
            ErrorCode::InvalidArguments | ErrorCode::InvalidPeriod => 2,
            ErrorCode::ConfigurationError => 3,
            ErrorCode::AuthenticationError => 4,
            ErrorCode::CollectionError | ErrorCode::ReportError => 5,
        }
    }
}

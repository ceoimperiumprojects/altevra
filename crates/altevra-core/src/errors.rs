use thiserror::Error;

#[derive(Debug, Error)]
pub enum AltevraError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Not found: {entity} with id {id}")]
    NotFound { entity: String, id: String },

    #[error("Skill error: {0}")]
    Skill(String),

    #[error("Hook error: {0}")]
    Hook(String),

    #[error("Adapter error: {0}")]
    Adapter(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Drift detected in managed file: {path}")]
    DriftDetected { path: String },

    #[error("Security violation: {0}")]
    SecurityViolation(String),
}

impl AltevraError {
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    pub fn skill(msg: impl Into<String>) -> Self {
        Self::Skill(msg.into())
    }

    pub fn hook(msg: impl Into<String>) -> Self {
        Self::Hook(msg.into())
    }

    pub fn adapter(msg: impl Into<String>) -> Self {
        Self::Adapter(msg.into())
    }

    pub fn database(msg: impl Into<String>) -> Self {
        Self::Database(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, AltevraError>;

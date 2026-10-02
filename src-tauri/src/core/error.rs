use serde::{ser::SerializeStruct, Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Clipboard(String),

    /// 局域网同步给用户看的根因（配对码错误、连不上对方等）。
    #[error("{0}")]
    Sync(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AppError {
    fn kind(&self) -> &'static str {
        match self {
            AppError::Clipboard(_) => "Clipboard",
            AppError::Sync(_) => "Sync",
            AppError::Other(_) => "Other",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("AppError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

//! Defines errors returned while configuring, organizing, and undoing Maid operations.

use std::fmt;

#[derive(Debug)]
pub enum MaidError {
    Io(std::io::Error),
    InvalidDirectory(String),
    UndoFailed(String),
    ConfigError(String),
    DestinationExhausted(String),
}

impl fmt::Display for MaidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaidError::Io(e) => write!(f, "IO error: {}", e),
            MaidError::InvalidDirectory(path) => write!(f, "Invalid directory: {}", path),
            MaidError::UndoFailed(reason) => write!(f, "Undo failed: {}", reason),
            MaidError::ConfigError(reason) => write!(f, "Config error: {}", reason),
            MaidError::DestinationExhausted(reason) => {
                write!(f, "Destination exhausted: {}", reason)
            }
        }
    }
}

impl std::error::Error for MaidError {}

impl From<std::io::Error> for MaidError {
    fn from(e: std::io::Error) -> Self {
        MaidError::Io(e)
    }
}

impl From<serde_json::Error> for MaidError {
    fn from(e: serde_json::Error) -> Self {
        MaidError::Io(std::io::Error::other(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_exhausted_renders_reason() {
        let err = MaidError::DestinationExhausted("no free name for 'invoice.md'".into());
        assert_eq!(
            err.to_string(),
            "Destination exhausted: no free name for 'invoice.md'"
        );
    }

    #[test]
    fn invalid_directory_renders_path() {
        let err = MaidError::InvalidDirectory("/nope".into());
        assert_eq!(err.to_string(), "Invalid directory: /nope");
    }
}

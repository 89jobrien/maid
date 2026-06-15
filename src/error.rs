use std::fmt;

#[derive(Debug)]
pub enum MaidError {
    Io(std::io::Error),
    InvalidDirectory(String),
    UndoFailed(String),
    ConfigError(String),
}

impl fmt::Display for MaidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaidError::Io(e) => write!(f, "IO error: {}", e),
            MaidError::InvalidDirectory(path) => write!(f, "Invalid directory: {}", path),
            MaidError::UndoFailed(reason) => write!(f, "Undo failed: {}", reason),
            MaidError::ConfigError(reason) => write!(f, "Config error: {}", reason),
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
        MaidError::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        ))
    }
}

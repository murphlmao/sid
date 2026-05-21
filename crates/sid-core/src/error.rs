use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SidError {
    #[error("storage error: {0}")]
    Storage(String),

    #[error("io error reading {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("widget '{0}' not registered")]
    UnknownWidget(String),

    #[error("action '{0}' not registered")]
    UnknownAction(String),

    #[error("invalid keybind: {0}")]
    InvalidKeybind(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = SidError> = std::result::Result<T, E>;

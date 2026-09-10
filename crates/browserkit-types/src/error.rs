/// Broad source of a BrowserKit failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Runtime,
    Window,
    WebView,
    Geometry,
    UnsupportedHostMode,
}

/// BrowserKit error.
#[derive(Debug, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

/// BrowserKit result.
pub type Result<T> = std::result::Result<T, Error>;

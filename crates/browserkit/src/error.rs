#[derive(Debug, thiserror::Error)]
pub enum BrowserKitError {
    #[error("runtime error: {0}")]
    Runtime(#[from] browserkit_runtime::Error),
    #[error("invalid BrowserKit configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("window already exists")]
    WindowAlreadyExists,
    #[error("window not found")]
    WindowNotFound,
    #[error("page not found: {0:?}")]
    PageNotFound(browserkit_types::PageId),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("page creation failed: {0}")]
    PageCreationFailed(String),
    #[error("page close failed: {0}")]
    PageCloseFailed(String),
    #[error("runtime is not ready")]
    RuntimeNotReady,
    #[error("invalid active page")]
    InvalidActivePage,
    #[error("invalid page view bounds: {0}")]
    InvalidViewBounds(String),
}

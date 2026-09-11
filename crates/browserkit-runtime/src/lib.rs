mod cef;
mod runtime;

pub use runtime::{BrowserKitRuntime, BrowserKitRuntimeOptions};

pub use browserkit_cef::{CommandQueue, PageCommand};

pub fn navigate(
    queue: &CommandQueue,
    page_id: browserkit_types::PageId,
    url: String,
) -> std::result::Result<(), Error> {
    browserkit_cef::enqueue(queue, PageCommand::Navigate { page_id, url })
        .map_err(|e| Error::Cef(e.to_string()))
}
pub fn create_page(
    queue: &CommandQueue,
    window_id: browserkit_types::WindowId,
    page: browserkit_types::PageState,
) -> std::result::Result<(), Error> {
    browserkit_cef::enqueue(queue, PageCommand::Create { window_id, page })
        .map_err(|e| Error::Cef(e.to_string()))
}
pub fn page_command(
    queue: &CommandQueue,
    page_id: browserkit_types::PageId,
    command: PageCommand,
) -> std::result::Result<(), Error> {
    let command = match command {
        PageCommand::Reload { .. } => PageCommand::Reload { page_id },
        PageCommand::GoBack { .. } => PageCommand::GoBack { page_id },
        PageCommand::GoForward { .. } => PageCommand::GoForward { page_id },
        PageCommand::Stop { .. } => PageCommand::Stop { page_id },
        other => other,
    };
    browserkit_cef::enqueue(queue, command).map_err(|e| Error::Cef(e.to_string()))
}
pub fn activate_page(
    queue: &CommandQueue,
    window_id: browserkit_types::WindowId,
    page_id: browserkit_types::PageId,
) -> std::result::Result<(), Error> {
    browserkit_cef::enqueue(queue, PageCommand::Activate { window_id, page_id })
        .map_err(|e| Error::Cef(e.to_string()))
}
pub fn close_page(
    queue: &CommandQueue,
    window_id: browserkit_types::WindowId,
    page_id: browserkit_types::PageId,
) -> std::result::Result<(), Error> {
    browserkit_cef::enqueue(queue, PageCommand::Close { window_id, page_id })
        .map_err(|e| Error::Cef(e.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("CEF error: {0}")]
    Cef(String),
    #[error("invalid runtime configuration: {0}")]
    InvalidConfiguration(&'static str),
}

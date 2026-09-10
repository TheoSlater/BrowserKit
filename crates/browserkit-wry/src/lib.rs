//! Low-level compatibility boundary around the vendored Wry fork.

mod webview;

pub use webview::{NativeRoot, WebView};

use browserkit_types::{Error, ErrorKind, PageOptions, Result, WebViewHostMode};
use tao::window::Window;

pub trait PlatformBackend {
    fn create_page(
        &self,
        window: &Window,
        root: &NativeRoot,
        options: &PageOptions,
    ) -> Result<WebView>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WryBackend;

impl PlatformBackend for WryBackend {
    fn create_page(
        &self,
        window: &Window,
        root: &NativeRoot,
        options: &PageOptions,
    ) -> Result<WebView> {
        ensure_host_mode(options.host_mode)?;
        WebView::new(window, root, options.url.as_deref())
    }
}

fn ensure_host_mode(mode: WebViewHostMode) -> Result<()> {
    if mode == WebViewHostMode::Composition {
        Err(Error::new(
            ErrorKind::UnsupportedHostMode,
            "Composition host mode is not implemented",
        ))
    } else {
        Ok(())
    }
}

pub fn initialize() -> Result<()> {
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    gtk::init().map_err(|error| Error::new(ErrorKind::Runtime, error.to_string()))?;
    Ok(())
}

pub fn pump_events() {
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

pub(crate) fn map_error(error: impl std::fmt::Display) -> Error {
    Error::new(ErrorKind::WebView, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::ensure_host_mode;
    use browserkit_types::WebViewHostMode;

    #[test]
    fn composition_is_explicitly_unsupported() {
        assert!(ensure_host_mode(WebViewHostMode::Composition).is_err());
        assert!(ensure_host_mode(WebViewHostMode::NativeChild).is_ok());
    }
}

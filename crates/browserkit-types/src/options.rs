/// Browser-wide configuration.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrowserOptions {}

/// Native window configuration.
#[derive(Debug, Clone)]
pub struct WindowOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    /// Show BrowserKit's Linux-only native overlay smoke widget.
    pub debug_native_overlay: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "BrowserKit".into(),
            width: 1200,
            height: 800,
            debug_native_overlay: false,
        }
    }
}

/// Page configuration.
#[derive(Debug, Clone)]
pub struct PageOptions {
    pub url: Option<String>,
    pub host_mode: WebViewHostMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebViewHostMode {
    NativeChild,
    Composition,
}

impl Default for PageOptions {
    fn default() -> Self {
        Self {
            url: None,
            host_mode: WebViewHostMode::NativeChild,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_child_is_default_host_mode() {
        assert_eq!(
            PageOptions::default().host_mode,
            WebViewHostMode::NativeChild
        );
        assert!(!WindowOptions::default().debug_native_overlay);
    }
}

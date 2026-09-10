use browserkit_types::{LogicalRect, Result};
use tao::window::Window;
use wry::{
    dpi::{LogicalPosition, LogicalSize},
    WebViewBuilder,
};

/// BrowserKit-owned WebView wrapper.
pub struct WebView {
    inner: wry::WebView,
    bounds: Option<LogicalRect>,
}

pub struct NativeRoot {
    #[cfg(target_os = "linux")]
    fixed: gtk::Fixed,
}

impl NativeRoot {
    pub fn new(window: &Window) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use gtk::prelude::*;
            use tao::platform::unix::WindowExtUnix;
            let vbox = window.default_vbox().ok_or_else(|| {
                browserkit_types::Error::new(
                    browserkit_types::ErrorKind::Runtime,
                    "Tao default GTK container unavailable",
                )
            })?;
            let fixed = gtk::Fixed::new();
            vbox.pack_start(&fixed, true, true, 0);
            fixed.show_all();
            Ok(Self { fixed })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = window;
            Ok(Self {})
        }
    }
}

impl WebView {
    pub fn new(_window: &Window, root: &NativeRoot, url: Option<&str>) -> Result<Self> {
        let mut builder = WebViewBuilder::new();
        if let Some(url) = url {
            builder = builder.with_url(url);
        }
        #[cfg(target_os = "linux")]
        #[cfg(target_os = "linux")]
        use wry::WebViewBuilderExtUnix;

        #[cfg(target_os = "linux")]
        let inner = builder.build_gtk(&root.fixed).map_err(super::map_error)?;
        #[cfg(not(target_os = "linux"))]
        let inner = builder.build_as_child(_window).map_err(super::map_error)?;
        Ok(Self {
            inner,
            bounds: None,
        })
    }

    pub fn navigate(&self, url: &str) -> Result<()> {
        self.inner.load_url(url).map_err(super::map_error)
    }

    pub fn set_bounds(&mut self, bounds: LogicalRect) -> Result<()> {
        bounds.validate()?;
        if self.bounds == Some(bounds) {
            return Ok(());
        }
        self.inner
            .set_bounds(wry::Rect {
                position: LogicalPosition::new(bounds.x, bounds.y).into(),
                size: LogicalSize::new(bounds.width, bounds.height).into(),
            })
            .map_err(super::map_error)?;
        self.bounds = Some(bounds);
        Ok(())
    }

    pub fn set_visible(&self, visible: bool) -> Result<()> {
        self.inner.set_visible(visible).map_err(super::map_error)
    }

    pub fn focus(&self) -> Result<()> {
        self.inner.focus().map_err(super::map_error)
    }
}

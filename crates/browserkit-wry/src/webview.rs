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
    #[cfg(target_os = "linux")]
    overlay_fixed: gtk::Fixed,
    #[cfg(target_os = "linux")]
    overlay: Option<gtk::Button>,
    #[cfg(target_os = "linux")]
    overlay_bounds: Option<LogicalRect>,
}

#[cfg(target_os = "linux")]
const DEBUG_OVERLAY_BOUNDS: LogicalRect = LogicalRect {
    x: 760.0,
    y: 120.0,
    width: 220.0,
    height: 80.0,
};

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
            // Root follows the toplevel allocation; child requests must not resize it.
            fixed.set_size_request(0, 0);
            let overlay_host = gtk::Overlay::new();
            let overlay_fixed = gtk::Fixed::new();
            overlay_host.set_size_request(0, 0);
            overlay_fixed.set_size_request(0, 0);
            overlay_host.add(&fixed);
            overlay_host.add_overlay(&overlay_fixed);
            vbox.pack_start(&overlay_host, true, true, 0);
            overlay_host.show_all();
            Ok(Self {
                fixed,
                overlay_fixed,
                overlay: None,
                overlay_bounds: None,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = window;
            Ok(Self {})
        }
    }

    #[cfg(target_os = "linux")]
    pub fn add_debug_overlay(&mut self) -> Result<()> {
        if self.overlay.is_some() {
            return Ok(());
        }
        use gtk::prelude::*;
        let button = gtk::Button::with_label("BrowserKit Overlay");
        button.set_can_focus(true);
        button.connect_clicked(|_| eprintln!("BrowserKit overlay clicked"));
        // GtkOverlay owns this as a separate overlay layer, above the page GtkFixed.
        self.overlay_fixed.put(&button, 0, 0);
        button.show();
        self.overlay = Some(button);
        self.set_overlay_bounds(DEBUG_OVERLAY_BOUNDS)
    }

    #[cfg(target_os = "linux")]
    pub fn set_overlay_bounds(&mut self, bounds: LogicalRect) -> Result<()> {
        bounds.validate()?;
        if self.overlay_bounds == Some(bounds) {
            return Ok(());
        }
        if let Some(button) = &self.overlay {
            use gtk::prelude::*;
            // GTK3 widget coordinates are logical here. Scale conversion already happened
            // when Tao converted the physical WindowEvent::Resized size.
            let x = bounds.x.round() as i32;
            let y = bounds.y.round() as i32;
            let width = bounds.width.round() as i32;
            let height = bounds.height.round() as i32;
            button.set_size_request(width, height);
            self.overlay_fixed.move_(button, x, y);
            self.overlay_bounds = Some(bounds);
            #[cfg(debug_assertions)]
            eprintln!("BrowserKit overlay bounds applied: {bounds:?}");
        }
        Ok(())
    }
}

impl WebView {
    pub fn new(_window: &Window, root: &NativeRoot, url: Option<&str>) -> Result<Self> {
        let mut builder = WebViewBuilder::new();
        if let Some(url) = url {
            builder = builder.with_url(url);
        }
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
        #[cfg(debug_assertions)]
        eprintln!("BrowserKit page_bounds_requested: {bounds:?}");
        if self.bounds == Some(bounds) {
            #[cfg(debug_assertions)]
            eprintln!("BrowserKit page_bounds_deduplicated");
            return Ok(());
        }
        self.inner
            .set_bounds(wry::Rect {
                position: LogicalPosition::new(bounds.x, bounds.y).into(),
                size: LogicalSize::new(bounds.width, bounds.height).into(),
            })
            .map_err(super::map_error)?;
        self.bounds = Some(bounds);
        #[cfg(debug_assertions)]
        eprintln!("BrowserKit page_bounds_applied_gtk: {bounds:?}");
        Ok(())
    }

    pub fn set_visible(&self, visible: bool) -> Result<()> {
        self.inner.set_visible(visible).map_err(super::map_error)
    }

    #[cfg(target_os = "linux")]
    pub fn set_interactive(&self, interactive: bool) -> Result<()> {
        use gtk::prelude::*;
        use wry::WebViewExtUnix;
        self.inner.webview().set_sensitive(interactive);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn set_interactive(&self, _interactive: bool) -> Result<()> {
        Ok(())
    }

    pub fn focus(&self) -> Result<()> {
        self.inner.focus().map_err(super::map_error)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::DEBUG_OVERLAY_BOUNDS;

    #[test]
    fn debug_overlay_geometry_is_logical_and_valid() {
        assert!(DEBUG_OVERLAY_BOUNDS.validate().is_ok());
        assert_eq!(DEBUG_OVERLAY_BOUNDS.width, 220.0);
        assert_eq!(DEBUG_OVERLAY_BOUNDS.height, 80.0);
    }
}

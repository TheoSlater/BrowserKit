use std::{cell::RefCell, rc::Rc};

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
    requested: Rc<RefCell<Option<LogicalRect>>>,
    surface_name: &'static str,
}

pub struct ChromeWebView {
    webview: WebView,
    commands: Rc<RefCell<Vec<super::ChromeCommand>>>,
}

pub struct NativeRoot {
    #[cfg(target_os = "linux")]
    fixed: gtk::Fixed,
    #[cfg(target_os = "linux")]
    overlay_fixed: gtk::Fixed,
    #[cfg(target_os = "linux")]
    chrome_fixed: gtk::Fixed,
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
            let background = gtk::DrawingArea::new();
            let overlay_fixed = gtk::Fixed::new();
            let chrome_fixed = gtk::Fixed::new();
            overlay_host.set_size_request(0, 0);
            overlay_fixed.set_size_request(0, 0);
            chrome_fixed.set_size_request(0, 0);
            background.set_size_request(0, 0);
            for layer in [&fixed, &overlay_fixed, &chrome_fixed] {
                layer.set_hexpand(true);
                layer.set_vexpand(true);
                layer.set_halign(gtk::Align::Fill);
                layer.set_valign(gtk::Align::Fill);
            }
            // Overlay children are allocated over the neutral background but are not
            // included in GtkOverlay's preferred-size request. This keeps persistent
            // surface requests from resizing the Tao toplevel.
            overlay_host.add(&background);
            overlay_host.add_overlay(&fixed);
            overlay_host.add_overlay(&overlay_fixed);
            overlay_host.add_overlay(&chrome_fixed);
            vbox.pack_start(&overlay_host, true, true, 0);
            overlay_host.show_all();
            Ok(Self {
                fixed,
                overlay_fixed,
                chrome_fixed,
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

impl ChromeWebView {
    pub fn new(window: &Window, root: &NativeRoot) -> Result<Self> {
        #[cfg(target_os = "linux")]
        let _ = window;
        let commands = Rc::new(RefCell::new(Vec::new()));
        let command_queue = Rc::clone(&commands);
        let builder = WebViewBuilder::new()
            .with_html(CHROME_HTML)
            .with_ipc_handler(move |request| {
                if let Some(command) = parse_chrome_command(request.body()) {
                    #[cfg(debug_assertions)]
                    eprintln!("BrowserKit chrome_command_received: {command:?}");
                    command_queue.borrow_mut().push(command);
                }
            });
        #[cfg(target_os = "linux")]
        use wry::WebViewBuilderExtUnix;
        #[cfg(target_os = "linux")]
        let inner = builder
            .build_gtk(&root.chrome_fixed)
            .map_err(super::map_error)?;
        #[cfg(not(target_os = "linux"))]
        let inner = builder.build_as_child(window).map_err(super::map_error)?;
        Ok(Self {
            webview: WebView::from_inner(inner, "Chrome"),
            commands,
        })
    }

    pub fn set_bounds(&mut self, bounds: LogicalRect) -> Result<bool> {
        let before = self.webview.bounds;
        self.webview.set_bounds(bounds)?;
        Ok(before != self.webview.bounds)
    }

    pub fn set_visible(&self, visible: bool) -> Result<()> {
        self.webview.set_visible(visible)
    }

    pub fn focus(&self) -> Result<()> {
        self.webview.focus()
    }

    pub fn drain_commands(&self) -> Vec<super::ChromeCommand> {
        self.commands.borrow_mut().drain(..).collect()
    }
}

fn parse_chrome_command(body: &str) -> Option<super::ChromeCommand> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if value.get("type")?.as_str()? != "browserkit.command" {
        return None;
    }
    match value.get("command")?.as_str()? {
        "back" => Some(super::ChromeCommand::Back),
        "forward" => Some(super::ChromeCommand::Forward),
        "reload" => Some(super::ChromeCommand::Reload),
        _ => None,
    }
}

const CHROME_HTML: &str = r#"<!doctype html>
<meta charset="utf-8">
<style>
  html, body { margin: 0; height: 100%; background: #20242b; color: #f4f7fb;
    font: 14px sans-serif; }
  .toolbar { box-sizing: border-box; height: 56px; display: flex; align-items: center;
    gap: 8px; padding: 8px 16px; }
  button { padding: 7px 12px; }
  span { margin-left: 8px; font-weight: 600; }
</style>
<div class="toolbar">
  <button onclick="command('back')">Back</button>
  <button onclick="command('forward')">Forward</button>
  <button onclick="command('reload')">Reload</button>
  <span>BrowserKit Chrome</span>
</div>
<script>
  function command(command) {
    window.ipc.postMessage(JSON.stringify({type: 'browserkit.command', command}));
  }
</script>"#;

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
        Ok(Self::from_inner(inner, "Page"))
    }

    fn from_inner(inner: wry::WebView, surface_name: &'static str) -> Self {
        let requested: Rc<RefCell<Option<LogicalRect>>> = Rc::new(RefCell::new(None));
        #[cfg(target_os = "linux")]
        {
            use gtk::prelude::*;
            use wry::WebViewExtUnix;
            let widget = inner.webview();
            let requested_for_log = Rc::clone(&requested);
            widget.connect_size_allocate(move |widget, allocation| {
                let parent = widget.parent().map(|parent| parent.allocation());
                let root = widget
                    .parent()
                    .and_then(|parent| parent.parent())
                    .map(|root| root.allocation());
                #[cfg(debug_assertions)]
                let requested = *requested_for_log.borrow();
                #[cfg(debug_assertions)]
                eprintln!(
                    "BrowserKit surface allocation: surface={surface_name} requested={:?} size_request={}x{} actual={}x{} @ {},{} parent={parent:?} root={root:?}",
                    requested,
                    widget.width_request(),
                    widget.height_request(),
                    allocation.width(),
                    allocation.height(),
                    allocation.x(),
                    allocation.y(),
                );
                #[cfg(debug_assertions)]
                if let Some(requested) = requested {
                    let actual_width = f64::from(allocation.width());
                    let actual_height = f64::from(allocation.height());
                    if (actual_width - requested.width.round()).abs() > 1.0
                        || (actual_height - requested.height.round()).abs() > 1.0
                    {
                        eprintln!(
                            "BrowserKit warning: {surface_name} allocation differs from requested: requested={}x{} actual={}x{}",
                            requested.width,
                            requested.height,
                            actual_width,
                            actual_height
                        );
                    }
                }
            });
        }
        Self {
            inner,
            bounds: None,
            requested,
            surface_name,
        }
    }

    pub fn navigate(&self, url: &str) -> Result<()> {
        self.inner.load_url(url).map_err(super::map_error)
    }

    pub fn go_back(&self) -> Result<()> {
        self.inner.go_back().map_err(super::map_error)
    }

    pub fn go_forward(&self) -> Result<()> {
        self.inner.go_forward().map_err(super::map_error)
    }

    pub fn reload(&self) -> Result<()> {
        self.inner.reload().map_err(super::map_error)
    }

    pub fn set_bounds(&mut self, bounds: LogicalRect) -> Result<()> {
        bounds.validate()?;
        #[cfg(debug_assertions)]
        eprintln!(
            "BrowserKit {}_bounds_requested: {bounds:?}",
            self.surface_name.to_ascii_lowercase()
        );
        if self.bounds == Some(bounds) {
            #[cfg(debug_assertions)]
            eprintln!(
                "BrowserKit {}_bounds_deduplicated",
                self.surface_name.to_ascii_lowercase()
            );
            return Ok(());
        }
        *self.requested.borrow_mut() = Some(bounds);
        #[cfg(target_os = "linux")]
        {
            use gtk::prelude::*;
            let width = bounds.width.round().clamp(0.0, i32::MAX as f64) as i32;
            let height = bounds.height.round().clamp(0.0, i32::MAX as f64) as i32;
            // The root and layers have neutral requests, so this persistent child
            // request cannot become a toplevel request. It is required for GtkFixed
            // to retain the assigned size after a later relayout.
            use wry::WebViewExtUnix;
            self.inner.webview().set_size_request(width, height);
        }
        self.inner
            .set_bounds(wry::Rect {
                position: LogicalPosition::new(bounds.x, bounds.y).into(),
                size: LogicalSize::new(bounds.width, bounds.height).into(),
            })
            .map_err(super::map_error)?;
        self.bounds = Some(bounds);
        #[cfg(debug_assertions)]
        eprintln!(
            "BrowserKit {}_bounds_applied_gtk: {bounds:?}",
            self.surface_name.to_ascii_lowercase()
        );
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
    use super::{parse_chrome_command, DEBUG_OVERLAY_BOUNDS};

    #[test]
    fn chrome_commands_accept_only_supported_messages() {
        assert_eq!(
            parse_chrome_command(r#"{"type":"browserkit.command","command":"reload"}"#),
            Some(super::super::ChromeCommand::Reload)
        );
        assert_eq!(parse_chrome_command(r#"{"command":"reload"}"#), None);
        assert_eq!(parse_chrome_command("not-json"), None);
    }

    #[test]
    fn debug_overlay_geometry_is_logical_and_valid() {
        assert!(DEBUG_OVERLAY_BOUNDS.validate().is_ok());
        assert_eq!(DEBUG_OVERLAY_BOUNDS.width, 220.0);
        assert_eq!(DEBUG_OVERLAY_BOUNDS.height, 80.0);
    }
}

use std::{cell::RefCell, rc::Rc};

use browserkit_types::{LogicalRect, PageId, Result};
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
    events: super::PageEventQueue,
}

pub struct ChromeWebView {
    webview: WebView,
    messages: Rc<RefCell<Vec<String>>>,
}

pub struct NativeRoot {
    #[cfg(target_os = "linux")]
    overlay_host: gtk::Overlay,
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
                overlay_host,
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
    pub fn set_frontend_layering(&self) {
        use gtk::prelude::*;
        self.overlay_host.reorder_overlay(&self.chrome_fixed, 0);
        self.overlay_host.reorder_overlay(&self.fixed, 1);
        self.overlay_host.reorder_overlay(&self.overlay_fixed, 2);
    }

    #[cfg(not(target_os = "linux"))]
    pub fn set_frontend_layering(&self) {}

    #[cfg(target_os = "linux")]
    pub fn allocated_size(&self) -> Option<(f64, f64)> {
        use gtk::prelude::*;
        let allocation = self.fixed.allocation();
        if allocation.width() > 0 && allocation.height() > 0 {
            Some((
                f64::from(allocation.width()),
                f64::from(allocation.height()),
            ))
        } else {
            None
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn allocated_size(&self) -> Option<(f64, f64)> {
        None
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
    pub fn new(window: &Window, root: &NativeRoot, frontend_url: Option<&str>) -> Result<Self> {
        #[cfg(target_os = "linux")]
        let _ = window;
        let messages = Rc::new(RefCell::new(Vec::new()));
        let message_queue = Rc::clone(&messages);
        let builder = match frontend_url {
            Some(url) => WebViewBuilder::new().with_url(url).with_transparent(true),
            None => WebViewBuilder::new().with_html(CHROME_HTML),
        }
        .with_ipc_handler(move |request| {
            #[cfg(debug_assertions)]
            eprintln!("BrowserKit ipc_received");
            message_queue.borrow_mut().push(request.body().to_owned());
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
            webview: WebView::from_inner(inner, "Chrome", Rc::new(RefCell::new(Vec::new()))),
            messages,
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

    pub fn drain_messages(&self) -> Vec<String> {
        self.messages.borrow_mut().drain(..).collect()
    }

    pub fn send_message(&self, message: &browserkit_types::NativeMessage) -> Result<()> {
        let json = serde_json::to_string(message).map_err(|error| {
            super::map_error(format!("failed to serialize chrome message: {error}"))
        })?;
        self.webview
            .inner
            .evaluate_script(&format!("window.__browserkit.receive({json});"))
            .map_err(super::map_error)
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
  input { width: 280px; padding: 7px; }
  span { margin-left: 8px; font-weight: 600; }
</style>
<div class="toolbar">
  <button id="back">Back</button>
  <button id="forward">Forward</button>
  <button id="reload">Reload</button>
  <input id="url" placeholder="https://example.com">
  <button id="go">Go</button>
  <button id="new-page">New Page</button>
  <span>BrowserKit Chrome</span>
</div>
<script>
  const state = { activePageId: null, pages: new Map() };
  let nextRequestId = 1;
  function send(command) {
    window.ipc.postMessage(JSON.stringify({type: 'command', command}));
  }
  function request(request, onResponse) {
    const id = 'req-' + nextRequestId++;
    window.__browserkit.pending.set(id, onResponse);
    window.ipc.postMessage(JSON.stringify({type: 'request', id, request}));
  }
  function activePage() { return state.pages.get(state.activePageId); }
  function render() {
    const page = activePage();
    document.getElementById('back').disabled = !page || !page.canGoBack;
    document.getElementById('forward').disabled = !page || !page.canGoForward;
    document.getElementById('reload').disabled = !page;
    document.getElementById('url').value = page && page.url || '';
  }
  function updatePage(page) {
    state.pages.set(page.pageId, page);
    if (page.active) state.activePageId = page.pageId;
    render();
  }
  window.__browserkit = {
    pending: new Map(),
    receive(message) {
      if (message.type === 'response') {
        const callback = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (callback) callback(message);
        return;
      }
      if (message.type !== 'event') return;
      const event = message.event;
      if (event.type === 'page.created' || event.type === 'page.activated') {
        updatePage(event.state);
      } else if (event.type === 'page.closed') {
        state.pages.delete(event.pageId);
        if (state.activePageId === event.pageId) state.activePageId = null;
        render();
      } else {
        const page = state.pages.get(event.pageId);
        if (!page) return;
        if (event.type === 'page.url_changed') page.url = event.url;
        if (event.type === 'page.title_changed') page.title = event.title;
        if (event.type === 'page.loading_changed') page.loading = event.loading;
        if (event.type === 'page.navigation_state_changed') {
          page.canGoBack = event.canGoBack;
          page.canGoForward = event.canGoForward;
        }
        render();
      }
    }
  };
  document.getElementById('back').onclick = () => send({type: 'page.go_back'});
  document.getElementById('forward').onclick = () => send({type: 'page.go_forward'});
  document.getElementById('reload').onclick = () => send({type: 'page.reload'});
  document.getElementById('go').onclick = () => {
    const url = document.getElementById('url').value;
    send({type: 'page.navigate', url});
  };
  document.getElementById('new-page').onclick = () => request(
    {type: 'page.create', url: 'https://example.com'},
    response => {
      if (response.ok) send({type: 'page.activate', pageId: response.data.pageId});
    }
  );
  request({type: 'window.get_state'}, response => {
    if (!response.ok) return;
    state.activePageId = response.data.state.activePageId;
    response.data.state.pages.forEach(page => state.pages.set(page.pageId, page));
    render();
  });
  render();
</script>"#;

impl WebView {
    pub fn new(
        _window: &Window,
        root: &NativeRoot,
        url: Option<&str>,
        page_id: PageId,
        events: super::PageEventQueue,
    ) -> Result<Self> {
        let mut builder = WebViewBuilder::new();
        if let Some(url) = url {
            builder = builder.with_url(url);
        }
        let title_events = events.clone();
        builder = builder.with_document_title_changed_handler(move |title| {
            title_events
                .borrow_mut()
                .push((page_id, super::PageEvent::TitleChanged(Some(title))));
        });
        let load_events = events.clone();
        builder = builder.with_on_page_load_handler(move |event, url| {
            let mut events = load_events.borrow_mut();
            match event {
                wry::PageLoadEvent::Started => {
                    events.push((page_id, super::PageEvent::UrlChanged(Some(url))));
                    events.push((page_id, super::PageEvent::LoadingChanged(true)));
                }
                wry::PageLoadEvent::Finished => {
                    events.push((page_id, super::PageEvent::UrlChanged(Some(url))));
                    events.push((page_id, super::PageEvent::LoadingChanged(false)));
                }
            }
        });
        #[cfg(target_os = "linux")]
        use wry::WebViewBuilderExtUnix;

        #[cfg(target_os = "linux")]
        let inner = builder.build_gtk(&root.fixed).map_err(super::map_error)?;
        #[cfg(not(target_os = "linux"))]
        let inner = builder.build_as_child(_window).map_err(super::map_error)?;
        Ok(Self::from_inner(inner, "Page", events))
    }

    fn from_inner(
        inner: wry::WebView,
        surface_name: &'static str,
        events: super::PageEventQueue,
    ) -> Self {
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
            events,
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

    pub fn can_go_back(&self) -> Result<bool> {
        self.inner.can_go_back().map_err(super::map_error)
    }

    pub fn can_go_forward(&self) -> Result<bool> {
        self.inner.can_go_forward().map_err(super::map_error)
    }

    pub fn drain_events(&self, page_id: PageId) -> Vec<super::PageEvent> {
        let mut events = self.events.borrow_mut();
        let mut page_events = Vec::new();
        let mut remaining = Vec::with_capacity(events.len());
        for (id, event) in events.drain(..) {
            if id == page_id {
                page_events.push(event);
            } else {
                remaining.push((id, event));
            }
        }
        *events = remaining;
        page_events
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
    use super::DEBUG_OVERLAY_BOUNDS;

    #[test]
    fn debug_overlay_geometry_is_logical_and_valid() {
        assert!(DEBUG_OVERLAY_BOUNDS.validate().is_ok());
        assert_eq!(DEBUG_OVERLAY_BOUNDS.width, 220.0);
        assert_eq!(DEBUG_OVERLAY_BOUNDS.height, 80.0);
    }
}

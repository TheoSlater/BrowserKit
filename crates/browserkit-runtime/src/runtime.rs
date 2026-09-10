use std::collections::HashMap;

use browserkit_types::{
    BrowserOptions, Error, ErrorKind, LogicalRect, PageId, PageOptions, Result, WindowId,
    WindowOptions,
};
use browserkit_wry::{NativeRoot, PlatformBackend, WebView, WryBackend};
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

pub struct Runtime {
    event_loop: EventLoop<()>,
    windows: HashMap<WindowId, RuntimeWindow>,
    next_id: u64,
    backend: WryBackend,
}

struct RuntimeWindow {
    window: tao::window::Window,
    root: NativeRoot,
    pages: HashMap<PageId, RuntimePage>,
}

struct RuntimePage {
    webview: WebView,
}

impl Runtime {
    pub fn new(_options: BrowserOptions) -> Result<Self> {
        browserkit_wry::initialize()?;
        Ok(Self {
            event_loop: EventLoop::new(),
            windows: HashMap::new(),
            next_id: 1,
            backend: WryBackend,
        })
    }

    pub fn create_window(&mut self, options: WindowOptions) -> Result<WindowId> {
        let window = WindowBuilder::new()
            .with_title(options.title)
            .with_inner_size(tao::dpi::LogicalSize::new(options.width, options.height))
            .build(&self.event_loop)
            .map_err(|error| Error::new(ErrorKind::Window, error.to_string()))?;
        let id = WindowId::new(self.next_id());
        let root = NativeRoot::new(&window)?;
        self.windows.insert(
            id,
            RuntimeWindow {
                window,
                root,
                pages: HashMap::new(),
            },
        );
        Ok(id)
    }

    pub fn create_page(&mut self, window_id: WindowId, options: PageOptions) -> Result<PageId> {
        let page_id = PageId::new(self.next_id());
        let window = self
            .windows
            .get(&window_id)
            .ok_or_else(|| Error::new(ErrorKind::Window, "window not found"))?;
        let webview = self
            .backend
            .create_page(&window.window, &window.root, &options)?;
        self.windows
            .get_mut(&window_id)
            .ok_or_else(|| Error::new(ErrorKind::Window, "window not found"))?
            .pages
            .insert(page_id, RuntimePage { webview });
        Ok(page_id)
    }

    pub fn navigate(&self, window_id: WindowId, page_id: PageId, url: &str) -> Result<()> {
        self.page(window_id, page_id)?.webview.navigate(url)
    }
    pub fn set_bounds(
        &mut self,
        window_id: WindowId,
        page_id: PageId,
        bounds: LogicalRect,
    ) -> Result<()> {
        self.page_mut(window_id, page_id)?
            .webview
            .set_bounds(bounds)
    }
    pub fn set_visible(&self, window_id: WindowId, page_id: PageId, visible: bool) -> Result<()> {
        self.page(window_id, page_id)?.webview.set_visible(visible)
    }
    pub fn focus(&self, window_id: WindowId, page_id: PageId) -> Result<()> {
        self.page(window_id, page_id)?.webview.focus()
    }

    pub fn run(self) -> ! {
        let Runtime {
            event_loop,
            mut windows,
            ..
        } = self;
        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;
            match event {
                Event::WindowEvent {
                    window_id,
                    event: WindowEvent::Resized(size),
                    ..
                } => {
                    if let Some(window) = windows
                        .values_mut()
                        .find(|window| window.window.id() == window_id)
                    {
                        let logical = size.to_logical::<f64>(window.window.scale_factor());
                        let bounds = content_bounds(logical.width, logical.height);
                        for page in window.pages.values_mut() {
                            let _ = page.webview.set_bounds(bounds);
                        }
                    }
                }
                Event::WindowEvent {
                    window_id,
                    event: WindowEvent::CloseRequested,
                    ..
                } => {
                    windows.retain(|_, window| window.window.id() != window_id);
                    if windows.is_empty() {
                        *control_flow = ControlFlow::Exit;
                    }
                }
                _ => {}
            }
            browserkit_wry::pump_events();
        })
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
    fn page(&self, window_id: WindowId, page_id: PageId) -> Result<&RuntimePage> {
        self.windows
            .get(&window_id)
            .and_then(|window| window.pages.get(&page_id))
            .ok_or_else(|| Error::new(ErrorKind::WebView, "page not found"))
    }
    fn page_mut(&mut self, window_id: WindowId, page_id: PageId) -> Result<&mut RuntimePage> {
        self.windows
            .get_mut(&window_id)
            .and_then(|window| window.pages.get_mut(&page_id))
            .ok_or_else(|| Error::new(ErrorKind::WebView, "page not found"))
    }
}

fn content_bounds(width: f64, height: f64) -> LogicalRect {
    LogicalRect {
        x: 16.0,
        y: 56.0,
        width: (width - 32.0).max(0.0),
        height: (height - 72.0).max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::content_bounds;
    #[test]
    fn bounds_do_not_underflow_small_windows() {
        assert_eq!(content_bounds(8.0, 8.0).width, 0.0);
        assert_eq!(content_bounds(8.0, 8.0).height, 0.0);
    }

    #[test]
    fn bounds_preserve_fractional_resize() {
        assert_eq!(content_bounds(100.5, 100.5).width, 68.5);
        assert_eq!(content_bounds(100.5, 100.5).height, 28.5);
    }
}

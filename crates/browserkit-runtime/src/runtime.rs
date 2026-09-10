use std::collections::HashMap;

use browserkit_types::{
    BrowserOptions, Command, Error, ErrorKind, Event, FrontendMessage, LogicalRect, NativeMessage,
    PageId, PageOptions, PageState, ProtocolError, Request, ResponseData, Result, WindowId,
    WindowOptions, WindowState,
};
use browserkit_wry::{
    ChromeWebView, NativeRoot, PageEvent, PageEventQueue, PlatformBackend, WebView, WryBackend,
};
use tao::{
    dpi::PhysicalSize,
    event::{Event as TaoEvent, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use crate::ownership::{FocusOwner, InputCoordinator};

pub struct Runtime {
    event_loop: EventLoop<()>,
    windows: HashMap<WindowId, RuntimeWindow>,
    next_id: u64,
    next_surface_id: u64,
    backend: WryBackend,
    page_events: PageEventQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PageSurfaceId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ChromeSurfaceId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceLifecycle {
    Visible,
    Hidden,
    Destroyed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PageSurface {
    id: PageSurfaceId,
    page_id: PageId,
    bounds: LogicalRect,
    visible: bool,
    focused: bool,
    z_order: u64,
    lifecycle: SurfaceLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ChromeSurface {
    id: ChromeSurfaceId,
    bounds: LogicalRect,
    visible: bool,
    focused: bool,
}

impl PageSurface {
    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        self.focused = false;
        self.lifecycle = if visible {
            SurfaceLifecycle::Visible
        } else {
            SurfaceLifecycle::Hidden
        };
    }

    fn destroy(&mut self) {
        self.visible = false;
        self.focused = false;
        self.lifecycle = SurfaceLifecycle::Destroyed;
    }
}

struct RuntimeWindow {
    window: tao::window::Window,
    root: NativeRoot,
    debug_native_overlay: bool,
    scale_factor: f64,
    pages: HashMap<PageId, RuntimePage>,
    page_order: Vec<PageId>,
    active_page: Option<PageId>,
    chrome: Option<RuntimeChrome>,
    input: InputCoordinator,
}

struct RuntimePage {
    webview: WebView,
    surface: PageSurface,
    state: PageState,
}

struct RuntimeChrome {
    webview: ChromeWebView,
    surface: ChromeSurface,
}

impl Runtime {
    pub fn new(_options: BrowserOptions) -> Result<Self> {
        browserkit_wry::initialize()?;
        Ok(Self {
            event_loop: EventLoop::new(),
            windows: HashMap::new(),
            next_id: 1,
            next_surface_id: 1,
            backend: WryBackend,
            page_events: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
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
        let debug_native_overlay = options.debug_native_overlay;
        let scale_factor = window.scale_factor();
        self.windows.insert(
            id,
            RuntimeWindow {
                window,
                root,
                debug_native_overlay,
                scale_factor,
                pages: HashMap::new(),
                page_order: Vec::new(),
                active_page: None,
                chrome: None,
                input: InputCoordinator::default(),
            },
        );
        Ok(id)
    }

    pub fn create_page(&mut self, window_id: WindowId, options: PageOptions) -> Result<PageId> {
        let page_id = PageId::new(self.next_id());
        let surface_id = PageSurfaceId(self.next_surface_id);
        self.next_surface_id += 1;
        let page_events = self.page_events.clone();
        let backend = self.backend;
        let window = self
            .windows
            .get_mut(&window_id)
            .ok_or_else(|| Error::new(ErrorKind::Window, "window not found"))?;
        let page =
            create_page_surface(window, &backend, &options, page_id, surface_id, page_events)?;
        window.pages.insert(page_id, page);
        window.page_order.push(page_id);
        let initial_size = window.window.inner_size();
        apply_resize(window, initial_size);
        emit_event(
            window,
            Event::PageCreated {
                state: window.pages[&page_id].state.clone(),
            },
        );
        #[cfg(debug_assertions)]
        eprintln!("BrowserKit page_created: page={page_id:?} surface={surface_id:?}");
        Ok(page_id)
    }

    pub fn navigate(&self, window_id: WindowId, page_id: PageId, url: &str) -> Result<()> {
        self.page(window_id, page_id)?.webview.navigate(url)
    }
    pub fn go_back(&self, window_id: WindowId, page_id: PageId) -> Result<()> {
        self.page(window_id, page_id)?.webview.go_back()
    }
    pub fn go_forward(&self, window_id: WindowId, page_id: PageId) -> Result<()> {
        self.page(window_id, page_id)?.webview.go_forward()
    }
    pub fn reload(&self, window_id: WindowId, page_id: PageId) -> Result<()> {
        self.page(window_id, page_id)?.webview.reload()
    }
    pub fn set_bounds(
        &mut self,
        window_id: WindowId,
        page_id: PageId,
        bounds: LogicalRect,
    ) -> Result<()> {
        let page = self.page_mut(window_id, page_id)?;
        page.webview.set_bounds(bounds)?;
        page.surface.bounds = bounds;
        Ok(())
    }
    pub fn set_visible(
        &mut self,
        window_id: WindowId,
        page_id: PageId,
        visible: bool,
    ) -> Result<()> {
        let window = self.window_mut(window_id)?;
        let page = window
            .pages
            .get_mut(&page_id)
            .ok_or_else(|| Self::surface_error("page not found"))?;
        page.webview.set_visible(visible)?;
        page.webview
            .set_interactive(visible && window.active_page == Some(page_id))?;
        page.surface.set_visible(visible);
        if !visible && window.active_page == Some(page_id) {
            window.active_page = None;
            page.surface.focused = false;
            page.state.active = false;
        }
        if visible {
            window.input.focused_surface = FocusOwner::Page(page_id);
        }
        Ok(())
    }
    pub fn focus(&mut self, window_id: WindowId, page_id: PageId) -> Result<()> {
        let window = self.window_mut(window_id)?;
        if window.active_page != Some(page_id) {
            Self::activate_page_in_window(window, page_id)?;
        }
        let window = self.window_mut(window_id)?;
        for page in window.pages.values_mut() {
            page.surface.focused = false;
        }
        let page = window
            .pages
            .get_mut(&page_id)
            .ok_or_else(|| Self::surface_error("page not found"))?;
        page.webview.focus()?;
        page.surface.focused = true;
        page.state.active = true;
        window.input.focused_surface = FocusOwner::Page(page_id);
        #[cfg(debug_assertions)]
        eprintln!("BrowserKit focus_changed: page={page_id:?}");
        Ok(())
    }

    pub fn set_active_page(&mut self, window_id: WindowId, page_id: PageId) -> Result<()> {
        let window = self.window_mut(window_id)?;
        Self::activate_page_in_window(window, page_id)
    }

    pub fn close_page(&mut self, window_id: WindowId, page_id: PageId) -> Result<()> {
        let window = self.window_mut(window_id)?;
        let page = window
            .pages
            .remove(&page_id)
            .ok_or_else(|| Self::surface_error("page not found"))?;
        window.page_order.retain(|id| *id != page_id);
        let was_active = window.active_page == Some(page_id);
        let mut surface = page.surface;
        surface.destroy();
        emit_event(window, Event::PageClosed { page_id });
        #[cfg(debug_assertions)]
        eprintln!(
            "BrowserKit page_destroyed: page={page_id:?} surface={:?}",
            surface.id
        );
        if was_active {
            window.active_page = None;
            if let Some(next) = window.page_order.iter().copied().find(|id| {
                window
                    .pages
                    .get(id)
                    .is_some_and(|page| page.surface.lifecycle != SurfaceLifecycle::Destroyed)
            }) {
                Self::activate_page_in_window(window, next)?;
            }
        }
        if window.active_page.is_none() {
            window.input.focused_surface = FocusOwner::None;
        }
        Ok(())
    }

    pub fn run(self) -> ! {
        let Runtime {
            event_loop,
            mut windows,
            mut next_id,
            mut next_surface_id,
            backend,
            page_events,
        } = self;
        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;
            match event {
                TaoEvent::WindowEvent {
                    window_id,
                    event: WindowEvent::Resized(size),
                    ..
                } => {
                    if let Some(window) = windows
                        .values_mut()
                        .find(|window| window.window.id() == window_id)
                    {
                        apply_resize(window, size);
                    }
                }
                TaoEvent::WindowEvent {
                    window_id,
                    event:
                        WindowEvent::ScaleFactorChanged {
                            scale_factor,
                            new_inner_size,
                        },
                    ..
                } => {
                    if let Some(window) = windows
                        .values_mut()
                        .find(|window| window.window.id() == window_id)
                    {
                        window.scale_factor = scale_factor;
                        apply_resize(window, *new_inner_size);
                    }
                }
                TaoEvent::WindowEvent {
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
            process_page_events(&mut windows, &page_events);
            for window in windows.values_mut() {
                if let Some((width, height)) = window.root.allocated_size() {
                    reconcile_layout(window, width, height);
                }
            }
            let window_ids: Vec<_> = windows.keys().copied().collect();
            for window_id in window_ids {
                let _ = handle_chrome_messages(
                    &mut windows,
                    window_id,
                    &backend,
                    &mut next_id,
                    &mut next_surface_id,
                    page_events.clone(),
                );
            }
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

    fn window_mut(&mut self, window_id: WindowId) -> Result<&mut RuntimeWindow> {
        self.windows
            .get_mut(&window_id)
            .ok_or_else(|| Error::new(ErrorKind::Window, "window not found"))
    }

    fn surface_error(message: &str) -> Error {
        Error::new(ErrorKind::Surface, message)
    }

    fn activate_page_in_window(window: &mut RuntimeWindow, page_id: PageId) -> Result<()> {
        if !window.pages.contains_key(&page_id) {
            return Err(Self::surface_error("page not found"));
        }
        if window.active_page == Some(page_id) {
            let page = window.pages.get_mut(&page_id).unwrap();
            page.webview.set_visible(true)?;
            page.webview.set_interactive(true)?;
            page.surface.visible = true;
            page.surface.focused = true;
            page.state.active = true;
            page.webview.focus()?;
            window.input.focused_surface = FocusOwner::Page(page_id);
            return Ok(());
        }
        if let Some(old_id) = window.active_page {
            if let Some(old) = window.pages.get_mut(&old_id) {
                old.webview.set_visible(false)?;
                old.webview.set_interactive(false)?;
                old.surface.set_visible(false);
                old.state.active = false;
                #[cfg(debug_assertions)]
                eprintln!("BrowserKit page_hidden: page={old_id:?}");
            }
        }
        {
            let page = window.pages.get_mut(&page_id).unwrap();
            page.webview.set_visible(true)?;
            page.webview.set_interactive(true)?;
            page.webview.focus()?;
            page.surface.set_visible(true);
            page.surface.focused = true;
            page.state.active = true;
        }
        for other in window.pages.values_mut() {
            if other.surface.page_id != page_id {
                other.surface.z_order = 0;
            }
        }
        window.pages.get_mut(&page_id).unwrap().surface.z_order = 1;
        window.active_page = Some(page_id);
        window.input.focused_surface = FocusOwner::Page(page_id);
        if let Some(state) = window.pages.get(&page_id).map(|page| page.state.clone()) {
            emit_event(window, Event::PageActivated { state });
        }
        #[cfg(debug_assertions)]
        eprintln!("BrowserKit page_activated: page={page_id:?}");
        Ok(())
    }
}

fn create_page_surface(
    window: &mut RuntimeWindow,
    backend: &WryBackend,
    options: &PageOptions,
    page_id: PageId,
    surface_id: PageSurfaceId,
    events: PageEventQueue,
) -> Result<RuntimePage> {
    if options.host_mode == browserkit_types::WebViewHostMode::Composition
        && window.chrome.is_none()
    {
        let chrome = backend.create_chrome(&window.window, &mut window.root)?;
        window.chrome = Some(RuntimeChrome {
            webview: chrome,
            surface: ChromeSurface {
                id: ChromeSurfaceId(0),
                bounds: LogicalRect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
                visible: true,
                focused: false,
            },
        });
        #[cfg(debug_assertions)]
        eprintln!("BrowserKit chrome_created");
    }
    let webview = backend.create_page(
        &window.window,
        &mut window.root,
        options,
        window.debug_native_overlay,
        page_id,
        events,
    )?;
    let active = window.active_page.is_none();
    if !active {
        webview.set_visible(false)?;
        webview.set_interactive(false)?;
    } else {
        webview.set_interactive(true)?;
        window.active_page = Some(page_id);
    }
    let state = PageState {
        id: page_id,
        url: options.url.clone(),
        title: None,
        loading: options.url.is_some(),
        can_go_back: false,
        can_go_forward: false,
        active,
    };
    let surface = PageSurface {
        id: surface_id,
        page_id,
        bounds: LogicalRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        },
        visible: active,
        focused: active,
        z_order: if active { 1 } else { 0 },
        lifecycle: if active {
            SurfaceLifecycle::Visible
        } else {
            SurfaceLifecycle::Hidden
        },
    };
    Ok(RuntimePage {
        webview,
        surface,
        state,
    })
}

fn handle_chrome_messages(
    windows: &mut HashMap<WindowId, RuntimeWindow>,
    window_id: WindowId,
    backend: &WryBackend,
    next_id: &mut u64,
    next_surface_id: &mut u64,
    page_events: PageEventQueue,
) -> Result<()> {
    let messages = {
        let window = windows
            .get_mut(&window_id)
            .ok_or_else(|| Error::new(ErrorKind::Window, "window not found"))?;
        let chrome = window
            .chrome
            .as_ref()
            .ok_or_else(|| Error::new(ErrorKind::Surface, "chrome surface is not available"))?;
        window.input.focused_surface = FocusOwner::Chrome;
        chrome.webview.drain_messages()
    };
    for raw in messages {
        match crate::ipc::parse_message(&raw) {
            Ok(message) => {
                #[cfg(debug_assertions)]
                eprintln!("BrowserKit ipc_parsed");
                if let Err(error) = dispatch_message(
                    windows,
                    window_id,
                    message,
                    backend,
                    next_id,
                    next_surface_id,
                    page_events.clone(),
                ) {
                    #[cfg(debug_assertions)]
                    eprintln!("BrowserKit ipc_error: {}", error.code);
                }
            }
            Err(error) => {
                #[cfg(debug_assertions)]
                eprintln!("BrowserKit ipc_rejected: {}", error.code);
            }
        }
    }
    Ok(())
}

fn dispatch_message(
    windows: &mut HashMap<WindowId, RuntimeWindow>,
    window_id: WindowId,
    message: FrontendMessage,
    backend: &WryBackend,
    next_id: &mut u64,
    next_surface_id: &mut u64,
    page_events: PageEventQueue,
) -> std::result::Result<(), ProtocolError> {
    match message {
        FrontendMessage::Command { command } => execute_command(windows, window_id, command),
        FrontendMessage::Request { id, request } => {
            let result = execute_request(
                windows,
                window_id,
                request,
                backend,
                next_id,
                next_surface_id,
                page_events,
            );
            let response = match result {
                Ok(data) => NativeMessage::Response {
                    id,
                    ok: true,
                    data,
                    error: None,
                },
                Err(error) => NativeMessage::Response {
                    id,
                    ok: false,
                    data: None,
                    error: Some(error),
                },
            };
            send_to_chrome(windows, window_id, &response).map_err(protocol_from_error)
        }
    }
}

fn execute_command(
    windows: &mut HashMap<WindowId, RuntimeWindow>,
    window_id: WindowId,
    command: Command,
) -> std::result::Result<(), ProtocolError> {
    match command {
        Command::PageNavigate { page_id, url } => {
            let page_id = resolve_page(windows, window_id, page_id)?;
            validate_url(&url)?;
            windows[&window_id].pages[&page_id]
                .webview
                .navigate(&url)
                .map_err(protocol_from_error)
        }
        Command::PageReload { page_id } => {
            let page_id = resolve_page(windows, window_id, page_id)?;
            windows[&window_id].pages[&page_id]
                .webview
                .reload()
                .map_err(protocol_from_error)
        }
        Command::PageGoBack { page_id } => {
            let page_id = resolve_page(windows, window_id, page_id)?;
            windows[&window_id].pages[&page_id]
                .webview
                .go_back()
                .map_err(protocol_from_error)
        }
        Command::PageGoForward { page_id } => {
            let page_id = resolve_page(windows, window_id, page_id)?;
            windows[&window_id].pages[&page_id]
                .webview
                .go_forward()
                .map_err(protocol_from_error)
        }
        Command::PageActivate { page_id } => {
            let window = windows
                .get_mut(&window_id)
                .ok_or_else(|| error_for_protocol("window_not_found", "window not found"))?;
            Runtime::activate_page_in_window(window, page_id).map_err(protocol_from_error)
        }
        Command::PageClose { page_id } => {
            close_page_in_window(windows, window_id, page_id).map_err(protocol_from_error)
        }
    }
}

fn execute_request(
    windows: &mut HashMap<WindowId, RuntimeWindow>,
    window_id: WindowId,
    request: Request,
    backend: &WryBackend,
    next_id: &mut u64,
    next_surface_id: &mut u64,
    page_events: PageEventQueue,
) -> std::result::Result<Option<ResponseData>, ProtocolError> {
    match request {
        Request::PageCreate { url } => {
            if let Some(url) = &url {
                validate_url(url)?;
            }
            let page_id = PageId::new(*next_id);
            *next_id += 1;
            let surface_id = PageSurfaceId(*next_surface_id);
            *next_surface_id += 1;
            let window = windows
                .get_mut(&window_id)
                .ok_or_else(|| error_for_protocol("window_not_found", "window not found"))?;
            let page = create_page_surface(
                window,
                backend,
                &PageOptions {
                    url,
                    host_mode: browserkit_types::WebViewHostMode::Composition,
                },
                page_id,
                surface_id,
                page_events,
            )
            .map_err(protocol_from_error)?;
            let state = page.state.clone();
            window.pages.insert(page_id, page);
            window.page_order.push(page_id);
            let initial_size = window.window.inner_size();
            apply_resize(window, initial_size);
            emit_event(window, Event::PageCreated { state });
            Ok(Some(ResponseData::PageCreated { page_id }))
        }
        Request::PageGetState { page_id } => {
            let page_id = resolve_page(windows, window_id, page_id)?;
            let state = windows[&window_id].pages[&page_id].state.clone();
            Ok(Some(ResponseData::PageState { state }))
        }
        Request::WindowGetState => {
            let window = windows
                .get(&window_id)
                .ok_or_else(|| error_for_protocol("window_not_found", "window not found"))?;
            Ok(Some(ResponseData::WindowState {
                state: window_state(window),
            }))
        }
    }
}

fn resolve_page(
    windows: &HashMap<WindowId, RuntimeWindow>,
    window_id: WindowId,
    page_id: Option<PageId>,
) -> std::result::Result<PageId, ProtocolError> {
    let window = windows
        .get(&window_id)
        .ok_or_else(|| error_for_protocol("window_not_found", "window not found"))?;
    let page_id = page_id
        .or(window.active_page)
        .ok_or_else(|| error_for_protocol("no_active_page", "no active page"))?;
    if window.pages.contains_key(&page_id) {
        Ok(page_id)
    } else {
        Err(error_for_protocol("page_not_found", "page not found"))
    }
}

fn validate_url(url: &str) -> std::result::Result<(), ProtocolError> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(error_for_protocol(
            "invalid_url",
            "URL must use http or https",
        ))
    }
}

fn error_for_protocol(code: &str, message: &str) -> ProtocolError {
    ProtocolError {
        code: code.into(),
        message: message.into(),
    }
}

fn protocol_from_error(error: Error) -> ProtocolError {
    ProtocolError {
        code: "request_failed".into(),
        message: error.to_string(),
    }
}

fn window_state(window: &RuntimeWindow) -> WindowState {
    WindowState {
        active_page_id: window.active_page,
        pages: window
            .page_order
            .iter()
            .filter_map(|page_id| window.pages.get(page_id).map(|page| page.state.clone()))
            .collect(),
    }
}

fn send_to_chrome(
    windows: &mut HashMap<WindowId, RuntimeWindow>,
    window_id: WindowId,
    message: &NativeMessage,
) -> Result<()> {
    if let Some(chrome) = windows
        .get(&window_id)
        .and_then(|window| window.chrome.as_ref())
    {
        chrome.webview.send_message(message)?;
        #[cfg(debug_assertions)]
        eprintln!("BrowserKit chrome_message_sent");
    }
    Ok(())
}

fn emit_event(window: &mut RuntimeWindow, event: Event) {
    #[cfg(debug_assertions)]
    eprintln!("BrowserKit event_emitted");
    let message = NativeMessage::Event { event };
    if let Some(chrome) = window.chrome.as_ref() {
        let _ = chrome.webview.send_message(&message);
    }
}

fn close_page_in_window(
    windows: &mut HashMap<WindowId, RuntimeWindow>,
    window_id: WindowId,
    page_id: PageId,
) -> Result<()> {
    let window = windows
        .get_mut(&window_id)
        .ok_or_else(|| Error::new(ErrorKind::Window, "window not found"))?;
    let page = window
        .pages
        .remove(&page_id)
        .ok_or_else(|| Error::new(ErrorKind::Surface, "page not found"))?;
    window.page_order.retain(|id| *id != page_id);
    let was_active = window.active_page == Some(page_id);
    emit_event(window, Event::PageClosed { page_id });
    if was_active {
        window.active_page = None;
        if let Some(next) = window.page_order.first().copied() {
            Runtime::activate_page_in_window(window, next)?;
        }
    }
    if window.active_page.is_none() {
        window.input.focused_surface = FocusOwner::None;
    }
    drop(page);
    Ok(())
}

fn process_page_events(windows: &mut HashMap<WindowId, RuntimeWindow>, events: &PageEventQueue) {
    let events: Vec<_> = events.borrow_mut().drain(..).collect();
    for (page_id, event) in events {
        let Some(window_id) = windows.iter().find_map(|(window_id, window)| {
            window.pages.contains_key(&page_id).then_some(*window_id)
        }) else {
            continue;
        };
        let mut emitted = Vec::new();
        if let Some(window) = windows.get_mut(&window_id) {
            if let Some(page) = window.pages.get_mut(&page_id) {
                match event {
                    PageEvent::UrlChanged(url) => {
                        page.state.url = url.clone();
                        emitted.push(Event::PageUrlChanged { page_id, url });
                    }
                    PageEvent::TitleChanged(title) => {
                        page.state.title = title.clone();
                        emitted.push(Event::PageTitleChanged { page_id, title });
                    }
                    PageEvent::LoadingChanged(loading) => {
                        page.state.loading = loading;
                        emitted.push(Event::PageLoadingChanged { page_id, loading });
                    }
                }
                let can_go_back = page.webview.can_go_back().unwrap_or(false);
                let can_go_forward = page.webview.can_go_forward().unwrap_or(false);
                if page.state.can_go_back != can_go_back
                    || page.state.can_go_forward != can_go_forward
                {
                    page.state.can_go_back = can_go_back;
                    page.state.can_go_forward = can_go_forward;
                    emitted.push(Event::PageNavigationStateChanged {
                        page_id,
                        can_go_back,
                        can_go_forward,
                    });
                }
            }
            for event in emitted {
                emit_event(window, event);
            }
        }
    }
}

fn apply_resize(window: &mut RuntimeWindow, physical_size: PhysicalSize<u32>) {
    let logical_size = physical_size.to_logical::<f64>(window.scale_factor);
    #[cfg(debug_assertions)]
    eprintln!(
        "BrowserKit resize: physical={}x{} scale={} logical={}x{}",
        physical_size.width,
        physical_size.height,
        window.scale_factor,
        logical_size.width,
        logical_size.height,
    );
    reconcile_layout(window, logical_size.width, logical_size.height);
}

fn reconcile_layout(window: &mut RuntimeWindow, width: f64, height: f64) {
    let bounds = content_bounds(width, height);
    let chrome_bounds = chrome_bounds(width);
    #[cfg(debug_assertions)]
    if bounds.x + bounds.width > width || bounds.y + bounds.height > height {
        eprintln!("BrowserKit warning: page bounds exceed GTK logical root bounds");
    }
    for page in window.pages.values_mut() {
        if page.surface.bounds != bounds {
            let _ = page.webview.set_bounds(bounds);
            page.surface.bounds = bounds;
        }
    }
    if let Some(chrome) = &mut window.chrome {
        if chrome.surface.bounds != chrome_bounds {
            match chrome.webview.set_bounds(chrome_bounds) {
                Ok(_) => {
                    chrome.surface.bounds = chrome_bounds;
                    #[cfg(debug_assertions)]
                    eprintln!("BrowserKit chrome_bounds_applied: {chrome_bounds:?}");
                }
                Err(error) => {
                    #[cfg(debug_assertions)]
                    eprintln!("BrowserKit chrome_bounds_failed: {error}");
                }
            }
        } else {
            #[cfg(debug_assertions)]
            eprintln!("BrowserKit chrome_bounds_deduplicated");
        }
    }
    #[cfg(target_os = "linux")]
    if window.debug_native_overlay {
        let _ = window.root.set_overlay_bounds(LogicalRect {
            x: 760.0,
            y: 120.0,
            width: 220.0,
            height: 80.0,
        });
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

fn chrome_bounds(width: f64) -> LogicalRect {
    LogicalRect {
        x: 0.0,
        y: 0.0,
        width: width.max(0.0),
        height: 56.0,
    }
}

#[cfg(test)]
fn content_bounds_from_physical(width: u32, height: u32, scale_factor: f64) -> LogicalRect {
    let logical = PhysicalSize::new(width, height).to_logical::<f64>(scale_factor);
    content_bounds(logical.width, logical.height)
}

#[cfg(test)]
mod tests {
    use super::{
        chrome_bounds, content_bounds, content_bounds_from_physical, ChromeSurface,
        ChromeSurfaceId, PageSurface, PageSurfaceId, SurfaceLifecycle,
    };
    use browserkit_types::LogicalRect;
    #[test]
    fn bounds_do_not_underflow_small_windows() {
        assert_eq!(content_bounds(8.0, 8.0).width, 0.0);
        assert_eq!(content_bounds(8.0, 8.0).height, 0.0);
    }

    #[test]
    fn chrome_bounds_use_window_logical_width() {
        assert_eq!(
            chrome_bounds(1200.0),
            LogicalRect {
                x: 0.0,
                y: 0.0,
                width: 1200.0,
                height: 56.0,
            }
        );
        assert_eq!(chrome_bounds(-1.0).width, 0.0);
    }

    #[test]
    fn chrome_surface_is_window_owned_and_focusable() {
        let surface = ChromeSurface {
            id: ChromeSurfaceId(1),
            bounds: chrome_bounds(1200.0),
            visible: true,
            focused: false,
        };
        assert_eq!(surface.id, ChromeSurfaceId(1));
        assert!(surface.visible);
        assert!(!surface.focused);
    }

    #[test]
    fn bounds_preserve_fractional_resize() {
        assert_eq!(content_bounds(100.5, 100.5).width, 68.5);
        assert_eq!(content_bounds(100.5, 100.5).height, 28.5);
    }

    #[test]
    fn physical_resize_converts_once_at_each_scale() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            assert_eq!(
                content_bounds_from_physical(
                    (1200.0 * scale) as u32,
                    (800.0 * scale) as u32,
                    scale
                ),
                LogicalRect {
                    x: 16.0,
                    y: 56.0,
                    width: 1168.0,
                    height: 728.0,
                }
            );
        }
    }

    #[test]
    fn repeated_physical_resize_does_not_grow_geometry() {
        let first = content_bounds_from_physical(2400, 1600, 2.0);
        for _ in 0..100 {
            assert_eq!(content_bounds_from_physical(2400, 1600, 2.0), first);
        }
    }

    #[test]
    fn surface_lifecycle_hides_and_destroys_without_recreation() {
        let mut surface = PageSurface {
            id: PageSurfaceId(1),
            page_id: browserkit_types::PageId::new(1),
            bounds: content_bounds(1200.0, 800.0),
            visible: true,
            focused: true,
            z_order: 1,
            lifecycle: SurfaceLifecycle::Visible,
        };
        surface.set_visible(false);
        assert_eq!(surface.lifecycle, SurfaceLifecycle::Hidden);
        assert!(!surface.visible);
        surface.set_visible(true);
        assert_eq!(surface.lifecycle, SurfaceLifecycle::Visible);
        surface.destroy();
        assert_eq!(surface.lifecycle, SurfaceLifecycle::Destroyed);
        assert!(!surface.visible);
        assert_eq!(surface.id, PageSurfaceId(1));
    }
}

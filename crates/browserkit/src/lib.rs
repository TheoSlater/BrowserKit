mod error;

pub use browserkit_types::protocol::{
    Command, CommandEnvelope, Event, EventEnvelope, FrontendMessage, NativeMessage, ProtocolError,
    ProtocolErrorCode, Request, RequestEnvelope, RequestId, ResponseData, ResponseEnvelope,
    ResponseResult, PROTOCOL_VERSION,
};
pub use browserkit_types::{
    BrowserEvent, CoordinateSpace, LogicalRect, PageId, PageOptions, PageState, PageViewBounds,
    WindowId, WindowState,
};
pub use error::BrowserKitError;
pub type Result<T> = std::result::Result<T, BrowserKitError>;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

static NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_PAGE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_SUBSCRIPTION_ID: AtomicU64 = AtomicU64::new(1);
type SubscriberList = Arc<Mutex<Vec<(u64, Sender<BrowserEvent>)>>>;

#[derive(Clone)]
struct RuntimeControl {
    running: Arc<std::sync::atomic::AtomicBool>,
    commands: browserkit_runtime::CommandQueue,
    subscribers: SubscriberList,
}
impl RuntimeControl {
    fn emit(&self, event: BrowserEvent) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.retain(|(_, subscriber)| subscriber.send(event.clone()).is_ok());
        }
    }
}

pub struct BrowserKitOptions {
    pub title: String,
    pub initial_url: String,
    pub width: u32,
    pub height: u32,
}
impl Default for BrowserKitOptions {
    fn default() -> Self {
        Self {
            title: "BrowserKit".into(),
            initial_url: "about:blank".into(),
            width: 1200,
            height: 800,
        }
    }
}

pub struct BrowserKit {
    options: BrowserKitOptions,
    window: Arc<Mutex<Option<Arc<Mutex<WindowState>>>>>,
    control: RuntimeControl,
}
impl BrowserKit {
    pub fn new() -> Self {
        Self::builder().build().expect("default options are valid")
    }
    pub fn builder() -> BrowserKitBuilder {
        BrowserKitBuilder {
            options: BrowserKitOptions::default(),
        }
    }
    pub fn create_window(&self) -> Result<Window> {
        let mut slot = self.window.lock().expect("BrowserKit state lock poisoned");
        if slot.is_some() {
            return Err(BrowserKitError::WindowAlreadyExists);
        }
        let state = Arc::new(Mutex::new(WindowState {
            id: WindowId::from_raw(NEXT_WINDOW_ID.fetch_add(1, Ordering::Relaxed)),
            active_page_id: None,
            pages: Vec::new(),
        }));
        *slot = Some(state.clone());
        Ok(Window {
            state,
            control: self.control.clone(),
        })
    }
    pub fn run(&self) -> Result<()> {
        let state = self
            .window
            .lock()
            .expect("BrowserKit state lock poisoned")
            .clone()
            .ok_or(BrowserKitError::WindowNotFound)?;
        let window = state.lock().expect("window state lock poisoned").clone();
        if self.options.width == 0 || self.options.height == 0 {
            return Err(BrowserKitError::InvalidConfiguration(
                "window dimensions must be non-zero",
            ));
        }
        if self.options.title.trim().is_empty() {
            return Err(BrowserKitError::InvalidConfiguration(
                "window title cannot be empty",
            ));
        }
        self.control.running.store(true, Ordering::Release);
        let result = browserkit_runtime::BrowserKitRuntime::run(
            browserkit_runtime::BrowserKitRuntimeOptions {
                title: self.options.title.clone(),
                width: self.options.width,
                height: self.options.height,
                window_id: window.id,
                pages: window.pages,
                state: state.clone(),
                commands: self.control.commands.clone(),
                protocol_handler: {
                    let router = self.protocol_router();
                    Arc::new(move |message| router.handle_message(message))
                },
                emit: {
                    let subscribers = self.control.subscribers.clone();
                    Arc::new(move |event| {
                        if let Ok(mut subscribers) = subscribers.lock() {
                            subscribers
                                .retain(|(_, subscriber)| subscriber.send(event.clone()).is_ok());
                        }
                    })
                },
            },
        );
        self.control.running.store(false, Ordering::Release);
        result.map_err(Into::into)
    }
}
impl Default for BrowserKit {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Subscription {
    id: u64,
    receiver: Receiver<BrowserEvent>,
    subscribers: SubscriberList,
}
impl Subscription {
    pub fn try_recv(&self) -> std::result::Result<BrowserEvent, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
    pub fn recv(&self) -> std::result::Result<BrowserEvent, mpsc::RecvError> {
        self.receiver.recv()
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.retain(|(id, _)| *id != self.id);
        }
    }
}

impl BrowserKit {
    pub fn subscribe(&self) -> Subscription {
        let (sender, receiver) = mpsc::channel();
        let id = NEXT_SUBSCRIPTION_ID.fetch_add(1, Ordering::Relaxed);
        self.control
            .subscribers
            .lock()
            .expect("subscriber lock poisoned")
            .push((id, sender));
        Subscription {
            id,
            receiver,
            subscribers: self.control.subscribers.clone(),
        }
    }

    pub fn protocol_router(&self) -> ProtocolRouter {
        ProtocolRouter {
            window: self.window.clone(),
            control: self.control.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ProtocolRouter {
    window: Arc<Mutex<Option<Arc<Mutex<WindowState>>>>>,
    control: RuntimeControl,
}
impl ProtocolRouter {
    pub fn encode_event(&self, event: BrowserEvent) -> Option<String> {
        serde_json::to_string(&NativeMessage::Event(EventEnvelope {
            event: event.into(),
        }))
        .ok()
    }

    pub fn handle_message(&self, input: &str) -> Option<String> {
        let message = match serde_json::from_str::<FrontendMessage>(input) {
            Ok(message) => message,
            Err(error) => {
                let code = if error.to_string().contains("unknown variant") {
                    ProtocolErrorCode::UnsupportedMessage
                } else {
                    ProtocolErrorCode::InvalidMessage
                };
                return Some(encode_error(
                    recover_request_id(input),
                    code,
                    error.to_string(),
                ));
            }
        };
        let response = match message {
            FrontendMessage::Command(envelope) => {
                if let Err(error) = self.handle_command(envelope.command) {
                    eprintln!("BrowserKit protocol command failed: {error}");
                }
                return None;
            }
            FrontendMessage::Request(envelope) => self.handle_request(envelope),
        };
        serde_json::to_string(&NativeMessage::Response(response)).ok()
    }

    fn handle_command(&self, command: Command) -> Result<()> {
        match command {
            Command::PageNavigate { page_id, url } => self.page(page_id)?.navigate(url),
            Command::PageReload { page_id } => self.page(page_id)?.reload(),
            Command::PageGoBack { page_id } => self.page(page_id)?.go_back(),
            Command::PageGoForward { page_id } => self.page(page_id)?.go_forward(),
            Command::PageStop { page_id } => self.page(page_id)?.stop(),
            Command::PageActivate { window_id, page_id } => {
                self.window(window_id)?.set_active_page(page_id)
            }
            Command::PageClose { window_id, page_id } => {
                self.window(window_id)?.close_page(page_id)
            }
            Command::PageRegisterView { page_id } => self.page(page_id)?.register_view(),
            Command::PageSetViewBounds {
                page_id,
                rect,
                coordinate_space,
                device_pixel_ratio,
                visual_viewport_scale,
            } => self.page(page_id)?.set_view_bounds(PageViewBounds {
                rect,
                coordinate_space,
                device_pixel_ratio,
                visual_viewport_scale,
            }),
            Command::PageUnregisterView { page_id } => self.page(page_id)?.unregister_view(),
        }
    }

    fn handle_request(&self, envelope: RequestEnvelope) -> ResponseEnvelope {
        let id = envelope.id;
        let result = match envelope.request {
            Request::RuntimeHandshake { protocol_version } => {
                if protocol_version != PROTOCOL_VERSION {
                    Err((
                        ProtocolErrorCode::ProtocolVersionMismatch,
                        format!("supported protocol version is {PROTOCOL_VERSION}"),
                    ))
                } else {
                    Ok(ResponseData::RuntimeHandshake {
                        protocol_version: PROTOCOL_VERSION,
                        windows: self
                            .window_state()
                            .map(|state| vec![state])
                            .unwrap_or_default(),
                    })
                }
            }
            Request::PageCreate { window_id, options } => self
                .window(window_id)
                .and_then(|window| window.create_page(options))
                .map(|page| ResponseData::PageCreated {
                    page: page.state().unwrap_or_else(|_| PageState {
                        id: page.id(),
                        url: String::new(),
                        title: String::new(),
                        loading: false,
                        can_go_back: false,
                        can_go_forward: false,
                        active: false,
                    }),
                })
                .map_err(protocol_error_from_browserkit),
            Request::PageGetState { page_id } => self
                .page(page_id)
                .and_then(|page| page.state())
                .map(|page| ResponseData::PageState { page })
                .map_err(protocol_error_from_browserkit),
            Request::WindowGetState { window_id } => self
                .window(window_id)
                .and_then(|window| window.state())
                .map(|window| ResponseData::WindowState { window })
                .map_err(protocol_error_from_browserkit),
        };
        ResponseEnvelope {
            id,
            result: result.map_or_else(
                |error| ResponseResult::Err {
                    error: protocol_error(error.0, error.1),
                },
                |data| ResponseResult::Ok { data },
            ),
        }
    }

    fn window(&self, id: WindowId) -> Result<Window> {
        let window = self
            .window
            .lock()
            .expect("BrowserKit state lock poisoned")
            .clone()
            .map(|state| Window {
                state,
                control: self.control.clone(),
            });
        window
            .filter(|window| window.id() == id)
            .ok_or(BrowserKitError::WindowNotFound)
    }
    fn page(&self, id: PageId) -> Result<Page> {
        let state = self.window_state()?;
        self.window(state.id)?
            .pages()
            .into_iter()
            .find(|page| page.id() == id)
            .ok_or(BrowserKitError::PageNotFound(id))
    }
    fn window_state(&self) -> Result<WindowState> {
        self.window
            .lock()
            .expect("BrowserKit state lock poisoned")
            .clone()
            .map(|state| state.lock().expect("window state lock poisoned").clone())
            .ok_or(BrowserKitError::WindowNotFound)
    }
}

fn protocol_error(code: ProtocolErrorCode, message: String) -> ProtocolError {
    ProtocolError { code, message }
}
fn protocol_error_from_browserkit(error: BrowserKitError) -> (ProtocolErrorCode, String) {
    let code = match error {
        BrowserKitError::WindowNotFound => ProtocolErrorCode::WindowNotFound,
        BrowserKitError::PageNotFound(_) => ProtocolErrorCode::PageNotFound,
        BrowserKitError::InvalidUrl(_) => ProtocolErrorCode::InvalidUrl,
        BrowserKitError::InvalidViewBounds(_) => ProtocolErrorCode::InvalidViewBounds,
        BrowserKitError::RuntimeNotReady => ProtocolErrorCode::RuntimeNotReady,
        _ => ProtocolErrorCode::RequestFailed,
    };
    (code, error.to_string())
}
fn encode_error(id: Option<RequestId>, code: ProtocolErrorCode, message: String) -> String {
    id.map(|id| {
        serde_json::to_string(&NativeMessage::Response(ResponseEnvelope {
            id,
            result: ResponseResult::Err {
                error: protocol_error(code, message),
            },
        }))
        .unwrap_or_default()
    })
    .unwrap_or_default()
}
fn recover_request_id(input: &str) -> Option<RequestId> {
    serde_json::from_str::<serde_json::Value>(input)
        .ok()?
        .get("id")
        .and_then(|id| id.as_str())
        .map(|id| RequestId(id.into()))
}

pub struct BrowserKitBuilder {
    options: BrowserKitOptions,
}
impl BrowserKitBuilder {
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.options.title = title.into();
        self
    }
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.options.width = width;
        self.options.height = height;
        self
    }
    pub fn initial_url(mut self, url: impl Into<String>) -> Self {
        self.options.initial_url = url.into();
        self
    }
    pub fn build(self) -> Result<BrowserKit> {
        Ok(BrowserKit {
            options: self.options,
            window: Arc::new(Mutex::new(None)),
            control: RuntimeControl {
                running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                commands: browserkit_runtime::CommandQueue::new(),
                subscribers: Arc::new(Mutex::new(Vec::new())),
            },
        })
    }
    pub fn run(self) -> Result<()> {
        self.build()?.run()
    }
}

#[derive(Clone)]
pub struct Window {
    state: Arc<Mutex<WindowState>>,
    control: RuntimeControl,
}
impl Window {
    pub fn id(&self) -> WindowId {
        self.state.lock().expect("window state lock poisoned").id
    }
    pub fn create_page(&self, options: PageOptions) -> Result<Page> {
        validate_url(&options.url)?;
        let mut state = self.state.lock().expect("window state lock poisoned");
        let id = PageId::from_raw(NEXT_PAGE_ID.fetch_add(1, Ordering::Relaxed));
        let active = state.active_page_id.is_none();
        state.pages.push(PageState {
            id,
            url: options.url,
            title: String::new(),
            loading: false,
            can_go_back: false,
            can_go_forward: false,
            active,
        });
        if active {
            state.active_page_id = Some(id);
        }
        let page = state.pages.last().cloned().expect("page was just inserted");
        if self.control.running.load(Ordering::Acquire) {
            browserkit_runtime::create_page(&self.control.commands, state.id, page)?;
        }
        println!(
            "BrowserKit: page_create_requested window={:?} page={:?}",
            state.id, id
        );
        Ok(Page {
            id,
            state: self.state.clone(),
            control: self.control.clone(),
        })
    }
    pub fn set_active_page(&self, id: PageId) -> Result<()> {
        let mut state = self.state.lock().expect("window state lock poisoned");
        if !state.pages.iter().any(|page| page.id == id) {
            return Err(BrowserKitError::PageNotFound(id));
        }
        let window_id = state.id;
        state.active_page_id = Some(id);
        for page in &mut state.pages {
            if page.active && page.id != id {
                println!(
                    "BrowserKit: page_hidden window={:?} page={:?}",
                    window_id, page.id
                );
            }
            page.active = page.id == id;
        }
        println!(
            "BrowserKit: page_activated window={:?} page={:?}",
            window_id, id
        );
        if self.control.running.load(Ordering::Acquire) {
            browserkit_runtime::activate_page(&self.control.commands, window_id, id)?;
            self.control.emit(BrowserEvent::PageActivated {
                window_id,
                page_id: id,
            });
        }
        Ok(())
    }
    pub fn close_page(&self, id: PageId) -> Result<()> {
        let mut state = self.state.lock().expect("window state lock poisoned");
        let index = state
            .pages
            .iter()
            .position(|page| page.id == id)
            .ok_or(BrowserKitError::PageNotFound(id))?;
        let was_active = state.active_page_id == Some(id);
        println!(
            "BrowserKit: page_close_requested window={:?} page={:?}",
            state.id, id
        );
        if self.control.running.load(Ordering::Acquire) {
            browserkit_runtime::close_page(&self.control.commands, state.id, id)?;
            return Ok(());
        }
        state.pages.remove(index);
        if was_active {
            state.active_page_id = state
                .pages
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|i| state.pages.get(i)))
                .map(|page| page.id);
            let active_page_id = state.active_page_id;
            for page in &mut state.pages {
                page.active = active_page_id == Some(page.id);
            }
        }
        Ok(())
    }
    pub fn active_page(&self) -> Option<Page> {
        let state = self.state.lock().expect("window state lock poisoned");
        state.active_page_id.map(|id| Page {
            id,
            state: self.state.clone(),
            control: self.control.clone(),
        })
    }
    pub fn pages(&self) -> Vec<Page> {
        let state = self.state.lock().expect("window state lock poisoned");
        state
            .pages
            .iter()
            .map(|page| Page {
                id: page.id,
                state: self.state.clone(),
                control: self.control.clone(),
            })
            .collect()
    }
    pub fn state(&self) -> Result<WindowState> {
        Ok(self
            .state
            .lock()
            .expect("window state lock poisoned")
            .clone())
    }
}

#[derive(Clone)]
pub struct Page {
    id: PageId,
    state: Arc<Mutex<WindowState>>,
    control: RuntimeControl,
}
impl Page {
    pub fn id(&self) -> PageId {
        self.id
    }
    pub fn url(&self) -> String {
        self.state
            .lock()
            .expect("window state lock poisoned")
            .pages
            .iter()
            .find(|page| page.id == self.id)
            .map(|page| page.url.clone())
            .unwrap_or_default()
    }
    pub fn is_active(&self) -> bool {
        self.state
            .lock()
            .expect("window state lock poisoned")
            .active_page_id
            == Some(self.id)
    }

    pub fn state(&self) -> Result<PageState> {
        self.state
            .lock()
            .expect("window state lock poisoned")
            .pages
            .iter()
            .find(|page| page.id == self.id)
            .cloned()
            .ok_or(BrowserKitError::PageNotFound(self.id))
    }

    pub fn navigate(&self, url: impl Into<String>) -> Result<()> {
        let url = url.into();
        validate_url(&url)?;
        self.ensure_running()?;
        browserkit_runtime::navigate(&self.control.commands, self.id, url).map_err(Into::into)
    }

    pub fn reload(&self) -> Result<()> {
        self.command(browserkit_runtime::PageCommand::Reload { page_id: self.id })
    }

    pub fn go_back(&self) -> Result<()> {
        self.command(browserkit_runtime::PageCommand::GoBack { page_id: self.id })
    }

    pub fn go_forward(&self) -> Result<()> {
        self.command(browserkit_runtime::PageCommand::GoForward { page_id: self.id })
    }

    pub fn stop(&self) -> Result<()> {
        self.command(browserkit_runtime::PageCommand::Stop { page_id: self.id })
    }

    pub fn register_view(&self) -> Result<()> {
        self.command(browserkit_runtime::PageCommand::RegisterView { page_id: self.id })
    }

    pub fn set_view_bounds(&self, bounds: PageViewBounds) -> Result<()> {
        bounds
            .validate()
            .map_err(|error| BrowserKitError::InvalidViewBounds(error.into()))?;
        self.command(browserkit_runtime::PageCommand::SetViewBounds {
            page_id: self.id,
            bounds,
        })
    }

    pub fn unregister_view(&self) -> Result<()> {
        self.command(browserkit_runtime::PageCommand::UnregisterView { page_id: self.id })
    }

    fn ensure_running(&self) -> Result<()> {
        self.control
            .running
            .load(Ordering::Acquire)
            .then_some(())
            .ok_or(BrowserKitError::RuntimeNotReady)
    }

    fn command(&self, command: browserkit_runtime::PageCommand) -> Result<()> {
        self.ensure_running()?;
        browserkit_runtime::page_command(&self.control.commands, self.id, command)
            .map_err(Into::into)
    }
}

fn validate_url(url: &str) -> Result<()> {
    let valid = !url.trim().is_empty()
        && (url.starts_with("http://") || url.starts_with("https://") || url.starts_with("about:"));
    valid
        .then_some(())
        .ok_or_else(|| BrowserKitError::InvalidUrl(url.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_with_pages() -> (BrowserKit, Window, Page, Page) {
        let app = BrowserKit::builder().build().unwrap();
        let window = app.create_window().unwrap();
        let first = window
            .create_page(PageOptions::new("https://example.com"))
            .unwrap();
        let second = window
            .create_page(PageOptions::new("https://example.org"))
            .unwrap();
        (app, window, first, second)
    }

    #[test]
    fn ids_are_unique() {
        let app_a = BrowserKit::builder().build().unwrap();
        let app_b = BrowserKit::builder().build().unwrap();
        let window_a = app_a.create_window().unwrap();
        let window_b = app_b.create_window().unwrap();
        assert_ne!(window_a.id(), window_b.id());
        let page_a = window_a
            .create_page(PageOptions::new("https://example.com"))
            .unwrap();
        let page_b = window_a
            .create_page(PageOptions::new("https://example.org"))
            .unwrap();
        assert_ne!(page_a.id(), page_b.id());
    }

    #[test]
    fn first_page_is_active_and_order_is_stable() {
        let (_app, window, first, second) = window_with_pages();
        assert!(first.is_active());
        assert!(!second.is_active());
        assert_eq!(
            window.pages().iter().map(Page::id).collect::<Vec<_>>(),
            vec![first.id(), second.id()]
        );
        assert_eq!(window.state().unwrap().active_page_id, Some(first.id()));
    }

    #[test]
    fn switching_updates_active_state_without_changing_order() {
        let (_app, window, first, second) = window_with_pages();
        window.set_active_page(second.id()).unwrap();
        assert!(!first.is_active());
        assert!(second.is_active());
        assert_eq!(window.active_page().unwrap().id(), second.id());
        assert_eq!(window.state().unwrap().pages.len(), 2);
    }

    #[test]
    fn invalid_activation_is_rejected() {
        let (_app, window, _first, _second) = window_with_pages();
        assert!(matches!(
            window.set_active_page(PageId::from_raw(u64::MAX)),
            Err(BrowserKitError::PageNotFound(_))
        ));
    }

    #[test]
    fn active_close_prefers_right_then_left() {
        let app = BrowserKit::builder().build().unwrap();
        let window = app.create_window().unwrap();
        let first = window
            .create_page(PageOptions::new("https://example.com"))
            .unwrap();
        let second = window
            .create_page(PageOptions::new("https://example.org"))
            .unwrap();
        let third = window
            .create_page(PageOptions::new("https://example.net"))
            .unwrap();
        window.close_page(second.id()).unwrap();
        assert_eq!(window.active_page().unwrap().id(), first.id());
        window.set_active_page(third.id()).unwrap();
        window.close_page(third.id()).unwrap();
        assert_eq!(window.active_page().unwrap().id(), first.id());
    }

    #[test]
    fn inactive_close_preserves_active_page() {
        let (_app, window, first, second) = window_with_pages();
        window.close_page(second.id()).unwrap();
        assert_eq!(window.active_page().unwrap().id(), first.id());
        assert_eq!(window.pages().len(), 1);
    }

    #[test]
    fn final_page_close_leaves_window_empty() {
        let app = BrowserKit::builder().build().unwrap();
        let window = app.create_window().unwrap();
        let page = window
            .create_page(PageOptions::new("https://example.com"))
            .unwrap();
        window.close_page(page.id()).unwrap();
        assert!(window.active_page().is_none());
        assert!(window.pages().is_empty());
        assert_eq!(window.state().unwrap().active_page_id, None);
    }

    #[test]
    fn repeated_switching_keeps_the_same_pages() {
        let (_app, window, first, second) = window_with_pages();
        for _ in 0..100 {
            window.set_active_page(first.id()).unwrap();
            window.set_active_page(second.id()).unwrap();
        }
        assert_eq!(
            window.pages().iter().map(Page::id).collect::<Vec<_>>(),
            vec![first.id(), second.id()]
        );
        assert_eq!(window.active_page().unwrap().id(), second.id());
    }

    #[test]
    fn protocol_handshake_and_version_mismatch_are_structured() {
        let app = BrowserKit::builder().build().unwrap();
        let router = app.protocol_router();
        let response = router.handle_message(r#"{"type":"request","id":"req-1","request":{"type":"runtime_handshake","protocol_version":1}}"#).unwrap();
        assert!(response.contains("runtime_handshake"));
        let mismatch = router.handle_message(r#"{"type":"request","id":"req-2","request":{"type":"runtime_handshake","protocol_version":99}}"#).unwrap();
        assert!(mismatch.contains("protocol_version_mismatch"));
    }

    #[test]
    fn malformed_protocol_input_does_not_panic() {
        let app = BrowserKit::builder().build().unwrap();
        let router = app.protocol_router();
        let response = router
            .handle_message(r#"{"type":"request","id":"req-3","request":{"type":"unknown"}}"#)
            .unwrap();
        assert!(response.contains("unsupported_message"));
    }

    #[test]
    fn protocol_state_request_uses_authoritative_snapshot() {
        let app = BrowserKit::builder().build().unwrap();
        let window = app.create_window().unwrap();
        let page = window
            .create_page(PageOptions::new("https://example.com"))
            .unwrap();
        let router = app.protocol_router();
        let request = format!(
            r#"{{"type":"request","id":"req-4","request":{{"type":"page_get_state","page_id":{}}}}}"#,
            page.id().get()
        );
        let response = router.handle_message(&request).unwrap();
        assert!(response.contains("https://example.com"));
    }

    #[test]
    fn invalid_view_bounds_are_rejected_before_runtime_dispatch() {
        let app = BrowserKit::builder().build().unwrap();
        let window = app.create_window().unwrap();
        let page = window
            .create_page(PageOptions::new("https://example.com"))
            .unwrap();
        let result = page.set_view_bounds(PageViewBounds {
            rect: LogicalRect {
                x: 0.0,
                y: 0.0,
                width: -1.0,
                height: 1.0,
            },
            coordinate_space: CoordinateSpace::FrontendLogical,
            device_pixel_ratio: 1.0,
            visual_viewport_scale: 1.0,
        });
        assert!(matches!(result, Err(BrowserKitError::InvalidViewBounds(_))));
    }

    #[test]
    fn unknown_page_view_bounds_do_not_panic() {
        let app = BrowserKit::builder().build().unwrap();
        let router = app.protocol_router();
        let response = router.handle_message(
            r#"{"type":"command","command":{"type":"page_set_view_bounds","page_id":999,"rect":{"x":0.5,"y":0.5,"width":10.0,"height":10.0},"coordinate_space":"frontend_logical","device_pixel_ratio":1.0,"visual_viewport_scale":1.0}}"#,
        );
        assert!(response.is_none());
    }
}

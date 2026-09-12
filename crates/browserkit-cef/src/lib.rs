//! CEF Views integration. All View/Browser objects stay on CEF's UI thread.

mod compositor;

use browserkit_types::protocol::ProtocolSink;
use browserkit_types::LogicalRect;
use cef::wrapper::message_router::{
    BrowserSideHandler, BrowserSideRouter, MessageRouterBrowserSide,
    MessageRouterBrowserSideHandlerCallbacks, MessageRouterConfig, MessageRouterRendererSide,
    MessageRouterRendererSideHandlerCallbacks, RendererSideRouter,
};
use cef::wrapper::stream_resource_handler::StreamResourceHandler;
use cef::{args::Args, *};
use compositor::{
    clipped_dirty_rects, initialize_threads, surface_bounds, viewport_from_bounds, FrameKind,
    InputEvent, NativeCompositor, PageFrameSource, PageSurface,
};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompatibilityConfig {
    disable_gpu: bool,
    disable_vulkan: bool,
    no_sandbox: bool,
    ozone_platform: Option<String>,
}

impl CompatibilityConfig {
    fn from_environment() -> Self {
        Self::from_values(
            std::env::var("BROWSERKIT_DISABLE_GPU").ok().as_deref(),
            std::env::var("BROWSERKIT_DISABLE_VULKAN").ok().as_deref(),
            std::env::var("BROWSERKIT_NO_SANDBOX").ok().as_deref(),
            std::env::var("BROWSERKIT_OZONE_PLATFORM").ok().as_deref(),
        )
    }

    fn from_values(
        disable_gpu: Option<&str>,
        disable_vulkan: Option<&str>,
        no_sandbox: Option<&str>,
        ozone_platform: Option<&str>,
    ) -> Self {
        let ozone_platform = ozone_platform.and_then(|value| match value {
            "wayland" | "x11" => Some(value.to_string()),
            other => {
                eprintln!("BrowserKit: ignoring invalid BROWSERKIT_OZONE_PLATFORM={other:?}; expected wayland or x11");
                None
            }
        });
        Self {
            disable_gpu: disable_gpu == Some("1"),
            disable_vulkan: disable_vulkan == Some("1"),
            no_sandbox: no_sandbox == Some("1"),
            ozone_platform,
        }
    }

    fn apply(&self, command_line: &mut CommandLine) {
        if self.disable_gpu {
            for switch in ["disable-gpu", "disable-gpu-compositing", "disable-vulkan"] {
                command_line.append_switch(Some(&CefString::from(switch)));
            }
        }
        if self.disable_vulkan {
            command_line.append_switch_with_value(
                Some(&CefString::from("disable-features")),
                Some(&CefString::from("Vulkan")),
            );
        }
        if self.no_sandbox {
            command_line.append_switch(Some(&CefString::from("no-sandbox")));
        }
        if let Some(platform) = &self.ozone_platform {
            command_line.append_switch_with_value(
                Some(&CefString::from("ozone-platform")),
                Some(&CefString::from(platform.as_str())),
            );
        }
    }

    fn applied_switches(&self) -> Vec<String> {
        let mut switches = Vec::new();
        if self.disable_gpu {
            switches.extend([
                "--disable-gpu".to_string(),
                "--disable-gpu-compositing".to_string(),
                "--disable-vulkan".to_string(),
            ]);
        }
        if self.disable_vulkan {
            switches.push("--disable-features=Vulkan".to_string());
        }
        if self.no_sandbox {
            switches.push("--no-sandbox".to_string());
        }
        if let Some(platform) = &self.ozone_platform {
            switches.push(format!("--ozone-platform={platform}"));
        }
        switches
    }
}

fn log_compatibility(config: &CompatibilityConfig) {
    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "<unset>".into());
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let display = std::env::var_os("DISPLAY").is_some();
    let selected = config
        .ozone_platform
        .as_deref()
        .unwrap_or("CEF/Chromium default");
    println!("BrowserKit GPU diagnostics:");
    println!("- session type: {session}");
    println!("- WAYLAND_DISPLAY present: {wayland}");
    println!("- DISPLAY present: {display}");
    println!("- selected ozone platform: {selected}");
    println!(
        "- disable Vulkan feature: {}",
        if config.disable_vulkan { "yes" } else { "no" }
    );
    println!(
        "- full GPU disable: {}",
        if config.disable_gpu { "yes" } else { "no" }
    );
    println!(
        "- no-sandbox diagnostic: {}",
        if config.no_sandbox { "yes" } else { "no" }
    );
    let switches = config.applied_switches();
    println!(
        "- applied Chromium switches: {}",
        if switches.is_empty() {
            "none".into()
        } else {
            switches.join(", ")
        }
    );
    if config.disable_vulkan {
        println!("BrowserKit: Vulkan feature disabled");
    }
    if config.disable_gpu && config.disable_vulkan {
        println!("BrowserKit: full GPU disable and Vulkan-only diagnostics are both enabled");
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("CEF command-line arguments could not be initialized")]
    Arguments,
    #[error("CEF child process returned an invalid exit code: {0}")]
    ChildProcess(i32),
    #[error("CEF initialization failed")]
    Initialization,
    #[error("CEF runtime resources could not be located")]
    Resources,
    #[error("CEF runtime setup failed: {0}")]
    Runtime(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FrontendSource {
    DevServer {
        url: String,
        origin: String,
        token: String,
    },
    AssetsDirectory {
        root: PathBuf,
        token: String,
    },
}

impl FrontendSource {
    fn from_environment() -> Result<Self, Error> {
        let token = std::env::var("BROWSERKIT_FRONTEND_TOKEN")
            .ok()
            .filter(|token| !token.is_empty())
            .unwrap_or_else(frontend_token);
        if let Ok(url) = std::env::var("BROWSERKIT_FRONTEND_URL") {
            let url = url.trim_end_matches('/').to_string();
            let Some(origin) = frontend_origin(&url) else {
                return Err(Error::Runtime(format!(
                    "invalid BROWSERKIT_FRONTEND_URL={url:?}"
                )));
            };
            let local_origin = is_local_dev_origin(&origin);
            if !local_origin {
                return Err(Error::Runtime(
                    "BROWSERKIT_FRONTEND_URL must use localhost or 127.0.0.1".into(),
                ));
            }
            return Ok(Self::DevServer { url, origin, token });
        }

        let root = std::env::var_os("BROWSERKIT_FRONTEND_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("examples/react-browser/dist")
            });
        if !root.join("index.html").is_file() {
            return Err(Error::Runtime(format!(
                "frontend assets missing at {}; run `pnpm --filter react-browser build` or set BROWSERKIT_FRONTEND_URL",
                root.display()
            )));
        }
        Ok(Self::AssetsDirectory { root, token })
    }

    fn entry_url(&self) -> String {
        match self {
            Self::DevServer { url, token, .. } => append_frontend_token(url, token),
            Self::AssetsDirectory { token, .. } => {
                append_frontend_token("browserkit://app/index.html", token)
            }
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::DevServer { .. } => "dev_server",
            Self::AssetsDirectory { .. } => "assets",
        }
    }

    fn token(&self) -> &str {
        match self {
            Self::DevServer { token, .. } | Self::AssetsDirectory { token, .. } => token,
        }
    }

    fn is_trusted_url(&self, url: &str) -> bool {
        match self {
            Self::DevServer { origin, token, .. } => {
                frontend_origin(url).as_deref() == Some(origin) && has_frontend_token(url, token)
            }
            Self::AssetsDirectory { token, .. } => {
                (url == "browserkit://app" || url.starts_with("browserkit://app/"))
                    && has_frontend_token(url, token)
            }
        }
    }
}

fn frontend_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:x}-{:x}", std::process::id())
}

fn append_frontend_token(url: &str, token: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}browserkit_token={token}")
}

fn has_frontend_token(url: &str, token: &str) -> bool {
    let Some(query) = url.split_once('?').map(|(_, query)| query) else {
        return false;
    };
    query
        .split('#')
        .next()
        .unwrap_or_default()
        .split('&')
        .any(|part| part == format!("browserkit_token={token}"))
}

fn frontend_origin(url: &str) -> Option<String> {
    let scheme_end = url.find("://")?;
    let authority_start = scheme_end + 3;
    let authority_end = url[authority_start..]
        .find(['/', '?', '#'])
        .map(|offset| authority_start + offset)
        .unwrap_or(url.len());
    (authority_end > authority_start).then(|| url[..authority_end].to_string())
}

fn is_local_dev_origin(origin: &str) -> bool {
    ["http://127.0.0.1", "http://localhost"].iter().any(|host| {
        origin == *host
            || origin
                .strip_prefix(&format!("{host}:"))
                .is_some_and(|port| {
                    !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
                })
    })
}

#[derive(Clone, Debug)]
pub enum PageCommand {
    Create {
        window_id: browserkit_types::WindowId,
        page: browserkit_types::PageState,
    },
    Navigate {
        page_id: browserkit_types::PageId,
        url: String,
    },
    Reload {
        page_id: browserkit_types::PageId,
    },
    GoBack {
        page_id: browserkit_types::PageId,
    },
    GoForward {
        page_id: browserkit_types::PageId,
    },
    Stop {
        page_id: browserkit_types::PageId,
    },
    Activate {
        window_id: browserkit_types::WindowId,
        page_id: browserkit_types::PageId,
    },
    Close {
        window_id: browserkit_types::WindowId,
        page_id: browserkit_types::PageId,
    },
    RegisterView {
        page_id: browserkit_types::PageId,
    },
    SetViewBounds {
        page_id: browserkit_types::PageId,
        bounds: browserkit_types::PageViewBounds,
    },
    UnregisterView {
        page_id: browserkit_types::PageId,
    },
    Input {
        event: InputEvent,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PageRenderMode {
    NativeView,
    Offscreen,
}

impl PageRenderMode {
    fn from_environment() -> Self {
        match std::env::var("BROWSERKIT_PAGE_RENDERER").ok().as_deref() {
            Some("osr") => Self::Offscreen,
            _ => Self::NativeView,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RawDirtyRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PopupRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone)]
pub struct CommandQueue {
    queue: std::sync::Arc<std::sync::Mutex<VecDeque<PageCommand>>>,
}
impl CommandQueue {
    pub fn new() -> Self {
        Self {
            queue: std::sync::Arc::new(std::sync::Mutex::new(VecDeque::new())),
        }
    }
}
impl Default for CommandQueue {
    fn default() -> Self {
        Self::new()
    }
}
type UiDispatcher = Rc<dyn Fn(PageCommand)>;
thread_local! { static UI_DISPATCH: RefCell<Option<UiDispatcher>> = RefCell::new(None); }
thread_local! { static UI_PAGES: RefCell<Vec<PageView>> = const { RefCell::new(Vec::new()) }; }
thread_local! { static UI_CLIENTS: RefCell<Vec<Client>> = const { RefCell::new(Vec::new()) }; }
thread_local! { static UI_WINDOW: RefCell<Option<Window>> = const { RefCell::new(None) }; }
thread_local! { static UI_VIEW_REGISTRATIONS: RefCell<HashMap<browserkit_types::PageId, usize>> = RefCell::new(HashMap::new()); }
thread_local! { static UI_VIEW_BOUNDS: RefCell<HashMap<browserkit_types::PageId, browserkit_types::PageViewBounds>> = RefCell::new(HashMap::new()); }
thread_local! { static UI_APPLIED_BOUNDS: RefCell<HashMap<browserkit_types::PageId, NativeViewBounds>> = RefCell::new(HashMap::new()); }
thread_local! { static UI_VIEW_VISIBILITY: RefCell<HashMap<browserkit_types::PageId, bool>> = RefCell::new(HashMap::new()); }
thread_local! { static UI_INPUT_CAPTURE: RefCell<Option<browserkit_types::PageId>> = const { RefCell::new(None) }; }
thread_local! { static UI_INPUT_FOCUS: RefCell<Option<browserkit_types::PageId>> = const { RefCell::new(None) }; }
thread_local! { static UI_COMPOSITOR: RefCell<Option<NativeCompositor>> = const { RefCell::new(None) }; }
#[derive(Clone)]
struct UiContext {
    state: Arc<Mutex<browserkit_types::WindowState>>,
    emit: Arc<dyn Fn(browserkit_types::BrowserEvent) + Send + Sync>,
    remaining: Arc<AtomicUsize>,
}
thread_local! { static UI_CONTEXT: RefCell<Option<UiContext>> = const { RefCell::new(None) }; }
thread_local! { static CHROME_BROWSER: RefCell<Option<Browser>> = const { RefCell::new(None) }; }
thread_local! { static CHROME_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
thread_local! { static FRONTEND_ENTRY_URL: RefCell<Option<String>> = const { RefCell::new(None) }; }

const CHROME_URL: &str = "browserkit://app/index.html";
pub type ProtocolHandler = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

fn send_to_chrome(message: browserkit_types::protocol::NativeMessage) {
    if !CHROME_READY.with(|ready| ready.get()) {
        return;
    }
    let Ok(json) = serde_json::to_string(&message) else {
        return;
    };
    let Ok(literal) = serde_json::to_string(&json) else {
        return;
    };
    let entry_url = FRONTEND_ENTRY_URL.with(|url| url.borrow().clone());
    CHROME_BROWSER.with(|browser| {
        if let Some(browser) = browser.borrow().as_ref() {
            if let Some(frame) = browser.main_frame() {
                let script = format!(
                    "window.__browserkit && window.__browserkit.receive(JSON.parse({literal}));"
                );
                frame.execute_java_script(
                    Some(&CefString::from(script.as_str())),
                    Some(&CefString::from(entry_url.as_deref().unwrap_or(CHROME_URL))),
                    0,
                );
            }
        }
    });
}

fn send_event(event: browserkit_types::BrowserEvent) {
    CefProtocolSink.send(browserkit_types::protocol::NativeMessage::Event(
        browserkit_types::protocol::EventEnvelope {
            event: event.clone().into(),
        },
    ));
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeViewBounds {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn rounded_i32(value: f64) -> Option<i32> {
    let value = value.round();
    (value.is_finite() && value >= i32::MIN as f64 && value <= i32::MAX as f64)
        .then_some(value as i32)
}

fn native_view_bounds(
    bounds: &browserkit_types::PageViewBounds,
) -> std::result::Result<NativeViewBounds, String> {
    bounds.validate().map_err(str::to_owned)?;
    Ok(NativeViewBounds {
        x: rounded_i32(bounds.rect.x).ok_or_else(|| "x is outside CEF bounds".to_string())?,
        y: rounded_i32(bounds.rect.y).ok_or_else(|| "y is outside CEF bounds".to_string())?,
        width: rounded_i32(bounds.rect.width)
            .ok_or_else(|| "width is outside CEF bounds".to_string())?,
        height: rounded_i32(bounds.rect.height)
            .ok_or_else(|| "height is outside CEF bounds".to_string())?,
    })
}

fn cef_rect(bounds: NativeViewBounds) -> Rect {
    Rect {
        x: bounds.x,
        y: bounds.y,
        width: bounds.width,
        height: bounds.height,
    }
}

fn page_is_active(page_id: browserkit_types::PageId) -> bool {
    UI_CONTEXT.with(|context| {
        context
            .borrow()
            .as_ref()
            .and_then(|context| context.state.lock().ok())
            .is_some_and(|state| state.active_page_id == Some(page_id))
    })
}

fn page_view_registered(page_id: browserkit_types::PageId) -> bool {
    UI_VIEW_REGISTRATIONS.with(|registrations| {
        registrations
            .borrow()
            .get(&page_id)
            .copied()
            .unwrap_or_default()
            > 0
    })
}

fn page_view_should_be_visible(page_id: browserkit_types::PageId) -> bool {
    page_is_active(page_id)
        && page_view_registered(page_id)
        && UI_VIEW_BOUNDS.with(|bounds| {
            bounds
                .borrow()
                .get(&page_id)
                .is_some_and(|bounds| bounds.rect.width > 0.0 && bounds.rect.height > 0.0)
        })
}

fn page_runtime(page_id: browserkit_types::PageId) -> Option<PageView> {
    UI_PAGES.with(|pages| {
        pages
            .borrow()
            .iter()
            .find(|page| page.id == page_id)
            .cloned()
    })
}

fn page_view(page_id: browserkit_types::PageId) -> Option<BrowserView> {
    page_runtime(page_id).and_then(|page| page.view)
}

fn page_browser(page_id: browserkit_types::PageId) -> Option<Browser> {
    page_runtime(page_id).and_then(|page| {
        page.browser
            .or_else(|| page.view.and_then(|view| view.browser()))
    })
}

fn page_surface(page_id: browserkit_types::PageId) -> Option<Rc<RefCell<PageSurface>>> {
    page_runtime(page_id).and_then(|page| page.surface)
}

fn set_page_view_visibility(page_id: browserkit_types::PageId, view: &BrowserView, visible: bool) {
    let was_visible = UI_VIEW_VISIBILITY
        .with(|states| states.borrow_mut().insert(page_id, visible))
        .unwrap_or(false);
    view.set_visible(visible.into());
    if visible && !was_visible {
        view.request_focus();
    }
}

fn reset_frontend_geometry() {
    let page_views = UI_PAGES.with(|pages| {
        pages
            .borrow()
            .iter()
            .map(|page| (page.id, page.view.clone(), page.surface.clone()))
            .collect::<Vec<_>>()
    });
    let zero = Rect::default();
    for (page_id, view, surface) in page_views {
        if let Some(view) = view {
            view.set_bounds(Some(&zero));
            view.set_visible(0);
        }
        if let Some(surface) = surface {
            surface.borrow_mut().visible = false;
        }
        UI_VIEW_VISIBILITY.with(|states| {
            states.borrow_mut().insert(page_id, false);
        });
    }
    UI_COMPOSITOR.with(|compositor| {
        if let Some(compositor) = compositor.borrow_mut().as_mut() {
            compositor.set_visible(false);
        }
    });
    UI_VIEW_REGISTRATIONS.with(|registrations| registrations.borrow_mut().clear());
    UI_VIEW_BOUNDS.with(|bounds| bounds.borrow_mut().clear());
    UI_APPLIED_BOUNDS.with(|bounds| bounds.borrow_mut().clear());
    UI_INPUT_CAPTURE.with(|capture| *capture.borrow_mut() = None);
    UI_INPUT_FOCUS.with(|focus| *focus.borrow_mut() = None);
    println!("BrowserKit: frontend_geometry_reset");
}

fn register_page_view(page_id: browserkit_types::PageId) {
    if page_runtime(page_id).is_none() {
        println!("BrowserKit: page_not_found page={page_id:?}");
        return;
    }
    let count = UI_VIEW_REGISTRATIONS.with(|registrations| {
        let mut registrations = registrations.borrow_mut();
        let count = registrations.entry(page_id).or_default();
        *count += 1;
        *count
    });
    if count > 1 {
        eprintln!("BrowserKit: duplicate_page_view_registration page={page_id:?} count={count}");
    }
    let visible = page_view_should_be_visible(page_id);
    if let Some(view) = page_view(page_id) {
        set_page_view_visibility(page_id, &view, visible);
    }
    if let Some(surface) = page_surface(page_id) {
        surface.borrow_mut().visible = visible;
        if visible {
            UI_COMPOSITOR.with(|compositor| {
                if let Some(compositor) = compositor.borrow_mut().as_mut() {
                    compositor.set_visible(true);
                    compositor.present(&mut surface.borrow_mut());
                }
            });
        }
    }
}

fn unregister_page_view(page_id: browserkit_types::PageId) {
    let count = UI_VIEW_REGISTRATIONS.with(|registrations| {
        let mut registrations = registrations.borrow_mut();
        let Some(count) = registrations.get(&page_id).copied() else {
            return 0;
        };
        if count <= 1 {
            registrations.remove(&page_id);
            0
        } else {
            registrations.insert(page_id, count - 1);
            count - 1
        }
    });
    if count > 0 {
        return;
    }
    UI_VIEW_BOUNDS.with(|bounds| {
        bounds.borrow_mut().remove(&page_id);
    });
    UI_APPLIED_BOUNDS.with(|bounds| {
        bounds.borrow_mut().remove(&page_id);
    });
    if let Some(view) = page_view(page_id) {
        view.set_bounds(Some(&Rect::default()));
        set_page_view_visibility(page_id, &view, false);
    }
    if let Some(surface) = page_surface(page_id) {
        surface.borrow_mut().visible = false;
    }
    if page_is_active(page_id) {
        UI_COMPOSITOR.with(|compositor| {
            if let Some(compositor) = compositor.borrow_mut().as_mut() {
                compositor.set_visible(false);
            }
        });
    }
}

fn set_page_view_bounds(
    page_id: browserkit_types::PageId,
    bounds: browserkit_types::PageViewBounds,
) {
    println!(
        "BrowserKit: page_view_bounds_received page={page_id:?} logical=({}, {}, {}, {})",
        bounds.rect.x, bounds.rect.y, bounds.rect.width, bounds.rect.height
    );
    let native = match native_view_bounds(&bounds) {
        Ok(native) => native,
        Err(error) => {
            eprintln!("BrowserKit: invalid_page_view_bounds page={page_id:?} error={error}");
            return;
        }
    };
    let Some(page) = page_runtime(page_id) else {
        println!("BrowserKit: page_not_found page={page_id:?}");
        return;
    };
    let duplicate = UI_VIEW_BOUNDS.with(|previous| {
        let mut previous = previous.borrow_mut();
        if previous.get(&page_id) == Some(&bounds) {
            true
        } else {
            previous.insert(page_id, bounds);
            false
        }
    });
    if duplicate {
        println!("BrowserKit: page_view_bounds_deduplicated page={page_id:?}");
        return;
    }
    if page.mode == PageRenderMode::NativeView {
        let native_duplicate = UI_APPLIED_BOUNDS.with(|previous| {
            let mut previous = previous.borrow_mut();
            if previous.get(&page_id) == Some(&native) {
                true
            } else {
                previous.insert(page_id, native);
                false
            }
        });
        if !native_duplicate {
            if let Some(view) = page.view.as_ref() {
                view.set_bounds(Some(&cef_rect(native)));
            }
        }
        if let Some(view) = page.view.as_ref() {
            set_page_view_visibility(page_id, view, page_view_should_be_visible(page_id));
        }
    } else if let Some(surface) = page.surface {
        let viewport = match viewport_from_bounds(&bounds) {
            Ok(viewport) => viewport,
            Err(error) => {
                eprintln!("BrowserKit: invalid_osr_viewport page={page_id:?} error={error}");
                return;
            }
        };
        let changed = surface.borrow_mut().set_viewport(viewport);
        if changed {
            let visible = page_view_should_be_visible(page_id)
                && viewport.width_px > 0
                && viewport.height_px > 0;
            surface.borrow_mut().visible = visible;
            if let Some(browser) = page.browser {
                if let Some(host) = browser.host() {
                    host.was_hidden((!visible).into());
                    host.notify_screen_info_changed();
                    if visible {
                        host.was_resized();
                        host.invalidate(PaintElementType::VIEW);
                    }
                }
            }
            UI_COMPOSITOR.with(|compositor| {
                if let Some(compositor) = compositor.borrow_mut().as_mut() {
                    if page_is_active(page_id) {
                        if let Ok(native) = surface_bounds(&viewport) {
                            compositor.set_bounds(native);
                        }
                        compositor.set_visible(visible);
                        if visible {
                            compositor.present(&mut surface.borrow_mut());
                        }
                    }
                }
            });
            println!(
                "BrowserKit: osr_viewport_changed page={page_id:?} logical=({}, {}, {}, {}) px=({}, {}) scale={}",
                bounds.rect.x,
                bounds.rect.y,
                bounds.rect.width,
                bounds.rect.height,
                viewport.width_px,
                viewport.height_px,
                viewport.scale_factor
            );
        }
    }
    println!(
        "BrowserKit: page_view_bounds_applied page={page_id:?} logical=({}, {}, {}, {}) cef=({}, {}, {}, {})",
        bounds.rect.x,
        bounds.rect.y,
        bounds.rect.width,
        bounds.rect.height,
        native.x,
        native.y,
        native.width,
        native.height
    );
}

fn apply_active_page_visibility(active_page_id: browserkit_types::PageId) {
    let page_views = UI_PAGES.with(|pages| pages.borrow().iter().cloned().collect::<Vec<_>>());
    for page in page_views {
        let visible = surface_is_active(Some(active_page_id), page.id)
            && page_view_should_be_visible(page.id);
        if let Some(view) = page.view {
            set_page_view_visibility(page.id, &view, visible);
        }
        if let Some(surface) = page.surface {
            surface.borrow_mut().visible = visible;
        }
    }
    if let Some(page) = page_runtime(active_page_id) {
        if page.mode == PageRenderMode::Offscreen {
            if let Some(surface) = page.surface {
                let viewport = surface.borrow().viewport;
                UI_COMPOSITOR.with(|compositor| {
                    if let Some(compositor) = compositor.borrow_mut().as_mut() {
                        if let Ok(bounds) = surface_bounds(&viewport) {
                            compositor.set_bounds(bounds);
                        }
                        let visible = page_view_should_be_visible(active_page_id)
                            && viewport.width_px > 0
                            && viewport.height_px > 0;
                        compositor.set_visible(visible);
                        if visible {
                            compositor.present(&mut surface.borrow_mut());
                        }
                    }
                });
            }
        }
    }
}

struct CefProtocolSink;
impl browserkit_types::protocol::ProtocolSink for CefProtocolSink {
    fn send(&self, message: browserkit_types::protocol::NativeMessage) {
        send_to_chrome(message);
    }
}

fn activate_page_view(window_id: browserkit_types::WindowId, page_id: browserkit_types::PageId) {
    let Some(page) = page_runtime(page_id) else {
        println!("BrowserKit: page_not_found page={page_id:?}");
        return;
    };
    apply_active_page_visibility(page_id);
    if let Some(view) = page.view {
        if page_view_should_be_visible(page_id) {
            view.request_focus();
        }
    }
    if let Some(browser) = page.browser {
        if page_view_should_be_visible(page_id) {
            if let Some(host) = browser.host() {
                host.set_focus(1);
            }
            UI_INPUT_FOCUS.with(|focus| *focus.borrow_mut() = Some(page_id));
        }
    }
    send_event(browserkit_types::BrowserEvent::PageActivated { window_id, page_id });
    println!("BrowserKit: runtime_command_executed page={page_id:?}");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SurfaceTarget {
    Chrome,
    Page(browserkit_types::PageId),
    None,
}

fn hit_test(
    rect: LogicalRect,
    active_page_id: Option<browserkit_types::PageId>,
    x: f64,
    y: f64,
) -> SurfaceTarget {
    let Some(page_id) = active_page_id else {
        return SurfaceTarget::Chrome;
    };
    if x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height {
        SurfaceTarget::Page(page_id)
    } else {
        SurfaceTarget::Chrome
    }
}

fn page_local_point(x: f64, y: f64, rect: LogicalRect) -> (i32, i32) {
    ((x - rect.x).round() as i32, (y - rect.y).round() as i32)
}

fn active_page_id() -> Option<browserkit_types::PageId> {
    UI_CONTEXT.with(|context| {
        context
            .borrow()
            .as_ref()
            .and_then(|context| context.state.lock().ok())
            .and_then(|state| state.active_page_id)
    })
}

fn page_bounds(page_id: browserkit_types::PageId) -> Option<browserkit_types::PageViewBounds> {
    UI_VIEW_BOUNDS.with(|bounds| bounds.borrow().get(&page_id).copied())
}

fn page_input_target(page_id: browserkit_types::PageId, x: f64, y: f64) -> SurfaceTarget {
    let Some(bounds) = page_bounds(page_id) else {
        return SurfaceTarget::None;
    };
    hit_test(
        bounds.rect,
        active_page_id(),
        bounds.rect.x + x,
        bounds.rect.y + y,
    )
}

fn surface_is_active(
    active_page_id: Option<browserkit_types::PageId>,
    page_id: browserkit_types::PageId,
) -> bool {
    active_page_id == Some(page_id)
}

fn update_pointer_capture(
    capture: &mut Option<browserkit_types::PageId>,
    page_id: browserkit_types::PageId,
    mouse_up: bool,
) {
    if mouse_up {
        if *capture == Some(page_id) {
            *capture = None;
        }
    } else if capture.is_none() {
        *capture = Some(page_id);
    }
}

fn route_input(event: InputEvent) {
    let captured = UI_INPUT_CAPTURE.with(|capture| *capture.borrow());
    let page_id = captured.or_else(active_page_id);
    let Some(page_id) = page_id else {
        return;
    };
    let Some(page) = page_runtime(page_id) else {
        return;
    };
    let Some(browser) = page.browser else {
        return;
    };
    let Some(host) = browser.host() else {
        return;
    };
    let Some(bounds) = page_bounds(page_id) else {
        return;
    };
    let scale = page
        .surface
        .as_ref()
        .map(|surface| surface.borrow().viewport.scale_factor)
        .unwrap_or(1.0);
    let local = |x: i32, y: i32| {
        page_local_point(
            bounds.rect.x + x as f64 / scale,
            bounds.rect.y + y as f64 / scale,
            bounds.rect,
        )
    };
    match event {
        InputEvent::MouseMove {
            x_px,
            y_px,
            modifiers,
        } => {
            if matches!(
                page_input_target(page_id, x_px as f64 / scale, y_px as f64 / scale),
                SurfaceTarget::Page(_)
            ) || captured.is_some()
            {
                let (x, y) = local(x_px, y_px);
                host.send_mouse_move_event(Some(&MouseEvent { x, y, modifiers }), 0);
                println!("BrowserKit: osr_input_mouse page={page_id:?} kind=move");
            }
        }
        InputEvent::MouseLeave {
            x_px,
            y_px,
            modifiers,
        } => {
            let (x, y) = local(x_px, y_px);
            host.send_mouse_move_event(Some(&MouseEvent { x, y, modifiers }), 1);
            println!("BrowserKit: osr_input_mouse page={page_id:?} kind=leave");
        }
        InputEvent::MouseButton {
            x_px,
            y_px,
            button,
            mouse_up,
            click_count,
            modifiers,
        } => {
            let target = page_input_target(page_id, x_px as f64 / scale, y_px as f64 / scale);
            if !mouse_up && !matches!(target, SurfaceTarget::Page(_)) && captured.is_none() {
                return;
            }
            let Some(button) = (match button {
                1 => Some(MouseButtonType::LEFT),
                2 => Some(MouseButtonType::MIDDLE),
                3 => Some(MouseButtonType::RIGHT),
                _ => None,
            }) else {
                return;
            };
            let (x, y) = local(x_px, y_px);
            host.set_focus(1);
            UI_INPUT_FOCUS.with(|focus| *focus.borrow_mut() = Some(page_id));
            UI_INPUT_CAPTURE.with(|capture| {
                update_pointer_capture(&mut capture.borrow_mut(), page_id, mouse_up)
            });
            host.send_mouse_click_event(
                Some(&MouseEvent { x, y, modifiers }),
                button,
                mouse_up.into(),
                click_count.max(1),
            );
            println!("BrowserKit: osr_input_mouse page={page_id:?} kind=button");
        }
        InputEvent::Wheel {
            x_px,
            y_px,
            delta_x,
            delta_y,
            modifiers,
        } => {
            if !matches!(
                page_input_target(page_id, x_px as f64 / scale, y_px as f64 / scale),
                SurfaceTarget::Page(_)
            ) && captured.is_none()
            {
                return;
            }
            let (x, y) = local(x_px, y_px);
            host.send_mouse_wheel_event(Some(&MouseEvent { x, y, modifiers }), delta_x, delta_y);
            println!("BrowserKit: osr_input_wheel page={page_id:?}");
        }
        InputEvent::Key {
            key_code,
            native_key_code,
            character,
            modifiers,
            pressed,
        } => {
            if UI_INPUT_FOCUS.with(|focus| *focus.borrow()) != Some(page_id) {
                return;
            }
            let mut key = KeyEvent::default();
            key.type_ = if pressed {
                KeyEventType::RAWKEYDOWN
            } else {
                KeyEventType::KEYUP
            };
            key.modifiers = modifiers;
            key.windows_key_code = key_code;
            key.native_key_code = native_key_code;
            key.character = character.unwrap_or_default();
            key.unmodified_character = key.character;
            host.send_key_event(Some(&key));
            if pressed {
                if let Some(character) = character {
                    key.type_ = KeyEventType::CHAR;
                    key.character = character;
                    key.unmodified_character = character;
                    host.send_key_event(Some(&key));
                }
            }
        }
        InputEvent::Focus(focused) => {
            host.set_focus(focused.into());
            UI_INPUT_FOCUS.with(|focus| {
                if focused {
                    *focus.borrow_mut() = Some(page_id);
                } else if *focus.borrow() == Some(page_id) {
                    *focus.borrow_mut() = None;
                }
            });
            println!("BrowserKit: osr_focus_changed page={page_id:?} focused={focused}");
        }
    }
}

fn execute_command(command: PageCommand) {
    if let PageCommand::Create { .. } = &command {
        execute_create_page(command);
        return;
    }
    if let PageCommand::Input { event } = command.clone() {
        route_input(event);
        return;
    }
    match &command {
        PageCommand::RegisterView { page_id } => {
            register_page_view(*page_id);
            return;
        }
        PageCommand::SetViewBounds { page_id, bounds } => {
            set_page_view_bounds(*page_id, *bounds);
            return;
        }
        PageCommand::UnregisterView { page_id } => {
            unregister_page_view(*page_id);
            return;
        }
        PageCommand::Activate { window_id, page_id } => {
            activate_page_view(*window_id, *page_id);
            return;
        }
        _ => {}
    }
    let page_id = match &command {
        PageCommand::Navigate { page_id, .. }
        | PageCommand::Reload { page_id }
        | PageCommand::GoBack { page_id }
        | PageCommand::GoForward { page_id }
        | PageCommand::Stop { page_id }
        | PageCommand::Close { page_id, .. } => *page_id,
        _ => unreachable!(),
    };
    if page_runtime(page_id).is_none() {
        println!("BrowserKit: page_not_found page={page_id:?}");
        return;
    }
    let Some(browser) = page_browser(page_id) else {
        return;
    };
    match command {
        PageCommand::Navigate { url, .. } => {
            if let Some(frame) = browser.main_frame() {
                frame.load_url(Some(&CefString::from(url.as_str())));
            }
        }
        PageCommand::Reload { .. } => browser.reload(),
        PageCommand::GoBack { .. } => {
            if browser.can_go_back() == 1 {
                browser.go_back();
            }
        }
        PageCommand::GoForward { .. } => {
            if browser.can_go_forward() == 1 {
                browser.go_forward();
            }
        }
        PageCommand::Stop { .. } => browser.stop_load(),
        PageCommand::Close { .. } => {
            if let Some(host) = browser.host() {
                let _ = host.try_close_browser();
            }
        }
        _ => unreachable!(),
    }
    println!("BrowserKit: runtime_command_executed page={page_id:?}");
}

fn execute_create_page(command: PageCommand) {
    let PageCommand::Create { window_id, page } = command else {
        return;
    };
    let Some(context) = UI_CONTEXT.with(|context| context.borrow().clone()) else {
        return;
    };
    let Some(window) = UI_WINDOW.with(|window| window.borrow().clone()) else {
        return;
    };
    let mut clients = UI_CLIENTS.with(|clients| std::mem::take(&mut *clients.borrow_mut()));
    let Some((client, runtime)) = create_page_runtime(window_id, &page, &context) else {
        eprintln!("BrowserKit: page_view_create_failed page={:?}", page.id);
        UI_CLIENTS.with(|slot| *slot.borrow_mut() = clients);
        return;
    };
    context.remaining.fetch_add(1, Ordering::AcqRel);
    clients.push(client);
    UI_CLIENTS.with(|slot| *slot.borrow_mut() = clients);
    if let Some(view) = runtime.view.as_ref() {
        let mut child = View::from(view);
        window.add_child_view(Some(&mut child));
    }
    UI_PAGES.with(|pages| pages.borrow_mut().push(runtime));
    println!(
        "BrowserKit: page_view_created window={window_id:?} page={:?} mode={:?}",
        page.id,
        PageRenderMode::from_environment()
    );
}

pub fn enqueue(queue: &CommandQueue, command: PageCommand) -> Result<(), Error> {
    let queue = queue.queue.clone();
    queue
        .lock()
        .map_err(|_| Error::Runtime("command queue poisoned".into()))?
        .push_back(command);
    let mut task = DrainCommands::new(queue);
    if post_task(ThreadId::UI, Some(&mut task)) != 1 {
        return Err(Error::Runtime("CEF UI task could not be posted".into()));
    }
    Ok(())
}

wrap_task! {
    struct DrainCommands { queue: std::sync::Arc<std::sync::Mutex<VecDeque<PageCommand>>> }
    impl Task {
        fn execute(&self) {
            let commands = self.queue.lock().ok().map(|mut q| q.drain(..).collect::<Vec<_>>()).unwrap_or_default();
            UI_DISPATCH.with(|dispatcher| {
                if let Some(dispatcher) = dispatcher.borrow().as_ref() {
                    for command in commands { dispatcher(command); }
                }
            });
        }
    }
}

#[derive(Clone)]
pub struct Options {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub window_id: browserkit_types::WindowId,
    pub pages: Vec<browserkit_types::PageState>,
    pub state: std::sync::Arc<std::sync::Mutex<browserkit_types::WindowState>>,
    pub commands: CommandQueue,
    pub protocol_handler: ProtocolHandler,
    pub emit: std::sync::Arc<dyn Fn(browserkit_types::BrowserEvent) + Send + Sync>,
}

wrap_app! {
    struct Application {
        browser_process: BrowserProcessHandler,
        render_process: RenderProcessHandler,
        compatibility: CompatibilityConfig,
        frontend: FrontendSource,
    }
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> { Some(self.browser_process.clone()) }
        fn render_process_handler(&self) -> Option<RenderProcessHandler> { Some(self.render_process.clone()) }
        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            if self.frontend.is_assets() {
                if let Some(registrar) = registrar {
                    let name = CefString::from("browserkit");
                    registrar.add_custom_scheme(Some(&name), (SchemeOptions::STANDARD.get_raw() | SchemeOptions::SECURE.get_raw() | SchemeOptions::CORS_ENABLED.get_raw() | SchemeOptions::FETCH_ENABLED.get_raw()) as i32);
                }
            }
        }
        fn on_before_command_line_processing(&self, process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
            if let Some(process_type) = process_type {
                println!("BrowserKit: CEF process command-line hook type={process_type}");
                if process_type.to_string() == "gpu-process" {
                    println!("BrowserKit: configuring GPU subprocess command line");
                }
            }
            if let Some(command_line) = command_line { self.compatibility.apply(command_line); }
        }
    }
}

wrap_render_process_handler! {
    struct RenderProcessCallbacks { router: Arc<RendererSideRouter>, frontend: FrontendSource }
    impl RenderProcessHandler {
        fn on_context_created(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, context: Option<&mut V8Context>) {
            let allowed = frame.as_ref().map(|frame| frame.is_main() == 1 && self.frontend.is_trusted_url(&CefString::from(&frame.url()).to_string())).unwrap_or(false);
            if allowed { self.router.on_context_created(browser.cloned(), frame.cloned(), context.cloned()); }
        }
        fn on_context_released(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, context: Option<&mut V8Context>) {
            let allowed = frame.as_ref().map(|frame| frame.is_main() == 1 && self.frontend.is_trusted_url(&CefString::from(&frame.url()).to_string())).unwrap_or(false);
            if allowed { self.router.on_context_released(browser.cloned(), frame.cloned(), context.cloned()); }
        }
        fn on_process_message_received(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, source_process: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
            let allowed = frame.as_ref().map(|frame| frame.is_main() == 1 && self.frontend.is_trusted_url(&CefString::from(&frame.url()).to_string())).unwrap_or(false);
            if allowed { self.router.on_process_message_received(browser.cloned(), frame.cloned(), Some(source_process), message.cloned()).into() } else { 0 }
        }
    }
}

wrap_scheme_handler_factory! {
    struct FrontendSchemeFactory { root: PathBuf }
    impl SchemeHandlerFactory {
        fn create(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _scheme_name: Option<&CefString>, request: Option<&mut Request>) -> Option<ResourceHandler> {
            let url = request.map(|request| CefString::from(&request.url()).to_string()).unwrap_or_default();
            println!("BrowserKit: frontend_resource_request url={url}");
            let (bytes, mime_type) = frontend_asset(&self.root, &url)?;
            let mut bytes = bytes;
            let stream = stream_reader_create_for_data(bytes.as_mut_ptr(), bytes.len())?;
            Some(StreamResourceHandler::new_with_stream(mime_type, stream))
        }
    }
}

impl FrontendSource {
    fn is_assets(&self) -> bool {
        matches!(self, Self::AssetsDirectory { .. })
    }
}

fn frontend_asset(root: &Path, url: &str) -> Option<(Vec<u8>, String)> {
    let path = url
        .strip_prefix("browserkit://app")?
        .split(['?', '#'])
        .next()
        .unwrap_or_default();
    let relative = if path.is_empty() || path == "/" {
        "index.html"
    } else {
        path.strip_prefix('/')?
    };
    if relative.contains('\\')
        || relative.contains('%')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let candidate = root.join(relative);
    let path = if candidate.is_file() {
        let path = std::fs::canonicalize(candidate).ok()?;
        path.starts_with(&canonical_root).then_some(path)?
    } else if Path::new(relative).extension().is_none() {
        let path = std::fs::canonicalize(root.join("index.html")).ok()?;
        path.starts_with(&canonical_root).then_some(path)?
    } else {
        return None;
    };
    let mime_type = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    };
    Some((std::fs::read(path).ok()?, mime_type.into()))
}

struct FrontendQueryHandler {
    handler: ProtocolHandler,
    frontend: FrontendSource,
}
impl BrowserSideHandler for FrontendQueryHandler {
    fn on_query_str(
        &self,
        browser: Option<Browser>,
        frame: Option<Frame>,
        _query_id: i64,
        request: &str,
        _persistent: bool,
        callback: Arc<Mutex<dyn cef::wrapper::message_router::BrowserSideCallback>>,
    ) -> bool {
        let allowed = frame
            .as_ref()
            .map(|frame| {
                frame.is_main() == 1
                    && self
                        .frontend
                        .is_trusted_url(&CefString::from(&frame.url()).to_string())
            })
            .unwrap_or(false)
            && browser.is_some();
        if !allowed {
            if let Ok(callback) = callback.lock() {
                callback.failure(-1, "frontend transport is restricted to the chrome frame");
            }
            return true;
        }
        if let Some(response) = (self.handler)(request) {
            if let Ok(callback) = callback.lock() {
                callback.success_str(&response);
            }
            let handshake = request.contains("\"runtime_handshake\"");
            let successful = response.contains("\"status\":\"ok\"");
            CHROME_READY.with(|ready| ready.set(handshake && successful));
            if handshake && successful {
                reset_frontend_geometry();
                println!("BrowserKit: frontend_handshake");
                println!("BrowserKit: frontend_ready");
            }
        } else if let Ok(callback) = callback.lock() {
            callback.success_str("");
        }
        true
    }
}

wrap_browser_process_handler! {
    struct BrowserProcessCallbacks {
        options: Options,
        clients: RefCell<Vec<Client>>,
        remaining: Arc<AtomicUsize>,
        browser_router: Arc<BrowserSideRouter>,
        frontend: FrontendSource,
    }
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            let options = &self.options;
            if let FrontendSource::AssetsDirectory { root, .. } = &self.frontend {
                let mut factory = FrontendSchemeFactory::new(root.clone());
                register_scheme_handler_factory(Some(&CefString::from("browserkit")), Some(&CefString::from("app")), Some(&mut factory));
            }
            let _ = self.browser_router.add_handler(Arc::new(FrontendQueryHandler { handler: options.protocol_handler.clone(), frontend: self.frontend.clone() }), true);
            FRONTEND_ENTRY_URL.with(|url| *url.borrow_mut() = Some(self.frontend.entry_url()));
            let context = UiContext { state: options.state.clone(), emit: options.emit.clone(), remaining: self.remaining.clone() };
            UI_CONTEXT.with(|slot| *slot.borrow_mut() = Some(context.clone()));
            let mut chrome_client = BrowserClient::new(Arc::new(BrowserClientState {
                window_id: options.window_id,
                page_id: None,
                browser_id: Mutex::new(None),
                state: options.state.clone(),
                emit: options.emit.clone(),
                remaining: self.remaining.clone(),
                browser_router: Some(self.browser_router.clone()),
                render_handler: None,
            }));
            let chrome_client_ref = chrome_client.clone();
            let mut chrome_delegate = BrowserViewCallbacks::new(Size { width: options.width as i32, height: options.height as i32 });
            let chrome_url = CefString::from(self.frontend.entry_url().as_str());
            let Some(chrome_view) = browser_view_create(Some(&mut chrome_client), Some(&chrome_url), Some(&BrowserSettings::default()), None, None, Some(&mut chrome_delegate)) else {
                eprintln!("BrowserKit: chrome_view_create_failed"); quit_message_loop(); return;
            };
            println!("BrowserKit: chrome_view_created");
            self.clients.borrow_mut().push(chrome_client_ref);
            let mut pages = Vec::with_capacity(options.pages.len());
            for page in &options.pages {
                let Some((client, runtime)) = create_page_runtime(options.window_id, page, &context) else {
                    eprintln!("BrowserKit: page_view_create_failed page={:?}", page.id);
                    quit_message_loop();
                    return;
                };
                self.clients.borrow_mut().push(client);
                println!("BrowserKit: page_view_created window={:?} page={:?} mode={:?}", options.window_id, page.id, runtime.mode);
                pages.push(runtime);
            }
            println!("BrowserKit: window_create_requested window={:?}", options.window_id);
            let ui_pages = pages.clone();
            let mut delegate = BrowserKitWindow::new(
                RefCell::new(pages), chrome_view, options.title.clone(),
                Size { width: options.width as i32, height: options.height as i32 },
                options.pages.iter().find(|page| page.active).map(|page| page.id),
                options.commands.clone(),
            );
            if window_create_top_level(Some(&mut delegate)).is_none() { quit_message_loop(); }
            UI_PAGES.with(|pages| *pages.borrow_mut() = ui_pages);
            UI_CLIENTS.with(|clients| *clients.borrow_mut() = self.clients.borrow().clone());
            UI_DISPATCH.with(|dispatcher| *dispatcher.borrow_mut() = Some(Rc::new(execute_command)));
        }
    }
}

wrap_browser_view_delegate! {
    struct BrowserViewCallbacks { preferred_size: Size }
    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size { self.preferred_size.clone() }
    }
    impl BrowserViewDelegate {
        fn browser_runtime_style(&self) -> RuntimeStyle { RuntimeStyle::ALLOY }
        fn on_popup_browser_view_created(&self, _browser_view: Option<&mut BrowserView>, _popup: Option<&mut BrowserView>, _devtools: i32) -> i32 {
            println!("BrowserKit: popup_deferred");
            1
        }
    }
}

fn debug_render_logs() -> bool {
    std::env::var("BROWSERKIT_DEBUG_RENDER").ok().as_deref() == Some("1")
}

wrap_render_handler! {
    struct PageRenderCallbacks {
        page_id: browserkit_types::PageId,
        surface: Rc<RefCell<PageSurface>>,
    }
    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            if let Some(rect) = rect {
                let viewport = self.surface.borrow().viewport;
                rect.width = viewport.rect.width.max(1.0).round() as i32;
                rect.height = viewport.rect.height.max(1.0).round() as i32;
            }
        }
        fn screen_info(
            &self,
            _browser: Option<&mut Browser>,
            screen_info: Option<&mut ScreenInfo>,
        ) -> std::os::raw::c_int {
            if let Some(screen_info) = screen_info {
                screen_info.device_scale_factor = self.surface.borrow().viewport.scale_factor as f32;
                return 1;
            }
            0
        }
        fn screen_point(
            &self,
            _browser: Option<&mut Browser>,
            _view_x: std::os::raw::c_int,
            _view_y: std::os::raw::c_int,
            _screen_x: Option<&mut std::os::raw::c_int>,
            _screen_y: Option<&mut std::os::raw::c_int>,
        ) -> std::os::raw::c_int { 0 }
        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: std::os::raw::c_int) {
            if show == 0 {
                self.surface.borrow_mut().set_popup_rect(None);
                let surface = self.surface.clone();
                UI_COMPOSITOR.with(|compositor| {
                    if let Some(compositor) = compositor.borrow_mut().as_mut() {
                        if page_is_active(self.page_id) {
                            compositor.present(&mut surface.borrow_mut());
                        }
                    }
                });
            }
        }
        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            if let Some(rect) = rect {
                self.surface.borrow_mut().set_popup_rect(Some(PopupRect {
                    x: rect.x as f64,
                    y: rect.y as f64,
                    width: rect.width.max(1) as f64,
                    height: rect.height.max(1) as f64,
                }));
            }
        }
        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: std::os::raw::c_int,
            height: std::os::raw::c_int,
        ) {
            if buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let (width, height) = (width as u32, height as u32);
            let Some(byte_len) = (width as usize)
                .checked_mul(height as usize)
                .and_then(|value| value.checked_mul(4)) else {
                return;
            };
            let buffer = unsafe { std::slice::from_raw_parts(buffer, byte_len) };
            let raw_dirty = dirty_rects
                .unwrap_or_default()
                .iter()
                .map(|rect| RawDirtyRect {
                    x: rect.x,
                    y: rect.y,
                    width: rect.width,
                    height: rect.height,
                })
                .collect::<Vec<_>>();
            let dirty = clipped_dirty_rects(&raw_dirty, width, height);
            let kind = if type_ == PaintElementType::POPUP {
                FrameKind::Popup
            } else {
                FrameKind::View
            };
            let started = std::time::Instant::now();
            let result = self
                .surface
                .borrow_mut()
                .receive_frame(kind, &dirty, buffer, width, height);
            match result {
                Ok(()) => {
                    if debug_render_logs() {
                        println!("BrowserKit: osr_frame_received page={:?} size={}x{}", self.surface.borrow().page_id(), width, height);
                    }
                    let surface = self.surface.clone();
                    UI_COMPOSITOR.with(|compositor| {
                        if let Some(compositor) = compositor.borrow_mut().as_mut() {
                            if page_is_active(self.page_id) {
                                compositor.present(&mut surface.borrow_mut());
                                if debug_render_logs() {
                                    println!("BrowserKit: osr_frame_presented page={:?}", self.surface.borrow().page_id());
                                }
                            }
                        }
                    });
                }
                Err(error) => {
                    self.surface.borrow_mut().metrics.frames_dropped += 1;
                    eprintln!("BrowserKit: osr_frame_dropped page={:?} error={error}", self.page_id);
                }
            }
            self.surface.borrow_mut().metrics.paint_time += started.elapsed();
        }
    }
}

#[derive(Clone)]
struct PageView {
    id: browserkit_types::PageId,
    mode: PageRenderMode,
    view: Option<BrowserView>,
    browser: Option<Browser>,
    surface: Option<Rc<RefCell<PageSurface>>>,
}

wrap_window_delegate! {
    struct BrowserKitWindow {
        pages: RefCell<Vec<PageView>>,
        chrome_view: BrowserView,
        title: String,
        size: Size,
        active_page_id: Option<browserkit_types::PageId>,
        commands: CommandQueue,
    }
    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size { self.size.clone() }
    }
    impl PanelDelegate {}
    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let Some(window) = window else { return };
            UI_WINDOW.with(|slot| *slot.borrow_mut() = Some(window.clone()));
            window.set_title(Some(&CefString::from(self.title.as_str())));
            reset_frontend_geometry();
            let window_bounds = window.bounds();
            let chrome_bounds = Rect {
                x: 0,
                y: 0,
                width: if window_bounds.width > 0 { window_bounds.width } else { self.size.width },
                height: if window_bounds.height > 0 { window_bounds.height } else { self.size.height },
            };
            self.chrome_view.set_visible(1);
            self.chrome_view.set_bounds(Some(&chrome_bounds));
            let mut chrome = View::from(&self.chrome_view);
            window.add_child_view(Some(&mut chrome));
            for page in self.pages.borrow().iter() {
                if let Some(page_view) = page.view.as_ref() {
                    page_view.set_bounds(Some(&Rect::default()));
                    page_view.set_visible(0);
                    let mut view = View::from(page_view);
                    window.add_child_view(Some(&mut view));
                }
            }
            if self.pages.borrow().iter().any(|page| page.mode == PageRenderMode::Offscreen) {
                match NativeCompositor::new(window.window_handle(), self.commands.clone()) {
                    Ok(compositor) => UI_COMPOSITOR.with(|slot| *slot.borrow_mut() = Some(compositor)),
                    Err(error) => eprintln!("BrowserKit: osr_compositor_create_failed error={error}"),
                }
            }
            window.show();
            println!("BrowserKit: window_created");
        }
        fn on_window_bounds_changed(&self, window: Option<&mut Window>, new_bounds: Option<&Rect>) {
            let Some(window) = window else { return; };
            let bounds = new_bounds.cloned().unwrap_or_else(|| window.bounds());
            let chrome_bounds = Rect { x: 0, y: 0, width: bounds.width, height: bounds.height };
            self.chrome_view.set_bounds(Some(&chrome_bounds));
        }
        fn on_window_closing(&self, _window: Option<&mut Window>) {
            println!("BrowserKit: window_close_requested");
        }
        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            self.pages.borrow_mut().clear();
            UI_COMPOSITOR.with(|slot| *slot.borrow_mut() = None);
            println!("BrowserKit: window_destroyed");
        }
        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            println!("BrowserKit: window_close_requested");
            let mut can_close = true;
            for page in self.pages.borrow().iter() {
                if let Some(browser) = page
                    .browser
                    .clone()
                    .or_else(|| page.view.as_ref().and_then(BrowserView::browser))
                {
                    if let Some(host) = browser.host() { can_close &= host.try_close_browser() == 1; }
                }
            }
            can_close.into()
        }
        fn window_runtime_style(&self) -> RuntimeStyle { RuntimeStyle::ALLOY }
    }
}

struct BrowserClientState {
    window_id: browserkit_types::WindowId,
    page_id: Option<browserkit_types::PageId>,
    browser_id: Mutex<Option<i32>>,
    state: Arc<Mutex<browserkit_types::WindowState>>,
    emit: Arc<dyn Fn(browserkit_types::BrowserEvent) + Send + Sync>,
    remaining: Arc<AtomicUsize>,
    browser_router: Option<Arc<BrowserSideRouter>>,
    render_handler: Option<RenderHandler>,
}

fn update_page_state<F>(
    inner: &BrowserClientState,
    update: F,
) -> Option<browserkit_types::BrowserEvent>
where
    F: FnOnce(&mut browserkit_types::PageState) -> Option<browserkit_types::BrowserEvent>,
{
    let mut state = inner.state.lock().ok()?;
    let page_id = inner.page_id?;
    let page = state.pages.iter_mut().find(|page| page.id == page_id)?;
    update(page)
}
fn publish(inner: &BrowserClientState, event: browserkit_types::BrowserEvent) {
    (inner.emit)(event.clone());
    send_event(event);
}
wrap_client! {
    struct BrowserClient { inner: Arc<BrowserClientState> }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(BrowserLifeSpanHandler::new(self.inner.clone())) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(BrowserLoadHandler::new(self.inner.clone())) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(BrowserDisplayHandler::new(self.inner.clone())) }
        fn render_handler(&self) -> Option<RenderHandler> { self.inner.render_handler.clone() }
        fn on_process_message_received(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, source_process: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
            let Some(router) = &self.inner.browser_router else { return 0; };
            router.on_process_message_received(browser.cloned(), frame.cloned(), source_process, message.cloned()).into()
        }
    }
}

fn create_page_runtime(
    window_id: browserkit_types::WindowId,
    page: &browserkit_types::PageState,
    context: &UiContext,
) -> Option<(Client, PageView)> {
    let mode = PageRenderMode::from_environment();
    let surface = (mode == PageRenderMode::Offscreen)
        .then(|| Rc::new(RefCell::new(PageSurface::new(page.id))));
    let render_handler = surface
        .as_ref()
        .map(|surface| PageRenderCallbacks::new(page.id, surface.clone()));
    let mut client = BrowserClient::new(Arc::new(BrowserClientState {
        window_id,
        page_id: Some(page.id),
        browser_id: Mutex::new(None),
        state: context.state.clone(),
        emit: context.emit.clone(),
        remaining: context.remaining.clone(),
        browser_router: None,
        render_handler,
    }));
    let url = CefString::from(page.url.as_str());
    match mode {
        PageRenderMode::NativeView => {
            let mut delegate = BrowserViewCallbacks::new(Size {
                width: 1200,
                height: 800,
            });
            let view = browser_view_create(
                Some(&mut client),
                Some(&url),
                Some(&BrowserSettings::default()),
                None,
                None,
                Some(&mut delegate),
            )?;
            view.set_bounds(Some(&Rect::default()));
            view.set_visible(0);
            Some((
                client,
                PageView {
                    id: page.id,
                    mode,
                    view: Some(view),
                    browser: None,
                    surface: None,
                },
            ))
        }
        PageRenderMode::Offscreen => {
            let window_info = WindowInfo {
                windowless_rendering_enabled: 1,
                ..Default::default()
            };
            let browser_settings = BrowserSettings {
                windowless_frame_rate: 60,
                background_color: 0xffff_ffff,
                ..Default::default()
            };
            let browser = browser_host_create_browser_sync(
                Some(&window_info),
                Some(&mut client),
                Some(&url),
                Some(&browser_settings),
                None,
                None,
            )?;
            println!("BrowserKit: osr_browser_created page={:?}", page.id);
            Some((
                client,
                PageView {
                    id: page.id,
                    mode,
                    view: None,
                    browser: Some(browser),
                    surface,
                },
            ))
        }
    }
}

wrap_load_handler! {
    struct BrowserLoadHandler { inner: Arc<BrowserClientState> }
    impl LoadHandler {
        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, is_loading: i32, can_go_back: i32, can_go_forward: i32) {
            if self.inner.page_id.is_none() && is_loading == 1 && CHROME_READY.with(|ready| ready.get()) {
                println!("BrowserKit: frontend_reload");
            }
            if self.inner.page_id.is_none() && is_loading == 0 {
                println!("BrowserKit: frontend_loaded");
            }
            if let Some(event) = update_page_state(&self.inner, |page| {
                let changed = page.loading != (is_loading == 1);
                page.loading = is_loading == 1;
                changed.then_some(browserkit_types::BrowserEvent::PageLoadingChanged { page_id: self.inner.page_id?, loading: page.loading })
            }) { publish(&self.inner, event); }
            if let Some(event) = update_page_state(&self.inner, |page| {
                let changed = page.can_go_back != (can_go_back == 1) || page.can_go_forward != (can_go_forward == 1);
                page.can_go_back = can_go_back == 1;
                page.can_go_forward = can_go_forward == 1;
                changed.then_some(browserkit_types::BrowserEvent::PageNavigationStateChanged { page_id: self.inner.page_id?, can_go_back: page.can_go_back, can_go_forward: page.can_go_forward })
            }) { publish(&self.inner, event); }
        }
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, http_status_code: i32) {
            let Some(frame) = frame else { return; };
            if frame.is_main() == 1 {
                let url = CefString::from(&frame.url()).to_string();
                if self.inner.page_id.is_none() {
                    println!("BrowserKit: frontend_load_end status={http_status_code} url={url}");
                } else {
                    println!("BrowserKit: page_load_end status={http_status_code} url={url}");
                }
            }
        }
        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, error_code: Errorcode, error_text: Option<&CefString>, failed_url: Option<&CefString>) {
            let Some(frame) = frame else { return; };
            if frame.is_main() == 1 {
                let text = error_text.map(CefString::to_string).unwrap_or_default();
                let url = failed_url.map(CefString::to_string).unwrap_or_default();
                let kind = if self.inner.page_id.is_none() { "frontend" } else { "page" };
                println!("BrowserKit: {kind}_load_error code={} text={text:?} url={url}", error_code.get_raw());
            }
        }
    }
}
wrap_display_handler! {
    struct BrowserDisplayHandler { inner: Arc<BrowserClientState> }
    impl DisplayHandler {
        fn on_address_change(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, url: Option<&CefString>) {
            if frame.map(|frame| frame.is_main() == 1).unwrap_or(false) {
                let url = url.map(|url| url.to_string()).unwrap_or_default();
                if let Some(event) = update_page_state(&self.inner, |page| {
                    if page.url == url { None } else { page.url = url.clone(); Some(browserkit_types::BrowserEvent::PageUrlChanged { page_id: self.inner.page_id?, url: url.clone() }) }
                }) { publish(&self.inner, event); }
            }
        }
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = title.map(|title| title.to_string()).unwrap_or_default();
            if let Some(event) = update_page_state(&self.inner, |page| {
                if page.title == title { None } else { page.title = title.clone(); Some(browserkit_types::BrowserEvent::PageTitleChanged { page_id: self.inner.page_id?, title: title.clone() }) }
                }) { publish(&self.inner, event); }
        }
        fn on_console_message(&self, _browser: Option<&mut Browser>, level: LogSeverity, message: Option<&CefString>, source: Option<&CefString>, line: i32) -> i32 {
            if self.inner.page_id.is_none() {
                let message = message.map(CefString::to_string).unwrap_or_default();
                let source = source.map(CefString::to_string).unwrap_or_default();
                println!("BrowserKit: frontend_console level={} source={source:?} line={line} message={message:?}", level.get_raw());
            }
            0
        }
        fn on_cursor_change(
            &self,
            _browser: Option<&mut Browser>,
            _cursor: std::os::raw::c_ulong,
            type_: CursorType,
            _custom_cursor_info: Option<&CursorInfo>,
        ) -> std::os::raw::c_int {
            if self.inner.page_id.is_some() {
                UI_COMPOSITOR.with(|compositor| {
                    if let Some(compositor) = compositor.borrow_mut().as_mut() {
                        compositor.set_cursor(type_.get_raw());
                    }
                });
            }
            0
        }
    }
}
wrap_life_span_handler! {
    struct BrowserLifeSpanHandler { inner: Arc<BrowserClientState> }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                let id = browser.identifier();
                *self.inner.browser_id.lock().expect("browser id lock poisoned") = Some(id);
                if let Some(page_id) = self.inner.page_id { println!("BrowserKit: page_browser_created page={page_id:?} cef_browser_id={id}"); } else { CHROME_BROWSER.with(|slot| *slot.borrow_mut() = Some(browser.clone())); println!("BrowserKit: chrome_browser_created"); }
                if self.inner.page_id.is_none() { CHROME_READY.with(|ready| ready.set(false)); }
            }
        }
        fn do_close(&self, _browser: Option<&mut Browser>) -> i32 { if let Some(page_id) = self.inner.page_id { println!("BrowserKit: page_close_requested page={page_id:?}"); } 0 }
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            if let Some(router) = &self.inner.browser_router { router.on_before_close(_browser.cloned()); }
            let Some(page_id) = self.inner.page_id else { CHROME_BROWSER.with(|slot| *slot.borrow_mut() = None); CHROME_READY.with(|ready| ready.set(false)); if self.inner.remaining.fetch_sub(1, Ordering::AcqRel) == 1 { quit_message_loop(); } return; };
            println!("BrowserKit: page_closed page={page_id:?}");
            if debug_render_logs() {
                if let Some(surface) = page_surface(page_id) {
                    let metrics = &surface.borrow().metrics;
                    println!(
                        "BrowserKit: osr_metrics page={page_id:?} received={} presented={} dropped={} last_size={:?} paint={:?} upload={:?} present={:?}",
                        metrics.frames_received,
                        metrics.frames_presented,
                        metrics.frames_dropped,
                        metrics.last_frame_size,
                        metrics.paint_time,
                        metrics.upload_time,
                        metrics.present_time
                    );
                }
            }
            if page_is_active(page_id) {
                UI_COMPOSITOR.with(|compositor| {
                    if let Some(compositor) = compositor.borrow_mut().as_mut() {
                        compositor.set_visible(false);
                    }
                });
            }
            UI_PAGES.with(|pages| pages.borrow_mut().retain(|page| page.id != page_id));
            UI_VIEW_REGISTRATIONS.with(|registrations| {
                registrations.borrow_mut().remove(&page_id);
            });
            UI_VIEW_BOUNDS.with(|bounds| {
                bounds.borrow_mut().remove(&page_id);
            });
            UI_APPLIED_BOUNDS.with(|bounds| {
                bounds.borrow_mut().remove(&page_id);
            });
            UI_VIEW_VISIBILITY.with(|visibility| {
                visibility.borrow_mut().remove(&page_id);
            });
            let fallback = if let Ok(mut state) = self.inner.state.lock() {
                let Some(index) = state.pages.iter().position(|page| page.id == page_id) else { return; };
                let was_active = state.active_page_id == Some(page_id);
                state.pages.remove(index);
                if was_active {
                    state.active_page_id = state.pages.get(index).or_else(|| index.checked_sub(1).and_then(|i| state.pages.get(i))).map(|page| page.id);
                    let active = state.active_page_id;
                    for page in &mut state.pages { page.active = active == Some(page.id); }
                }
                state.active_page_id
            } else { None };
            let closed_event = browserkit_types::BrowserEvent::PageClosed { window_id: self.inner.window_id, page_id };
            (self.inner.emit)(closed_event.clone());
            send_event(closed_event);
            if let Some(page_id) = fallback {
                execute_command(PageCommand::Activate { window_id: self.inner.window_id, page_id });
                let activated_event = browserkit_types::BrowserEvent::PageActivated { window_id: self.inner.window_id, page_id };
                (self.inner.emit)(activated_event.clone());
                send_event(activated_event);
            }
            if self.inner.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                quit_message_loop();
            }
        }
    }
}

pub fn run(options: Options) -> Result<(), Error> {
    let _ = api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let Some(cef_dir) = cef::sys::get_cef_dir() else {
        return Err(Error::Resources);
    };
    println!("BrowserKit: CEF runtime {}", cef_dir.display());
    let args = Args::new();
    let Some(mut command_line) = args.as_cmd_line() else {
        return Err(Error::Arguments);
    };
    let compatibility = CompatibilityConfig::from_environment();
    log_compatibility(&compatibility);
    println!(
        "BrowserKit: page_renderer={:?}",
        PageRenderMode::from_environment()
    );
    initialize_threads();
    compatibility.apply(&mut command_line);
    // CEF launches secondary processes by re-executing this executable. Use the
    // OS argument vector for this decision as well as CEF's parsed command line:
    // on Linux, the wrapper can expose the CEF switch value later than the
    // native argv during early process startup. Most importantly, a process
    // with `--type=` must never continue into BrowserKit initialization.
    let type_switch = CefString::from("type");
    let cef_process_type =
        CefString::from(&command_line.switch_value(Some(&type_switch))).to_string();
    let argv_process_type = std::env::args().find_map(|argument| {
        argument
            .strip_prefix("--type=")
            .map(std::string::ToString::to_string)
    });
    let process_type = argv_process_type
        .or_else(|| (!cef_process_type.is_empty()).then_some(cef_process_type.clone()));
    let browser_process = process_type.is_none();
    println!(
        "BrowserKit: process dispatch type={} browser_process={browser_process}",
        process_type.as_deref().unwrap_or("browser")
    );
    let frontend = FrontendSource::from_environment()?;
    std::env::set_var("BROWSERKIT_FRONTEND_TOKEN", frontend.token());
    println!("BrowserKit: frontend_source={}", frontend.label());
    let remaining = Arc::new(AtomicUsize::new(options.pages.len() + 1));
    let no_sandbox = compatibility.no_sandbox;
    let browser_router = BrowserSideRouter::new(MessageRouterConfig::default());
    let renderer_router = RendererSideRouter::new(MessageRouterConfig::default());
    let handler = BrowserProcessCallbacks::new(
        options,
        RefCell::new(Vec::new()),
        remaining,
        browser_router,
        frontend.clone(),
    );
    let render_handler = RenderProcessCallbacks::new(renderer_router, frontend.clone());
    let mut app = Application::new(handler, render_handler, compatibility, frontend);
    let child_exit = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if !browser_process {
        println!(
            "BrowserKit: child process {} exited with code {child_exit}",
            process_type.as_deref().unwrap_or("unknown")
        );
        return if child_exit >= 0 {
            Ok(())
        } else {
            Err(Error::ChildProcess(child_exit))
        };
    }
    if child_exit != -1 {
        return Err(Error::ChildProcess(child_exit));
    }
    println!("BrowserKit starting");
    println!(
        "BrowserKit: CEF version {}",
        String::from_utf8_lossy(cef::sys::CEF_VERSION).trim_end_matches('\0')
    );
    println!("BrowserKit: process type browser");
    let cache_path = std::env::temp_dir()
        .join("browserkit")
        .join("cef-user-data");
    std::fs::create_dir_all(&cache_path).map_err(|e| Error::Runtime(e.to_string()))?;
    let settings = Settings {
        root_cache_path: CefString::from(cache_path.to_string_lossy().as_ref()),
        windowless_rendering_enabled: 1,
        // Keep Chromium's normal Linux multi-process sandbox enabled, as in
        // the upstream cefsimple sample. The browser process must not disable
        // this unless an explicit diagnostic build is introduced.
        no_sandbox: no_sandbox.into(),
        ..Default::default()
    };
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        return Err(Error::Initialization);
    }
    println!("BrowserKit: CEF initialized");
    run_message_loop();
    println!("BrowserKit: cef_shutdown");
    shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        frontend_asset, frontend_origin, hit_test, is_local_dev_origin, page_local_point,
        update_pointer_capture, CompatibilityConfig, FrontendSource, SurfaceTarget,
    };
    use browserkit_types::{LogicalRect, PageId};
    use std::path::{Path, PathBuf};

    #[test]
    fn parses_vulkan_only_mode() {
        let config = CompatibilityConfig::from_values(None, Some("1"), None, None);
        assert!(!config.disable_gpu);
        assert!(config.disable_vulkan);
        assert_eq!(config.applied_switches(), ["--disable-features=Vulkan"]);
    }

    #[test]
    fn parses_full_gpu_mode_separately() {
        let config = CompatibilityConfig::from_values(Some("1"), None, None, None);
        assert!(config.disable_gpu);
        assert!(!config.disable_vulkan);
        assert_eq!(
            config.applied_switches(),
            [
                "--disable-gpu",
                "--disable-gpu-compositing",
                "--disable-vulkan",
            ]
        );
    }

    #[test]
    fn parses_supported_ozone_platforms() {
        assert_eq!(
            CompatibilityConfig::from_values(None, None, None, Some("wayland")).ozone_platform,
            Some("wayland".into())
        );
        assert_eq!(
            CompatibilityConfig::from_values(None, None, None, Some("x11")).ozone_platform,
            Some("x11".into())
        );
    }

    #[test]
    fn ignores_invalid_ozone_platform_without_panicking() {
        let config = CompatibilityConfig::from_values(None, None, None, Some("mir"));
        assert_eq!(config.ozone_platform, None);
        assert!(config.applied_switches().is_empty());
    }

    #[test]
    fn allows_explicit_combination_of_diagnostics() {
        let config = CompatibilityConfig::from_values(Some("1"), Some("1"), None, Some("x11"));
        assert_eq!(
            config.applied_switches(),
            [
                "--disable-gpu",
                "--disable-gpu-compositing",
                "--disable-vulkan",
                "--disable-features=Vulkan",
                "--ozone-platform=x11",
            ]
        );
    }

    #[test]
    fn keeps_no_sandbox_as_an_explicit_diagnostic() {
        let config = CompatibilityConfig::from_values(None, None, Some("1"), None);
        assert!(config.no_sandbox);
        assert_eq!(config.applied_switches(), ["--no-sandbox"]);
    }

    #[test]
    fn restricts_dev_frontend_to_local_origins() {
        assert!(is_local_dev_origin("http://127.0.0.1:5173"));
        assert!(is_local_dev_origin("http://localhost:5173"));
        assert!(!is_local_dev_origin("http://127.0.0.1.evil:5173"));
        assert!(!is_local_dev_origin("http://127.0.0.1:5173.evil"));
        assert_eq!(
            frontend_origin("http://127.0.0.1:5173/app"),
            Some("http://127.0.0.1:5173".into())
        );
    }

    #[test]
    fn rejects_frontend_asset_traversal() {
        assert!(frontend_asset(Path::new("."), "browserkit://app/../Cargo.toml").is_none());
        assert!(frontend_asset(Path::new("."), "browserkit://app/%2e%2e/Cargo.toml").is_none());
    }

    #[test]
    fn frontend_bridge_requires_runtime_chrome_token() {
        let dev = FrontendSource::DevServer {
            url: "http://127.0.0.1:5173".into(),
            origin: "http://127.0.0.1:5173".into(),
            token: "secret".into(),
        };
        assert!(dev.is_trusted_url(&dev.entry_url()));
        assert!(!dev.is_trusted_url("http://127.0.0.1:5173/"));
        assert!(!dev.is_trusted_url("http://127.0.0.1:5173/?browserkit_token=wrong"));

        let production = FrontendSource::AssetsDirectory {
            root: PathBuf::from("."),
            token: "secret".into(),
        };
        assert!(production.is_trusted_url(&production.entry_url()));
        assert!(!production.is_trusted_url("browserkit://app/index.html"));
    }

    #[test]
    fn rounds_fractional_view_bounds_deterministically() {
        assert_eq!(super::rounded_i32(12.49), Some(12));
        assert_eq!(super::rounded_i32(12.5), Some(13));
        assert_eq!(super::rounded_i32(-12.5), Some(-13));
    }

    #[test]
    fn hit_tests_page_then_chrome() {
        let page = PageId::from_raw(7);
        let rect = LogicalRect {
            x: 238.0,
            y: 94.0,
            width: 924.0,
            height: 628.0,
        };
        assert_eq!(
            hit_test(rect, Some(page), 500.0, 300.0),
            SurfaceTarget::Page(page)
        );
        assert_eq!(
            hit_test(rect, Some(page), 100.0, 300.0),
            SurfaceTarget::Chrome
        );
        assert_eq!(hit_test(rect, None, 500.0, 300.0), SurfaceTarget::Chrome);
    }

    #[test]
    fn converts_window_pointer_to_page_local_coordinates() {
        let rect = LogicalRect {
            x: 238.0,
            y: 94.0,
            width: 924.0,
            height: 628.0,
        };
        assert_eq!(page_local_point(500.0, 300.0, rect), (262, 206));
    }

    #[test]
    fn pointer_capture_survives_drag_until_release() {
        let page = PageId::from_raw(3);
        let other = PageId::from_raw(4);
        let mut capture = None;
        update_pointer_capture(&mut capture, page, false);
        assert_eq!(capture, Some(page));
        update_pointer_capture(&mut capture, other, false);
        assert_eq!(capture, Some(page));
        update_pointer_capture(&mut capture, page, true);
        assert_eq!(capture, None);
        update_pointer_capture(&mut capture, other, true);
        assert_eq!(capture, None);
    }

    #[test]
    fn switches_one_visible_page_surface() {
        let first = PageId::from_raw(1);
        let second = PageId::from_raw(2);
        assert!(super::surface_is_active(Some(first), first));
        assert!(!super::surface_is_active(Some(first), second));
        assert!(super::surface_is_active(Some(second), second));
    }
}

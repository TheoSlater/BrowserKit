//! CEF Views integration. All View/Browser objects stay on CEF's UI thread.

use browserkit_types::protocol::ProtocolSink;
use cef::wrapper::message_router::{
    BrowserSideHandler, BrowserSideRouter, MessageRouterBrowserSide,
    MessageRouterBrowserSideHandlerCallbacks, MessageRouterConfig, MessageRouterRendererSide,
    MessageRouterRendererSideHandlerCallbacks, RendererSideRouter,
};
use cef::wrapper::stream_resource_handler::StreamResourceHandler;
use cef::{args::Args, *};
use std::{
    cell::RefCell,
    collections::VecDeque,
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
#[derive(Clone)]
struct UiContext {
    state: Arc<Mutex<browserkit_types::WindowState>>,
    emit: Arc<dyn Fn(browserkit_types::BrowserEvent) + Send + Sync>,
    remaining: Arc<AtomicUsize>,
}
thread_local! { static UI_CONTEXT: RefCell<Option<UiContext>> = const { RefCell::new(None) }; }
thread_local! { static CHROME_BROWSER: RefCell<Option<Browser>> = const { RefCell::new(None) }; }
thread_local! { static CHROME_READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

const CHROME_URL: &str = "browserkit://app/index.html";
pub type ProtocolHandler = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

const CHROME_HTML: &str = r#"<!doctype html><meta charset="utf-8"><title>BrowserKit</title>
<style>body{font:14px sans-serif;margin:8px}button,input,select{margin:2px}pre{white-space:pre-wrap}</style>
<button id="create">Create page</button><select id="pages"></select>
<button id="close">Close active</button><button id="back">Back</button><button id="forward">Forward</button>
<button id="reload">Reload</button><button id="stop">Stop</button>
<input id="url" value="https://example.com"><button id="navigate">Go</button><pre id="state"></pre>
<script>
(() => { const pending=new Map(); let serial=0, state={windows:[]};
window.__browserkit={send(message){ window.cefQuery({request:JSON.stringify(message),onSuccess:receive,onFailure:(_,e)=>console.error(e)}); },receive};
function request(request){const id='req-'+(++serial);return new Promise((resolve,reject)=>{pending.set(id,{resolve,reject});window.__browserkit.send({type:'request',id,request});setTimeout(()=>{if(pending.delete(id))reject(new Error('request timeout'));},10000);});}
function receive(raw){let message;try{message=JSON.parse(raw)}catch(e){return} if(message.type==='response'){const p=pending.get(message.id);if(!p)return;pending.delete(message.id);if(message.result.status==='ok')p.resolve(message.result.data);else p.reject(message.result.error)}else if(message.type==='event'){apply(message.event);render();}}
function active(){return state.windows[0]?.active_page_id} function page(id){return state.windows[0]?.pages.find(p=>p.id===id)}
function command(command){window.__browserkit.send({type:'command',command});}
function apply(e){const w=state.windows[0]; if(!w)return; if(e.type==='page_url_changed'){page(e.page_id).url=e.url} if(e.type==='page_title_changed'){page(e.page_id).title=e.title} if(e.type==='page_loading_changed'){page(e.page_id).loading=e.loading} if(e.type==='page_navigation_state_changed'){Object.assign(page(e.page_id),e)} if(e.type==='page_activated'){w.active_page_id=e.page_id;w.pages.forEach(p=>p.active=p.id===e.page_id)} if(e.type==='page_closed'){w.pages=w.pages.filter(p=>p.id!==e.page_id);if(w.active_page_id===e.page_id)w.active_page_id=w.pages.find(p=>p.active)?.id||null}}
function render(){document.querySelector('#state').textContent=JSON.stringify(state,null,2);const s=document.querySelector('#pages');s.replaceChildren(...(state.windows[0]?.pages||[]).map(p=>{const o=document.createElement('option');o.value=p.id;o.textContent=(p.title||p.url||'page')+' ['+p.id+']';o.selected=p.active;return o}));const p=page(active());document.querySelector('#back').disabled=!p?.can_go_back;document.querySelector('#forward').disabled=!p?.can_go_forward;document.querySelector('#stop').disabled=!p?.loading;document.querySelector('#url').value=p?.url||'';}
document.querySelector('#create').onclick=()=>request({type:'page_create',window_id:state.windows[0].id,options:{url:'https://example.org'}}).then(d=>{state.windows[0].pages.push(d.page);render()});
document.querySelector('#pages').onchange=e=>command({type:'page_activate',window_id:state.windows[0].id,page_id:+e.target.value});document.querySelector('#close').onclick=()=>command({type:'page_close',window_id:state.windows[0].id,page_id:active()});document.querySelector('#navigate').onclick=()=>command({type:'page_navigate',page_id:active(),url:document.querySelector('#url').value});document.querySelector('#back').onclick=()=>command({type:'page_go_back',page_id:active()});document.querySelector('#forward').onclick=()=>command({type:'page_go_forward',page_id:active()});document.querySelector('#reload').onclick=()=>command({type:'page_reload',page_id:active()});document.querySelector('#stop').onclick=()=>command({type:'page_stop',page_id:active()});
request({type:'runtime_handshake',protocol_version:1}).then(d=>{state.windows=d.windows;render()}).catch(console.error);
})();</script>"#;

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
    CHROME_BROWSER.with(|browser| {
        if let Some(browser) = browser.borrow().as_ref() {
            if let Some(frame) = browser.main_frame() {
                let script = format!(
                    "window.__browserkit && window.__browserkit.receive(JSON.parse({literal}));"
                );
                frame.execute_java_script(
                    Some(&CefString::from(script.as_str())),
                    Some(&CefString::from(CHROME_URL)),
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

struct CefProtocolSink;
impl browserkit_types::protocol::ProtocolSink for CefProtocolSink {
    fn send(&self, message: browserkit_types::protocol::NativeMessage) {
        send_to_chrome(message);
    }
}

fn execute_command(command: PageCommand) {
    UI_PAGES.with(|pages| {
        let pages = pages.borrow();
        let page_id = match &command {
            PageCommand::Create { .. } => return execute_create_page(command),
            PageCommand::Navigate { page_id, .. }
            | PageCommand::Reload { page_id }
            | PageCommand::GoBack { page_id }
            | PageCommand::GoForward { page_id }
            | PageCommand::Stop { page_id }
            | PageCommand::Close { page_id, .. } => *page_id,
            PageCommand::Activate { page_id, .. } => *page_id,
        };
        let Some(page) = pages.iter().find(|page| page.id == page_id) else {
            println!("BrowserKit: page_not_found page={page_id:?}");
            return;
        };
        let Some(browser) = page.view.browser() else {
            return;
        };
        match command {
            PageCommand::Create { .. } => unreachable!(),
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
            PageCommand::Activate { window_id, .. } => {
                for page in &*pages {
                    page.view.set_visible((page.id == page_id).into());
                }
                page.view.request_focus();
                send_event(browserkit_types::BrowserEvent::PageActivated { window_id, page_id });
            }
            PageCommand::Close { .. } => {
                if let Some(host) = browser.host() {
                    let _ = host.try_close_browser();
                }
            }
        }
        println!("BrowserKit: runtime_command_executed page={page_id:?}");
    });
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
    let mut client = BrowserClient::new(Arc::new(BrowserClientState {
        window_id,
        page_id: Some(page.id),
        browser_id: Mutex::new(None),
        state: context.state.clone(),
        emit: context.emit.clone(),
        remaining: context.remaining.clone(),
        browser_router: None,
    }));
    let mut delegate = BrowserViewCallbacks::new(Size {
        width: 1200,
        height: 800,
    });
    let url = CefString::from(page.url.as_str());
    let Some(view) = browser_view_create(
        Some(&mut client),
        Some(&url),
        Some(&BrowserSettings::default()),
        None,
        None,
        Some(&mut delegate),
    ) else {
        eprintln!("BrowserKit: page_view_create_failed page={:?}", page.id);
        UI_CLIENTS.with(|slot| *slot.borrow_mut() = clients);
        return;
    };
    context.remaining.fetch_add(1, Ordering::AcqRel);
    clients.push(client);
    UI_CLIENTS.with(|slot| *slot.borrow_mut() = clients);
    view.set_visible(page.active.into());
    let mut child = View::from(&view);
    UI_PAGES.with(|pages| {
        pages.borrow_mut().push(PageView {
            id: page.id,
            view: view.clone(),
        })
    });
    window.add_child_view(Some(&mut child));
    if let Some(layout) = window
        .get_layout()
        .and_then(|layout| layout.as_box_layout())
    {
        layout.set_flex_for_view(Some(&mut child), 1);
    }
    window.layout();
    if page.active {
        view.request_focus();
    }
    println!(
        "BrowserKit: page_view_created window={window_id:?} page={:?}",
        page.id
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
    }
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> { Some(self.browser_process.clone()) }
        fn render_process_handler(&self) -> Option<RenderProcessHandler> { Some(self.render_process.clone()) }
        fn on_register_custom_schemes(&self, registrar: Option<&mut SchemeRegistrar>) {
            if let Some(registrar) = registrar {
                let name = CefString::from("browserkit");
                registrar.add_custom_scheme(Some(&name), (SchemeOptions::STANDARD.get_raw() | SchemeOptions::SECURE.get_raw()) as i32);
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
    struct RenderProcessCallbacks { router: Arc<RendererSideRouter> }
    impl RenderProcessHandler {
        fn on_context_created(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, context: Option<&mut V8Context>) {
            let allowed = frame.as_ref().map(|frame| frame.is_main() == 1 && CefString::from(&frame.url()).to_string().starts_with("browserkit://app")).unwrap_or(false);
            if allowed { self.router.on_context_created(browser.cloned(), frame.cloned(), context.cloned()); }
        }
        fn on_context_released(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, context: Option<&mut V8Context>) {
            let allowed = frame.as_ref().map(|frame| frame.is_main() == 1 && CefString::from(&frame.url()).to_string().starts_with("browserkit://app")).unwrap_or(false);
            if allowed { self.router.on_context_released(browser.cloned(), frame.cloned(), context.cloned()); }
        }
        fn on_process_message_received(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, source_process: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
            let allowed = frame.as_ref().map(|frame| frame.is_main() == 1 && CefString::from(&frame.url()).to_string().starts_with("browserkit://app")).unwrap_or(false);
            if allowed { self.router.on_process_message_received(browser.cloned(), frame.cloned(), Some(source_process), message.cloned()).into() } else { 0 }
        }
    }
}

wrap_scheme_handler_factory! {
    struct FrontendSchemeFactory {}
    impl SchemeHandlerFactory {
        fn create(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _scheme_name: Option<&CefString>, request: Option<&mut Request>) -> Option<ResourceHandler> {
            let url = request.map(|request| CefString::from(&request.url()).to_string()).unwrap_or_default();
            println!("BrowserKit: frontend_resource_request url={url}");
            if !url.ends_with("/index.html") && !url.ends_with("/app/") { return None; }
            // Follow the ownership pattern used by cef-rs's ResourceManager:
            // the CEF stream reader owns/copies this temporary buffer.
            let mut bytes = CHROME_HTML.as_bytes().to_vec();
            let stream = stream_reader_create_for_data(bytes.as_mut_ptr(), bytes.len())?;
            Some(StreamResourceHandler::new_with_stream("text/html; charset=utf-8".into(), stream))
        }
    }
}

struct FrontendQueryHandler {
    handler: ProtocolHandler,
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
                    && CefString::from(&frame.url())
                        .to_string()
                        .starts_with("browserkit://app")
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
            CHROME_READY.with(|ready| ready.set(request.contains("runtime_handshake")));
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
    }
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            let options = &self.options;
            let mut factory = FrontendSchemeFactory::new();
            register_scheme_handler_factory(Some(&CefString::from("browserkit")), Some(&CefString::from("app")), Some(&mut factory));
            let _ = self.browser_router.add_handler(Arc::new(FrontendQueryHandler { handler: options.protocol_handler.clone() }), true);
            let mut chrome_client = BrowserClient::new(Arc::new(BrowserClientState {
                window_id: options.window_id,
                page_id: None,
                browser_id: Mutex::new(None),
                state: options.state.clone(),
                emit: options.emit.clone(),
                remaining: self.remaining.clone(),
                browser_router: Some(self.browser_router.clone()),
            }));
            let chrome_client_ref = chrome_client.clone();
            let mut chrome_delegate = BrowserViewCallbacks::new(Size { width: options.width as i32, height: 120 });
            let chrome_url = CefString::from(CHROME_URL);
            let Some(chrome_view) = browser_view_create(Some(&mut chrome_client), Some(&chrome_url), Some(&BrowserSettings::default()), None, None, Some(&mut chrome_delegate)) else {
                eprintln!("BrowserKit: chrome_view_create_failed"); quit_message_loop(); return;
            };
            println!("BrowserKit: chrome_view_created");
            self.clients.borrow_mut().push(chrome_client_ref);
            let mut pages = Vec::with_capacity(options.pages.len());
            for page in &options.pages {
                let client = BrowserClient::new(Arc::new(BrowserClientState {
                    window_id: options.window_id,
                    page_id: Some(page.id),
                    browser_id: Mutex::new(None),
                    state: options.state.clone(),
                    emit: options.emit.clone(),
                    remaining: self.remaining.clone(),
                    browser_router: None,
                }));
                let mut clients = self.clients.borrow_mut();
                clients.push(client);
                let client = clients.last_mut().expect("CEF client was just installed");
                let url = CefString::from(page.url.as_str());
                let mut delegate = BrowserViewCallbacks::new(Size { width: options.width as i32, height: options.height as i32 });
                let Some(view) = browser_view_create(Some(client), Some(&url), Some(&BrowserSettings::default()), None, None, Some(&mut delegate)) else {
                    eprintln!("BrowserKit: page_view_create_failed page={:?}", page.id);
                    quit_message_loop();
                    return;
                };
                println!("BrowserKit: page_view_created window={:?} page={:?}", options.window_id, page.id);
                pages.push(PageView { id: page.id, view });
            }
            println!("BrowserKit: window_create_requested window={:?}", options.window_id);
            let ui_pages = pages.clone();
            let mut delegate = BrowserKitWindow::new(
                RefCell::new(pages), chrome_view, options.title.clone(),
                Size { width: options.width as i32, height: options.height as i32 },
                options.pages.iter().find(|page| page.active).map(|page| page.id),
            );
            if window_create_top_level(Some(&mut delegate)).is_none() { quit_message_loop(); }
            UI_PAGES.with(|pages| *pages.borrow_mut() = ui_pages);
            UI_CLIENTS.with(|clients| *clients.borrow_mut() = self.clients.borrow().clone());
            UI_CONTEXT.with(|context| *context.borrow_mut() = Some(UiContext { state: options.state.clone(), emit: options.emit.clone(), remaining: self.remaining.clone() }));
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

#[derive(Clone)]
struct PageView {
    id: browserkit_types::PageId,
    view: BrowserView,
}

wrap_window_delegate! {
    struct BrowserKitWindow {
        pages: RefCell<Vec<PageView>>,
        chrome_view: BrowserView,
        title: String,
        size: Size,
        active_page_id: Option<browserkit_types::PageId>,
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
            let settings = BoxLayoutSettings {
                horizontal: 0,
                ..Default::default()
            };
            let Some(layout) = window.set_to_box_layout(Some(&settings)) else { return; };
            self.chrome_view.set_visible(1);
            let mut chrome = View::from(&self.chrome_view);
            window.add_child_view(Some(&mut chrome));
            for page in self.pages.borrow().iter() {
                page.view.set_visible((self.active_page_id == Some(page.id)).into());
                let mut view = View::from(&page.view);
                window.add_child_view(Some(&mut view));
            }
            layout.set_flex_for_view(Some(&mut chrome), 0);
            for page in self.pages.borrow().iter() {
                let mut view = View::from(&page.view);
                layout.set_flex_for_view(Some(&mut view), 1);
            }
            window.layout();
            window.show();
            if let Some(page) = self.pages.borrow().iter().find(|page| self.active_page_id == Some(page.id)) { page.view.request_focus(); }
            println!("BrowserKit: window_created");
        }
        fn on_window_closing(&self, _window: Option<&mut Window>) {
            println!("BrowserKit: window_close_requested");
        }
        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            self.pages.borrow_mut().clear();
            println!("BrowserKit: window_destroyed");
        }
        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            println!("BrowserKit: window_close_requested");
            let mut can_close = true;
            for page in self.pages.borrow().iter() {
                if let Some(browser) = page.view.browser() {
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
        fn on_process_message_received(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, source_process: ProcessId, message: Option<&mut ProcessMessage>) -> i32 {
            let Some(router) = &self.inner.browser_router else { return 0; };
            router.on_process_message_received(browser.cloned(), frame.cloned(), source_process, message.cloned()).into()
        }
    }
}
wrap_load_handler! {
    struct BrowserLoadHandler { inner: Arc<BrowserClientState> }
    impl LoadHandler {
        fn on_loading_state_change(&self, _browser: Option<&mut Browser>, is_loading: i32, can_go_back: i32, can_go_forward: i32) {
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
            UI_PAGES.with(|pages| pages.borrow_mut().retain(|page| page.id != page_id));
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
    let child_exit = execute_process(
        Some(args.as_main_args()),
        None::<&mut App>,
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
    let remaining = Arc::new(AtomicUsize::new(options.pages.len() + 1));
    let no_sandbox = compatibility.no_sandbox;
    let browser_router = BrowserSideRouter::new(MessageRouterConfig::default());
    let renderer_router = RendererSideRouter::new(MessageRouterConfig::default());
    let handler =
        BrowserProcessCallbacks::new(options, RefCell::new(Vec::new()), remaining, browser_router);
    let render_handler = RenderProcessCallbacks::new(renderer_router);
    let mut app = Application::new(handler, render_handler, compatibility);
    let settings = Settings {
        root_cache_path: CefString::from(cache_path.to_string_lossy().as_ref()),
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
    use super::CompatibilityConfig;

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
}

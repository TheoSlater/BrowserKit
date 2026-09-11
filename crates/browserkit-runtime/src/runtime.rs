pub struct BrowserKitRuntimeOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub window_id: browserkit_types::WindowId,
    pub pages: Vec<browserkit_types::PageState>,
    pub state: std::sync::Arc<std::sync::Mutex<browserkit_types::WindowState>>,
    pub commands: browserkit_cef::CommandQueue,
    pub protocol_handler: browserkit_cef::ProtocolHandler,
    pub emit: std::sync::Arc<dyn Fn(browserkit_types::BrowserEvent) + Send + Sync>,
}

pub struct BrowserKitRuntime;

impl BrowserKitRuntime {
    pub fn run(options: BrowserKitRuntimeOptions) -> Result<(), super::Error> {
        if options.width == 0 || options.height == 0 {
            return Err(super::Error::InvalidConfiguration(
                "window dimensions must be non-zero",
            ));
        }
        if options.title.trim().is_empty() {
            return Err(super::Error::InvalidConfiguration(
                "window title cannot be empty",
            ));
        }
        crate::cef::run(options)
    }
}

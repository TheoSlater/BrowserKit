//! Public high-level BrowserKit API.

use browserkit_runtime::Runtime;

pub use browserkit_types::{
    BrowserOptions, CoordinateSpace, Error, ErrorKind, LogicalRect, PageId, PageOptions,
    PhysicalRect, Result, ScaleContext, WebViewHostMode, WindowId, WindowOptions,
};

/// BrowserKit application.
pub struct Browser {
    runtime: Runtime,
}

/// Native window handle.
pub struct Window<'a> {
    browser: &'a mut Browser,
    id: WindowId,
}

/// Browser page handle.
pub struct Page<'a> {
    browser: &'a mut Browser,
    window_id: WindowId,
    id: PageId,
}

impl Browser {
    pub fn new(options: BrowserOptions) -> Result<Self> {
        Ok(Self {
            runtime: Runtime::new(options)?,
        })
    }
    pub fn create_window(&mut self, options: WindowOptions) -> Result<Window<'_>> {
        let id = self.runtime.create_window(options)?;
        Ok(Window { browser: self, id })
    }
    pub fn run(self) -> Result<()> {
        self.runtime.run()
    }
}

impl Window<'_> {
    pub fn create_page(&mut self, options: PageOptions) -> Result<Page<'_>> {
        let id = self.browser.runtime.create_page(self.id, options)?;
        Ok(Page {
            browser: self.browser,
            window_id: self.id,
            id,
        })
    }

    pub fn set_active_page(&mut self, page_id: PageId) -> Result<()> {
        self.browser.runtime.set_active_page(self.id, page_id)
    }

    pub fn close_page(&mut self, page_id: PageId) -> Result<()> {
        self.browser.runtime.close_page(self.id, page_id)
    }
}

impl Page<'_> {
    pub fn id(&self) -> PageId {
        self.id
    }
    pub fn navigate(&self, url: &str) -> Result<()> {
        self.browser.runtime.navigate(self.window_id, self.id, url)
    }
    pub fn go_back(&self) -> Result<()> {
        self.browser.runtime.go_back(self.window_id, self.id)
    }
    pub fn go_forward(&self) -> Result<()> {
        self.browser.runtime.go_forward(self.window_id, self.id)
    }
    pub fn reload(&self) -> Result<()> {
        self.browser.runtime.reload(self.window_id, self.id)
    }
    pub fn set_bounds(&mut self, bounds: LogicalRect) -> Result<()> {
        self.browser
            .runtime
            .set_bounds(self.window_id, self.id, bounds)
    }
    pub fn set_visible(&mut self, visible: bool) -> Result<()> {
        self.browser
            .runtime
            .set_visible(self.window_id, self.id, visible)
    }
    pub fn focus(&mut self) -> Result<()> {
        self.browser.runtime.focus(self.window_id, self.id)
    }
}

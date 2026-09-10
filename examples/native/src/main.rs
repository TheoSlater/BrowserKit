use browserkit::{Browser, BrowserOptions, LogicalRect, PageOptions, WindowOptions};

fn main() -> browserkit::Result<()> {
    let mut browser = Browser::new(BrowserOptions {})?;
    {
        let mut window = browser.create_window(WindowOptions {
            title: "BrowserKit M0".into(),
            ..Default::default()
        })?;
        let mut page = window.create_page(PageOptions {
            url: Some("https://example.com".into()),
            ..Default::default()
        })?;
        page.set_bounds(LogicalRect {
            x: 16.0,
            y: 56.0,
            width: 1168.0,
            height: 728.0,
        })?;
        page.set_visible(true)?;
        page.focus()?;
    }
    browser.run()
}

use browserkit::{Browser, BrowserOptions, PageOptions, WebViewHostMode, WindowOptions};

fn main() -> browserkit::Result<()> {
    let mut browser = Browser::new(BrowserOptions {})?;
    {
        let mut window = browser.create_window(WindowOptions {
            title: "BrowserKit M0-B2".into(),
            debug_native_overlay: false,
            frontend_url: std::env::var("BROWSERKIT_FRONTEND_URL").ok(),
            ..Default::default()
        })?;
        let page_a = window.create_page(PageOptions {
            url: Some("https://example.com".into()),
            host_mode: WebViewHostMode::Composition,
        })?;
        let page_a = page_a.id();
        let page_b = {
            let page_b = window.create_page(PageOptions {
                url: Some("https://www.iana.org/domains/reserved".into()),
                host_mode: WebViewHostMode::Composition,
            })?;
            page_b.id()
        };
        for _ in 0..100 {
            window.set_active_page(page_b)?;
            window.set_active_page(page_a)?;
        }
        window.set_active_page(page_a)?;
        window.close_page(page_b)?;
        let replacement = {
            let page = window.create_page(PageOptions {
                url: Some("https://www.iana.org/domains/reserved".into()),
                host_mode: WebViewHostMode::Composition,
            })?;
            page.id()
        };
        window.set_active_page(replacement)?;
        window.close_page(page_a)?;
    }
    browser.run()
}

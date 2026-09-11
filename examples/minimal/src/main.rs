use browserkit::{BrowserKit, PageOptions};

fn main() -> browserkit::Result<()> {
    let app = BrowserKit::builder()
        .title("BrowserKit")
        .size(1200, 800)
        .build()?;
    let window = app.create_window()?;
    let first = window.create_page(PageOptions::new("https://example.com"))?;
    let _second = window.create_page(PageOptions::new("https://example.org"))?;
    window.set_active_page(first.id())?;
    app.run()
}

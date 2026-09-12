import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { BrowserClient, CefBrowserTransport } from "@browserkit/core";
import { BrowserProvider, BrowserView, useActivePage, useBrowser, usePages, useWindowState } from "@browserkit/react";
import "./style.css";

const client = new BrowserClient(new CefBrowserTransport());

function App() {
  const browser = useBrowser();
  const window = useWindowState();
  const pages = usePages();
  const activePage = useActivePage();
  const [url, setUrl] = useState(activePage?.url ?? "");
  const [editingUrl, setEditingUrl] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [tallToolbar, setTallToolbar] = useState(false);
  const [paddedView, setPaddedView] = useState(true);

  useEffect(() => {
    if (!editingUrl) setUrl(activePage?.url ?? "");
  }, [activePage?.url, editingUrl]);

  const navigate = () => {
    if (!activePage || !url.trim()) return;
    browser.pages.navigate(activePage.id, url.trim());
    setEditingUrl(false);
  };

  return (
    <main className="browser-shell">
      {sidebarOpen && <aside className="sidebar">
        <strong className="brand">BrowserKit</strong>
        <span className="sidebar-label">Pages</span>
        {pages.map((page) => (
          <button
            className={page.active ? "page-choice active" : "page-choice"}
            key={page.id}
            onClick={() => window && browser.pages.activate(window.id, page.id)}
          >
            {page.title || page.url || "New Tab"}
          </button>
        ))}
        {window && <button className="page-choice add" onClick={() => void browser.pages.create(window.id, { url: "https://example.org" })}>+ New page</button>}
        <div className="layout-controls">
          <button onClick={() => setTallToolbar((value) => !value)}>Toggle toolbar height</button>
          <button onClick={() => setPaddedView((value) => !value)}>Toggle page padding</button>
        </div>
      </aside>}
      <section className="browser-main">
        <nav className="tabs" aria-label="Pages">
          <button onClick={() => setSidebarOpen((value) => !value)}>☰</button>
          <span className="layout-note">DOM-driven page layout</span>
        </nav>
        <section className={tallToolbar ? "toolbar tall" : "toolbar"} aria-label="Browser controls">
          <button disabled={!activePage?.can_go_back} onClick={() => activePage && browser.pages.goBack(activePage.id)}>Back</button>
          <button disabled={!activePage?.can_go_forward} onClick={() => activePage && browser.pages.goForward(activePage.id)}>Forward</button>
          <button disabled={!activePage} onClick={() => activePage && browser.pages.reload(activePage.id)}>Reload</button>
          <button disabled={!activePage?.loading} onClick={() => activePage && browser.pages.stop(activePage.id)}>Stop</button>
          <input
            aria-label="URL"
            value={url}
            onChange={(event) => { setEditingUrl(true); setUrl(event.target.value); }}
            onKeyDown={(event) => { if (event.key === "Enter") navigate(); }}
            onFocus={() => setEditingUrl(true)}
            onBlur={() => setEditingUrl(false)}
          />
          <button disabled={!activePage} onClick={navigate}>Go</button>
          <button disabled={!activePage || !window} onClick={() => activePage && window && browser.pages.close(window.id, activePage.id)}>Close</button>
        </section>
        <BrowserView className={paddedView ? "browser-view padded" : "browser-view"} page={activePage} aria-label="Browser page region" />
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserProvider client={client}><App /></BrowserProvider>
  </StrictMode>,
);

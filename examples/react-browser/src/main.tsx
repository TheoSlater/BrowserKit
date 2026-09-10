import { StrictMode, useState, type CSSProperties } from "react";
import { createRoot } from "react-dom/client";
import { BrowserClient, createBrowserTransport } from "@browserkit/core";
import { BrowserProvider, BrowserView, useActivePage, useBrowser, useWindowState } from "@browserkit/react";
import "./style.css";

function App() {
  const browser = useBrowser();
  const page = useActivePage();
  const windowState = useWindowState();
  const [sidebar, setSidebar] = useState(true);
  const [padding, setPadding] = useState(16);
  const [toolbarHeight, setToolbarHeight] = useState(56);

  return <div className="app" style={{ "--toolbar-height": `${toolbarHeight}px` } as CSSProperties}>
    <header className="toolbar">
      <button disabled={!page?.canGoBack} onClick={() => page && browser.pages.goBack(page.pageId)}>Back</button>
      <button disabled={!page?.canGoForward} onClick={() => page && browser.pages.goForward(page.pageId)}>Forward</button>
      <button disabled={!page} onClick={() => page && browser.pages.reload(page.pageId)}>Reload</button>
      <input value={page?.url ?? ""} onChange={(event) => page && browser.pages.navigate(page.pageId, event.target.value)} />
      <button onClick={() => void browser.pages.create("https://example.com").then((id) => browser.pages.activate(id))}>New Page</button>
      <button onClick={() => setSidebar((value) => !value)}>Sidebar</button>
      <button onClick={() => setToolbarHeight((value) => value === 56 ? 80 : 56)}>Toolbar</button>
      <button onClick={() => setPadding((value) => value === 16 ? 32 : 16)}>Padding</button>
    </header>
    <aside className={sidebar ? "sidebar open" : "sidebar"}>
      {windowState.pages.map((item) => <button key={item.pageId} onClick={() => browser.pages.activate(item.pageId)}>
        {item.title || item.url || `Page ${item.pageId}`}
      </button>)}
    </aside>
    <main className="content" style={{ padding }}>
      <BrowserView page={page} className="browser-view" />
    </main>
  </div>;
}

const client = new BrowserClient(createBrowserTransport());
createRoot(document.getElementById("root")!).render(<StrictMode><BrowserProvider client={client}><App /></BrowserProvider></StrictMode>);

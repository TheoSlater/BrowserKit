import type {
  Command,
  FrontendMessage,
  NativeEvent,
  NativeMessage,
  PageId,
  PageState,
  Request,
  ResponseData,
  WindowState,
} from "./protocol";
import type { BrowserTransport } from "./types";

export function createBrowserTransport(): BrowserTransport {
  const listeners = new Set<(message: NativeMessage) => void>();
  const bridge: NonNullable<Window["__browserkit"]> = window.__browserkit ?? { receive: () => {} };
  bridge.receive = (message: NativeMessage) => listeners.forEach((listener) => listener(message));
  window.__browserkit = bridge;
  return {
    send(message) {
      window.ipc.postMessage(JSON.stringify(message));
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export class BrowserClient {
  private readonly pagesById = new Map<PageId, PageState>();
  private readonly listeners = new Set<(event?: NativeEvent) => void>();
  private readonly pending = new Map<string, {
    resolve: (data: ResponseData | undefined) => void;
    reject: (error: Error) => void;
  }>();
  private activePageId: PageId | null = null;
  private sequence = 0;
  private windowState: WindowState = { activePageId: null, pages: [] };

  constructor(private readonly transport: BrowserTransport) {
    transport.subscribe((message) => this.receive(message));
    void this.getWindowState();
  }

  pages = {
    create: async (url?: string) => {
      const data = await this.request({ type: "page.create", url });
      if (data?.type !== "page.created") throw new Error("BrowserKit did not return a page ID");
      return data.pageId;
    },
    activate: (pageId: PageId) => this.command({ type: "page.activate", pageId }),
    close: (pageId: PageId) => this.command({ type: "page.close", pageId }),
    navigate: (pageId: PageId | undefined, url: string) => this.command({ type: "page.navigate", pageId, url }),
    reload: (pageId?: PageId) => this.command({ type: "page.reload", pageId }),
    goBack: (pageId?: PageId) => this.command({ type: "page.go_back", pageId }),
    goForward: (pageId?: PageId) => this.command({ type: "page.go_forward", pageId }),
    setVisible: (pageId: PageId, visible: boolean) => this.command({ type: "page.set_visible", pageId, visible }),
    setViewBounds: (pageId: PageId, rect: import("./protocol").LogicalRect) => this.command({
      type: "page.set_view_bounds", pageId, rect, coordinateSpace: "frontend_logical",
      devicePixelRatio: window.devicePixelRatio, visualViewportScale: window.visualViewport?.scale ?? 1,
    }),
  };

  command(command: Command): void {
    this.transport.send({ type: "command", command });
  }

  request(request: Request): Promise<ResponseData | undefined> {
    const id = `req-${++this.sequence}`;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.transport.send({ type: "request", id, request });
      window.setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error("BrowserKit request timed out"));
      }, 10_000);
    });
  }

  getWindowState(): Promise<ResponseData | undefined> { return this.request({ type: "window.get_state" }); }
  getWindowSnapshot(): WindowState { return this.windowState; }
  getActivePage(): PageState | undefined { return this.activePageId == null ? undefined : this.pagesById.get(this.activePageId); }
  getPages(): PageState[] { return [...this.pagesById.values()]; }
  subscribe(listener: (event?: NativeEvent) => void): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }

  private receive(message: NativeMessage): void {
    if (message.type === "response") {
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.ok && message.data?.type === "window.state") {
        this.applyWindowState(message.data.state);
        this.listeners.forEach((listener) => listener());
      }
      if (message.ok) pending.resolve(message.data);
      else pending.reject(new Error(`${message.error?.code ?? "request_failed"}: ${message.error?.message ?? "request failed"}`));
      return;
    }
    const event = message.event;
    if (event.type === "page.created" || event.type === "page.activated") this.updatePage(event.state);
    else if (event.type === "page.closed") {
      this.pagesById.delete(event.pageId);
      if (this.activePageId === event.pageId) this.activePageId = null;
    } else {
      const page = this.pagesById.get(event.pageId);
      if (page) {
        if (event.type === "page.url_changed") page.url = event.url;
        if (event.type === "page.title_changed") page.title = event.title;
        if (event.type === "page.loading_changed") page.loading = event.loading;
        if (event.type === "page.navigation_state_changed") {
          page.canGoBack = event.canGoBack;
          page.canGoForward = event.canGoForward;
        }
      }
    }
    this.windowState = { activePageId: this.activePageId, pages: this.getPages() };
    this.listeners.forEach((listener) => listener(event));
  }

  private updatePage(page: PageState): void {
    this.pagesById.set(page.pageId, page);
    if (page.active) this.activePageId = page.pageId;
  }

  private applyWindowState(state: WindowState): void {
    this.pagesById.clear();
    state.pages.forEach((page) => this.pagesById.set(page.pageId, page));
    this.activePageId = state.activePageId;
    this.windowState = { activePageId: this.activePageId, pages: this.getPages() };
  }
}

declare global {
  interface Window {
    ipc: { postMessage(message: string): void };
    __browserkit?: { receive(message: NativeMessage): void };
  }
}

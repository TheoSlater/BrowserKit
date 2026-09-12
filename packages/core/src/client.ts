import { BrowserKitProtocolError, BrowserKitRequestError } from "./errors";
import { BrowserStateStore, type BrowserState, type StateListener } from "./state";
import type {
  Command, FrontendMessage, NativeMessage, PageId, PageOptions, PageState, Request, ResponseData,
  PageViewBoundsInput, WindowId, WindowState,
} from "./protocol";
import { PROTOCOL_VERSION } from "./protocol";
import type { BrowserTransport } from "./transport";

export interface BrowserClientOptions {
  requestTimeoutMs?: number;
}

export class BrowserClient {
  readonly pages: {
    create: (windowId: WindowId, options: PageOptions) => Promise<PageState>;
    navigate: (pageId: PageId, url: string) => void;
    reload: (pageId: PageId) => void;
    goBack: (pageId: PageId) => void;
    goForward: (pageId: PageId) => void;
    stop: (pageId: PageId) => void;
    activate: (windowId: WindowId, pageId: PageId) => void;
    close: (windowId: WindowId, pageId: PageId) => void;
    registerView: (pageId: PageId) => void;
    setViewBounds: (pageId: PageId, bounds: PageViewBoundsInput) => void;
    unregisterView: (pageId: PageId) => void;
  };

  private readonly store = new BrowserStateStore();
  private readonly timeoutMs: number;
  private readonly pending = new Map<string, { request: Request; resolve: (data: ResponseData) => void; reject: (error: BrowserKitRequestError) => void; timer: ReturnType<typeof setTimeout> }>();
  private unsubscribeTransport?: () => void;
  private connectPromise?: Promise<void>;
  private connected = false;
  private serial = 0;
  private readonly connectionListeners = new Set<() => void>();
  private readonly viewBounds = new Map<PageId, PageViewBoundsInput["rect"]>();

  constructor(private readonly transport: BrowserTransport, options: BrowserClientOptions = {}) {
    this.timeoutMs = options.requestTimeoutMs ?? 10_000;
    this.pages = {
      create: (windowId, pageOptions) => this.createPage(windowId, pageOptions),
      navigate: (pageId, url) => this.command({ type: "page_navigate", page_id: pageId, url }),
      reload: (pageId) => this.command({ type: "page_reload", page_id: pageId }),
      goBack: (pageId) => this.command({ type: "page_go_back", page_id: pageId }),
      goForward: (pageId) => this.command({ type: "page_go_forward", page_id: pageId }),
      stop: (pageId) => this.command({ type: "page_stop", page_id: pageId }),
      activate: (windowId, pageId) => this.command({ type: "page_activate", window_id: windowId, page_id: pageId }),
      close: (windowId, pageId) => this.command({ type: "page_close", window_id: windowId, page_id: pageId }),
      registerView: (pageId) => this.command({ type: "page_register_view", page_id: pageId }),
      setViewBounds: (pageId, bounds) => this.setViewBounds(pageId, bounds),
      unregisterView: (pageId) => {
        this.viewBounds.delete(pageId);
        this.command({ type: "page_unregister_view", page_id: pageId });
      },
    };
  }

  async connect(): Promise<void> {
    if (this.connected) return;
    if (this.connectPromise) return this.connectPromise;
    this.connectPromise = this.startConnect().then(
      () => { this.connectPromise = undefined; },
      (error) => { this.connectPromise = undefined; throw error; },
    );
    return this.connectPromise;
  }

  disconnect(): void {
    this.connected = false;
    this.viewBounds.clear();
    for (const listener of this.connectionListeners) listener();
    this.unsubscribeTransport?.();
    this.unsubscribeTransport = undefined;
    for (const [requestId, pending] of this.pending) {
      clearTimeout(pending.timer);
      pending.reject(new BrowserKitRequestError({ code: "transport", message: "BrowserClient disconnected" }, requestId, pending.request.type));
    }
    this.pending.clear();
    this.transport.disconnect?.();
  }

  getState(): BrowserState { return this.store.getState(); }
  subscribe(listener: StateListener): () => void { return this.store.subscribe(listener); }
  isConnected(): boolean { return this.connected; }
  subscribeConnection(listener: () => void): () => void { this.connectionListeners.add(listener); return () => this.connectionListeners.delete(listener); }
  getWindowState(windowId?: WindowId): WindowState | undefined { return this.store.getState().windows.find((window) => window.id === windowId) ?? this.store.getState().windows[0]; }
  getPageState(pageId: PageId): PageState | undefined { return this.store.getState().windows.flatMap((window) => window.pages).find((page) => page.id === pageId); }

  async getWindowStateRequest(windowId: WindowId): Promise<WindowState> {
    const data = await this.request({ type: "window_get_state", window_id: windowId });
    if (data.type !== "window_state") throw new BrowserKitProtocolError("unexpected window state response");
    return data.window;
  }

  async getPageStateRequest(pageId: PageId): Promise<PageState> {
    const data = await this.request({ type: "page_get_state", page_id: pageId });
    if (data.type !== "page_state") throw new BrowserKitProtocolError("unexpected page state response");
    return data.page;
  }

  private async startConnect(): Promise<void> {
    await this.transport.connect?.();
    this.unsubscribeTransport = this.transport.subscribe((message) => this.receive(message));
    try {
      const data = await this.request({ type: "runtime_handshake", protocol_version: PROTOCOL_VERSION });
      if (data.type !== "runtime_handshake") throw new BrowserKitProtocolError("unexpected handshake response");
      if (data.protocol_version !== PROTOCOL_VERSION) throw new BrowserKitProtocolError(`protocol mismatch: frontend=${PROTOCOL_VERSION} runtime=${data.protocol_version}`);
      this.store.hydrate(data.windows);
      this.connected = true;
      for (const listener of this.connectionListeners) listener();
    } catch (error) {
      this.unsubscribeTransport?.();
      this.unsubscribeTransport = undefined;
      this.transport.disconnect?.();
      throw error;
    }
  }

  private receive(message: NativeMessage): void {
    if (message.type === "event") {
      this.store.apply(message.event);
      if (message.event.type === "page_closed") this.viewBounds.delete(message.event.page_id);
      return;
    }
    const pending = this.pending.get(message.id);
    if (!pending) return;
    this.pending.delete(message.id);
    clearTimeout(pending.timer);
    if (message.result.status === "ok") pending.resolve(message.result.data);
    else pending.reject(new BrowserKitRequestError(message.result.error, message.id, pending.request.type));
  }

  private command(command: Command): void {
    if (!this.connected) throw new Error("BrowserClient is not connected");
    this.transport.send({ type: "command", command });
  }

  private setViewBounds(pageId: PageId, bounds: PageViewBoundsInput): void {
    if (!this.connected) throw new Error("BrowserClient is not connected");
    const previous = this.viewBounds.get(pageId);
    const rect = bounds.rect;
    if (previous && Math.abs(previous.x - rect.x) < 0.01 && Math.abs(previous.y - rect.y) < 0.01 && Math.abs(previous.width - rect.width) < 0.01 && Math.abs(previous.height - rect.height) < 0.01) {
      console.debug("BrowserKit: browser_view_bounds_deduplicated", { pageId, rect });
      return;
    }
    this.viewBounds.set(pageId, { ...rect });
    console.debug("BrowserKit: browser_view_bounds_sent", { pageId, rect });
    this.transport.send({
      type: "command",
      command: {
        type: "page_set_view_bounds",
        page_id: pageId,
        rect,
        coordinate_space: bounds.coordinateSpace,
        device_pixel_ratio: bounds.devicePixelRatio,
        visual_viewport_scale: bounds.visualViewportScale,
      },
    });
  }

  private async createPage(windowId: WindowId, options: PageOptions): Promise<PageState> {
    const data = await this.request({ type: "page_create", window_id: windowId, options });
    if (data.type !== "page_created") throw new BrowserKitProtocolError("unexpected page create response");
    this.store.addPage(data.page);
    return data.page;
  }

  private request(request: Request): Promise<ResponseData> {
    if (!this.transport) return Promise.reject(new Error("BrowserTransport missing"));
    const id = `req-${++this.serial}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new BrowserKitRequestError({ code: "timeout", message: `request timed out after ${this.timeoutMs}ms` }, id, request.type));
      }, this.timeoutMs);
      this.pending.set(id, { request, resolve, reject, timer });
      try {
        this.transport.send({ type: "request", id, request });
      } catch (error) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(new BrowserKitRequestError({ code: "transport", message: error instanceof Error ? error.message : String(error) }, id, request.type));
      }
    });
  }
}

export function createBrowserClient(transport: BrowserTransport, options?: BrowserClientOptions): BrowserClient {
  return new BrowserClient(transport, options);
}

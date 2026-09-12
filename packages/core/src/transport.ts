import { parseNativeMessage, type FrontendMessage, type NativeMessage } from "./protocol";

export interface BrowserTransport {
  send(message: FrontendMessage): void;
  subscribe(listener: (message: NativeMessage) => void): () => void;
  connect?(): Promise<void>;
  disconnect?(): void;
}

interface CefQueryOptions {
  request: string;
  onSuccess: (response: string) => void;
  onFailure: (errorCode: number, errorMessage: string) => void;
}

interface BrowserKitBridge {
  send(message: FrontendMessage): void;
  receive(raw: string): void;
  onMessage?: (raw: string) => void;
}

interface CefWindow extends Window {
  cefQuery?: (options: CefQueryOptions) => void;
  __browserkit?: BrowserKitBridge;
}

export class CefBrowserTransport implements BrowserTransport {
  private readonly scope: CefWindow;
  private readonly listeners = new Set<(message: NativeMessage) => void>();
  private connected = false;

  constructor(scope: CefWindow = window) {
    this.scope = scope;
  }

  async connect(): Promise<void> {
    if (this.connected) return;
    if (typeof this.scope.cefQuery !== "function") {
      throw new Error("CEF transport unavailable: cefQuery is missing");
    }
    const bridge: BrowserKitBridge = this.scope.__browserkit ?? {
      send: (message) => {
        this.scope.cefQuery?.({
          request: JSON.stringify(message),
          onSuccess: (raw) => bridge.receive(raw),
          onFailure: (code, errorMessage) => bridge.receive(JSON.stringify({
            type: "response",
            id: message.type === "request" ? message.id : "",
            result: { status: "err", error: { code: "request_failed", message: `${code}: ${errorMessage}` } },
          })),
        });
      },
      receive: () => undefined,
    };
    bridge.onMessage = (raw) => {
      let value: unknown;
      try {
        value = JSON.parse(raw);
      } catch {
        return;
      }
      try {
        const message = parseNativeMessage(value);
        for (const listener of this.listeners) listener(message);
      } catch {
        // Malformed native messages are ignored at this trust boundary.
      }
    };
    bridge.receive = (raw) => bridge.onMessage?.(raw);
    this.scope.__browserkit = bridge;
    this.connected = true;
  }

  send(message: FrontendMessage): void {
    if (!this.connected || !this.scope.__browserkit) {
      throw new Error("CEF transport is not connected");
    }
    this.scope.__browserkit.send(message);
  }

  subscribe(listener: (message: NativeMessage) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  disconnect(): void {
    this.connected = false;
    if (this.scope.__browserkit) this.scope.__browserkit.onMessage = undefined;
  }
}

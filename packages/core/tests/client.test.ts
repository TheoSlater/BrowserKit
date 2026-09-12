import { describe, expect, it, vi } from "vitest";
import { BrowserClient, BrowserKitProtocolError, BrowserKitRequestError } from "../src/index";
import type { BrowserTransport } from "../src/index";
import type { FrontendMessage, NativeMessage } from "../src/protocol";

class MockTransport implements BrowserTransport {
  sent: FrontendMessage[] = [];
  listeners = new Set<(message: NativeMessage) => void>();
  async connect() {}
  send(message: FrontendMessage): void { this.sent.push(message); }
  subscribe(listener: (message: NativeMessage) => void): () => void { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  receive(message: NativeMessage): void { for (const listener of this.listeners) listener(message); }
}

const handshake = (transport: MockTransport, windows = [{ id: 1, active_page_id: 1, pages: [{ id: 1, url: "https://example.com", title: "", loading: false, can_go_back: false, can_go_forward: false, active: true }] }]) => {
  const message = transport.sent.at(-1);
  if (!message || message.type !== "request") throw new Error("handshake not sent");
  transport.receive({ type: "response", id: message.id, result: { status: "ok", data: { type: "runtime_handshake", protocol_version: 1, windows } } });
};

describe("BrowserClient", () => {
  it("handshakes and hydrates authoritative state", async () => {
    const transport = new MockTransport();
    const client = new BrowserClient(transport);
    const connected = client.connect();
    await Promise.resolve();
    handshake(transport);
    await connected;
    expect(client.getWindowState()?.pages[0]?.url).toBe("https://example.com");
  });

  it("rejects a mismatched handshake version", async () => {
    const transport = new MockTransport();
    const client = new BrowserClient(transport);
    const connected = client.connect();
    await Promise.resolve();
    const request = transport.sent.at(-1);
    if (!request || request.type !== "request") throw new Error("handshake not sent");
    transport.receive({ type: "response", id: request.id, result: { status: "ok", data: { type: "runtime_handshake", protocol_version: 99, windows: [] } } });
    await expect(connected).rejects.toBeInstanceOf(BrowserKitProtocolError);
    expect(client.getState().windows).toEqual([]);
  });

  it("correlates requests and reconciles events", async () => {
    const transport = new MockTransport();
    const client = new BrowserClient(transport);
    const connected = client.connect();
    await Promise.resolve();
    handshake(transport);
    await connected;
    transport.receive({ type: "event", event: { type: "page_title_changed", page_id: 1, title: "Example" } });
    transport.receive({ type: "event", event: { type: "page_url_changed", page_id: 1, url: "https://example.test" } });
    transport.receive({ type: "event", event: { type: "page_loading_changed", page_id: 1, loading: true } });
    transport.receive({ type: "event", event: { type: "page_navigation_state_changed", page_id: 1, can_go_back: true, can_go_forward: true } });
    transport.receive({ type: "event", event: { type: "page_activated", window_id: 1, page_id: 1 } });
    expect(client.getPageState(1)?.title).toBe("Example");
    expect(client.getPageState(1)).toMatchObject({ url: "https://example.test", loading: true, can_go_back: true, can_go_forward: true, active: true });
    transport.receive({ type: "event", event: { type: "page_closed", window_id: 1, page_id: 1 } });
    expect(client.getPageState(1)).toBeUndefined();
  });

  it("correlates page creation and updates state", async () => {
    const transport = new MockTransport();
    const client = new BrowserClient(transport);
    const connected = client.connect();
    await Promise.resolve();
    handshake(transport);
    await connected;
    const created = client.pages.create(1, { url: "https://example.org" });
    const request = transport.sent.at(-1);
    if (!request || request.type !== "request") throw new Error("page create request missing");
    transport.receive({ type: "response", id: request.id, result: { status: "ok", data: { type: "page_created", page: { id: 2, url: "https://example.org", title: "", loading: false, can_go_back: false, can_go_forward: false, active: false } } } });
    await expect(created).resolves.toMatchObject({ id: 2 });
    expect(client.getPageState(2)?.url).toBe("https://example.org");
  });

  it("rejects structured errors and times out", async () => {
    vi.useFakeTimers();
    const transport = new MockTransport();
    const client = new BrowserClient(transport, { requestTimeoutMs: 20 });
    const connected = client.connect();
    await Promise.resolve();
    const request = transport.sent.at(-1);
    if (!request || request.type !== "request") throw new Error("request missing");
    transport.receive({ type: "response", id: request.id, result: { status: "err", error: { code: "protocol_version_mismatch", message: "no" } } });
    await expect(connected).rejects.toMatchObject({ code: "protocol_version_mismatch", requestType: "runtime_handshake" });
    vi.useRealTimers();
  });

  it("fails a missing response without leaking pending work", async () => {
    vi.useFakeTimers();
    const transport = new MockTransport();
    const client = new BrowserClient(transport, { requestTimeoutMs: 20 });
    const promise = client.connect();
    await Promise.resolve();
    vi.advanceTimersByTime(21);
    await expect(promise).rejects.toBeInstanceOf(BrowserKitRequestError);
    vi.useRealTimers();
  });

  it("rehydrates on reconnect", async () => {
    const transport = new MockTransport();
    const client = new BrowserClient(transport);
    let connected = client.connect();
    await Promise.resolve();
    handshake(transport);
    await connected;
    client.disconnect();
    connected = client.connect();
    await Promise.resolve();
    handshake(transport, []);
    await connected;
    expect(client.getState().windows).toEqual([]);
  });

  it("serializes logical view bounds and deduplicates them", async () => {
    const transport = new MockTransport();
    const client = new BrowserClient(transport);
    const connected = client.connect();
    await Promise.resolve();
    handshake(transport);
    await connected;

    client.pages.registerView(1);
    client.pages.setViewBounds(1, {
      rect: { x: 12.5, y: 24, width: 640.25, height: 480 },
      coordinateSpace: "frontend_logical",
      devicePixelRatio: 2,
      visualViewportScale: 1,
    });
    const sent = transport.sent.at(-1);
    expect(sent).toEqual({
      type: "command",
      command: {
        type: "page_set_view_bounds",
        page_id: 1,
        rect: { x: 12.5, y: 24, width: 640.25, height: 480 },
        coordinate_space: "frontend_logical",
        device_pixel_ratio: 2,
        visual_viewport_scale: 1,
      },
    });
    const count = transport.sent.length;
    client.pages.setViewBounds(1, {
      rect: { x: 12.501, y: 24, width: 640.25, height: 480 },
      coordinateSpace: "frontend_logical",
      devicePixelRatio: 2,
      visualViewportScale: 1,
    });
    expect(transport.sent).toHaveLength(count);
    client.pages.unregisterView(1);
    expect(transport.sent.at(-1)).toEqual({ type: "command", command: { type: "page_unregister_view", page_id: 1 } });
  });
});

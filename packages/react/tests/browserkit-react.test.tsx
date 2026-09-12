import { act, cleanup, render, screen } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BrowserClient } from "@browserkit/core";
import type { BrowserTransport, FrontendMessage, NativeMessage, WindowState } from "@browserkit/core";
import { BrowserProvider, BrowserView, useActivePage, useBrowser } from "../src";

class MockTransport implements BrowserTransport {
  listeners = new Set<(message: NativeMessage) => void>();
  sent: FrontendMessage[] = [];
  handshakeCount = 0;
  constructor(private readonly windows: WindowState[] = []) {}
  send(message: FrontendMessage) {
    this.sent.push(message);
    if (message.type === "request" && message.request.type === "runtime_handshake") {
      this.handshakeCount += 1;
      for (const listener of this.listeners) listener({ type: "response", id: message.id, result: { status: "ok", data: { type: "runtime_handshake", protocol_version: 1, windows: this.windows } } });
    }
  }
  subscribe(listener: (message: NativeMessage) => void) { this.listeners.add(listener); return () => this.listeners.delete(listener); }
  receive(message: NativeMessage) { for (const listener of this.listeners) listener(message); }
}

function Probe() {
  const browser = useBrowser();
  const page = useActivePage();
  return <span data-testid="probe">{`${browser.constructor.name}:${page?.id ?? "none"}`}</span>;
}

describe("@browserkit/react", () => {
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("provides browser and renders BrowserView placeholder", () => {
    const client = new BrowserClient(new MockTransport());
    render(<BrowserProvider client={client}><Probe /><BrowserView page={{ id: 4, url: "", title: "", loading: false, can_go_back: false, can_go_forward: false, active: true }} /></BrowserProvider>);
    expect(screen.getByTestId("probe").textContent).toBe("BrowserClient:none");
    expect(document.querySelector("[data-browserkit-view='4']")).toBeTruthy();
  });

  it("does not duplicate the handshake in StrictMode", async () => {
    const transport = new MockTransport();
    const client = new BrowserClient(transport);
    await act(async () => {
      render(<StrictMode><BrowserProvider client={client}><Probe /></BrowserProvider></StrictMode>);
      await Promise.resolve();
    });
    expect(transport.handshakeCount).toBe(1);
  });

  it("reflects native state events", async () => {
    const transport = new MockTransport([{ id: 1, active_page_id: 4, pages: [{ id: 4, url: "https://example.com", title: "", loading: false, can_go_back: false, can_go_forward: false, active: true }] }]);
    const client = new BrowserClient(transport);
    await act(async () => {
      render(<BrowserProvider client={client}><Probe /></BrowserProvider>);
      await Promise.resolve();
    });
    transport.receive({ type: "event", event: { type: "page_title_changed", page_id: 4, title: "Example" } });
    expect(screen.getByTestId("probe").textContent).toBe("BrowserClient:4");
  });

  it("measures, coalesces, and sends BrowserView bounds", async () => {
    const transport = new MockTransport([{ id: 1, active_page_id: 4, pages: [{ id: 4, url: "https://example.com", title: "", loading: false, can_go_back: false, can_go_forward: false, active: true }] }]);
    const client = new BrowserClient(transport);
    let rect = { x: 10.5, y: 20, width: 400.25, height: 300 };
    const frameCallbacks: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { frameCallbacks.push(callback); return frameCallbacks.length; });
    vi.stubGlobal("cancelAnimationFrame", (id: number) => { frameCallbacks[id - 1] = () => undefined; });
    class TestResizeObserver {
      static instances: TestResizeObserver[] = [];
      constructor(private readonly callback: ResizeObserverCallback) { TestResizeObserver.instances.push(this); }
      observe() {}
      disconnect() {}
      trigger() { this.callback([], this as unknown as ResizeObserver); }
    }
    vi.stubGlobal("ResizeObserver", TestResizeObserver);
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(() => rect as DOMRect);

    await act(async () => {
      render(<BrowserProvider client={client}><BrowserView page={{ id: 4, url: "https://example.com", title: "", loading: false, can_go_back: false, can_go_forward: false, active: true }} /></BrowserProvider>);
      await Promise.resolve();
    });
    expect(transport.sent.some((message) => message.type === "command" && message.command.type === "page_set_view_bounds" && message.command.rect.x === 10.5)).toBe(true);

    const before = transport.sent.filter((message) => message.type === "command" && message.command.type === "page_set_view_bounds").length;
    rect = { x: 30, y: 20, width: 400.25, height: 300 };
    TestResizeObserver.instances[0]?.trigger();
    await act(async () => { frameCallbacks.shift()?.(0); });
    const after = transport.sent.filter((message) => message.type === "command" && message.command.type === "page_set_view_bounds").length;
    expect(after).toBe(before + 1);
  });

  it("unregisters the old page when switching BrowserView ownership", async () => {
    const transport = new MockTransport([{ id: 1, active_page_id: 4, pages: [
      { id: 4, url: "https://example.com", title: "", loading: false, can_go_back: false, can_go_forward: false, active: true },
      { id: 5, url: "https://example.org", title: "", loading: false, can_go_back: false, can_go_forward: false, active: false },
    ] }]);
    const client = new BrowserClient(transport);
    const { rerender } = render(<BrowserProvider client={client}><BrowserView page={{ id: 4, url: "", title: "", loading: false, can_go_back: false, can_go_forward: false, active: true }} /></BrowserProvider>);
    await act(async () => { await Promise.resolve(); });
    rerender(<BrowserProvider client={client}><BrowserView page={{ id: 5, url: "", title: "", loading: false, can_go_back: false, can_go_forward: false, active: false }} /></BrowserProvider>);
    const unregister = transport.sent.find((message) => message.type === "command" && message.command.type === "page_unregister_view");
    expect(unregister).toEqual({ type: "command", command: { type: "page_unregister_view", page_id: 4 } });
  });

  it("re-registers a mounted BrowserView after reconnect", async () => {
    const transport = new MockTransport([{ id: 1, active_page_id: 4, pages: [{ id: 4, url: "https://example.com", title: "", loading: false, can_go_back: false, can_go_forward: false, active: true }] }]);
    const client = new BrowserClient(transport);
    render(<BrowserProvider client={client}><BrowserView page={{ id: 4, url: "", title: "", loading: false, can_go_back: false, can_go_forward: false, active: true }} /></BrowserProvider>);
    await act(async () => { await Promise.resolve(); });
    const registrationsBefore = transport.sent.filter((message) => message.type === "command" && message.command.type === "page_register_view").length;
    client.disconnect();
    await act(async () => { await client.connect(); });
    const registrationsAfter = transport.sent.filter((message) => message.type === "command" && message.command.type === "page_register_view").length;
    expect(registrationsAfter).toBe(registrationsBefore + 1);
  });
});

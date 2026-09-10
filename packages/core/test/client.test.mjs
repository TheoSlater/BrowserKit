import test from "node:test";
import assert from "node:assert/strict";
import { BrowserClient } from "../dist/index.js";

test("client correlates requests and applies window state", async () => {
  const listeners = new Set();
  const sent = [];
  globalThis.window = {
    devicePixelRatio: 1,
    visualViewport: { scale: 1 },
    setTimeout: () => 0,
    ipc: { postMessage: (raw) => sent.push(JSON.parse(raw)) },
  };
  const transport = {
    send: (message) => {
      sent.push(message);
      if (message.type === "request") listeners.forEach((listener) => listener({
        type: "response", id: message.id, ok: true,
        data: { type: "window.state", state: { activePageId: 7, pages: [{
          pageId: 7, url: "https://example.com", title: "Example", loading: false,
          canGoBack: false, canGoForward: false, active: true,
        }] } },
      }));
    },
    subscribe: (listener) => { listeners.add(listener); return () => listeners.delete(listener); },
  };
  const client = new BrowserClient(transport);
  await client.getWindowState();
  assert.equal(client.getActivePage().pageId, 7);
  client.pages.reload(7);
  assert.deepEqual(sent.at(-1).command, { type: "page.reload", pageId: 7 });
});

import type { FrontendMessage, NativeMessage } from "./protocol";

export interface BrowserTransport {
  send(message: FrontendMessage): void;
  subscribe(listener: (message: NativeMessage) => void): () => void;
}

export type BrowserEvent = NativeMessage & { type: "event" };

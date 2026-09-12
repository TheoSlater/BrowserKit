export const PROTOCOL_VERSION = 1 as const;

export type RequestId = string;
export type PageId = number;
export type WindowId = number;

export interface LogicalRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type CoordinateSpace = "frontend_logical";

export interface PageViewBounds {
  rect: LogicalRect;
  coordinate_space: CoordinateSpace;
  device_pixel_ratio: number;
  visual_viewport_scale: number;
}

export interface PageViewBoundsInput {
  rect: LogicalRect;
  coordinateSpace: CoordinateSpace;
  devicePixelRatio: number;
  visualViewportScale: number;
}

export interface PageOptions {
  url: string;
}

export interface PageState {
  id: PageId;
  url: string;
  title: string;
  loading: boolean;
  can_go_back: boolean;
  can_go_forward: boolean;
  active: boolean;
}

export interface WindowState {
  id: WindowId;
  active_page_id: PageId | null;
  pages: PageState[];
}

export type Command =
  | { type: "page_navigate"; page_id: PageId; url: string }
  | { type: "page_reload"; page_id: PageId }
  | { type: "page_go_back"; page_id: PageId }
  | { type: "page_go_forward"; page_id: PageId }
  | { type: "page_stop"; page_id: PageId }
  | { type: "page_activate"; window_id: WindowId; page_id: PageId }
  | { type: "page_close"; window_id: WindowId; page_id: PageId }
  | { type: "page_register_view"; page_id: PageId }
  | ({ type: "page_set_view_bounds"; page_id: PageId } & PageViewBounds)
  | { type: "page_unregister_view"; page_id: PageId };

export type Request =
  | { type: "runtime_handshake"; protocol_version: number }
  | { type: "page_create"; window_id: WindowId; options: PageOptions }
  | { type: "page_get_state"; page_id: PageId }
  | { type: "window_get_state"; window_id: WindowId };

export type FrontendMessage =
  | { type: "command"; command: Command }
  | { type: "request"; id: RequestId; request: Request };

export type ResponseData =
  | { type: "runtime_handshake"; protocol_version: number; windows: WindowState[] }
  | { type: "page_created"; page: PageState }
  | { type: "page_state"; page: PageState }
  | { type: "window_state"; window: WindowState };

export type ProtocolErrorCode =
  | "invalid_message"
  | "unsupported_message"
  | "protocol_version_mismatch"
  | "window_not_found"
  | "page_not_found"
  | "invalid_url"
  | "runtime_not_ready"
  | "runtime_shutting_down"
  | "request_failed"
  | "invalid_view_bounds";

export interface ProtocolError {
  code: ProtocolErrorCode;
  message: string;
}

export type NativeMessage =
  | { type: "response"; id: RequestId; result: { status: "ok"; data: ResponseData } | { status: "err"; error: ProtocolError } }
  | { type: "event"; event: Event };

export type Event =
  | { type: "page_created"; page: PageState }
  | { type: "page_url_changed"; page_id: PageId; url: string }
  | { type: "page_title_changed"; page_id: PageId; title: string }
  | { type: "page_loading_changed"; page_id: PageId; loading: boolean }
  | { type: "page_navigation_state_changed"; page_id: PageId; can_go_back: boolean; can_go_forward: boolean }
  | { type: "page_activated"; window_id: WindowId; page_id: PageId }
  | { type: "page_closed"; window_id: WindowId; page_id: PageId };

export function parseNativeMessage(value: unknown): NativeMessage {
  if (!value || typeof value !== "object") {
    throw new Error("native message must be an object");
  }
  const message = value as { type?: unknown; id?: unknown; result?: unknown; event?: unknown };
  if (message.type === "response" && typeof message.id === "string" && message.result && typeof message.result === "object") {
    const result = message.result as { status?: unknown; data?: unknown; error?: unknown };
    if (result.status === "ok" && result.data && typeof result.data === "object") return value as NativeMessage;
    if (result.status === "err" && result.error && typeof result.error === "object") return value as NativeMessage;
  }
  if (message.type === "event" && message.event && typeof message.event === "object") {
    return value as NativeMessage;
  }
  throw new Error("unknown native message type");
}

export type PageId = number;

export interface LogicalRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PageState {
  pageId: PageId;
  url: string | null;
  title: string | null;
  loading: boolean;
  canGoBack: boolean;
  canGoForward: boolean;
  active: boolean;
}

export interface WindowState {
  activePageId: PageId | null;
  pages: PageState[];
}

export type Command =
  | { type: "page.navigate"; pageId?: PageId; url: string }
  | { type: "page.reload"; pageId?: PageId }
  | { type: "page.go_back"; pageId?: PageId }
  | { type: "page.go_forward"; pageId?: PageId }
  | { type: "page.activate"; pageId: PageId }
  | { type: "page.close"; pageId: PageId }
  | { type: "page.set_visible"; pageId: PageId; visible: boolean }
  | {
      type: "page.set_view_bounds";
      pageId: PageId;
      rect: LogicalRect;
      coordinateSpace: "frontend_logical";
      devicePixelRatio: number;
      visualViewportScale: number;
    };

export type Request =
  | { type: "page.create"; url?: string }
  | { type: "page.get_state"; pageId?: PageId }
  | { type: "window.get_state" };

export type FrontendMessage =
  | { type: "command"; command: Command }
  | { type: "request"; id: string; request: Request };

export type NativeEvent =
  | { type: "page.created"; state: PageState }
  | { type: "page.closed"; pageId: PageId }
  | { type: "page.activated"; state: PageState }
  | { type: "page.url_changed"; pageId: PageId; url: string | null }
  | { type: "page.title_changed"; pageId: PageId; title: string | null }
  | { type: "page.loading_changed"; pageId: PageId; loading: boolean }
  | {
      type: "page.navigation_state_changed";
      pageId: PageId;
      canGoBack: boolean;
      canGoForward: boolean;
    };

export type ResponseData =
  | { type: "page.created"; pageId: PageId }
  | { type: "page.state"; state: PageState }
  | { type: "window.state"; state: WindowState };

export interface ProtocolError {
  code: string;
  message: string;
}

export type NativeMessage =
  | { type: "event"; event: NativeEvent }
  | {
      type: "response";
      id: string;
      ok: boolean;
      data?: ResponseData;
      error?: ProtocolError;
    };

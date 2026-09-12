import type { Event, PageId, PageState, WindowState } from "./protocol";

export interface BrowserState {
  windows: WindowState[];
}

export type StateListener = () => void;

export class BrowserStateStore {
  private state: BrowserState = { windows: [] };
  private readonly listeners = new Set<StateListener>();

  getState(): BrowserState {
    return this.state;
  }

  subscribe(listener: StateListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  hydrate(windows: WindowState[]): void {
    this.update({ windows: windows.map(copyWindow) });
  }

  apply(event: Event): void {
    const windows = this.state.windows.map(copyWindow);
    const window = windows[0];
    if (!window && event.type !== "page_created") return;

    switch (event.type) {
      case "page_created":
        if (!window) return;
        if (!window.pages.some((page) => page.id === event.page.id)) window.pages.push({ ...event.page });
        break;
      case "page_url_changed":
        updatePage(windows, event.page_id, (page) => { page.url = event.url; });
        break;
      case "page_title_changed":
        updatePage(windows, event.page_id, (page) => { page.title = event.title; });
        break;
      case "page_loading_changed":
        updatePage(windows, event.page_id, (page) => { page.loading = event.loading; });
        break;
      case "page_navigation_state_changed":
        updatePage(windows, event.page_id, (page) => {
          page.can_go_back = event.can_go_back;
          page.can_go_forward = event.can_go_forward;
        });
        break;
      case "page_activated":
        for (const candidate of windows) {
          if (candidate.id !== event.window_id) continue;
          candidate.active_page_id = event.page_id;
          for (const page of candidate.pages) page.active = page.id === event.page_id;
        }
        break;
      case "page_closed":
        for (const candidate of windows) {
          if (candidate.id !== event.window_id) continue;
          candidate.pages = candidate.pages.filter((page) => page.id !== event.page_id);
          if (candidate.active_page_id === event.page_id) {
            candidate.active_page_id = candidate.pages.find((page) => page.active)?.id ?? candidate.pages[0]?.id ?? null;
            for (const page of candidate.pages) page.active = page.id === candidate.active_page_id;
          }
        }
        break;
    }
    this.update({ windows });
  }

  addPage(page: PageState): void {
    const windows = this.state.windows.map(copyWindow);
    if (windows[0] && !windows[0].pages.some((candidate) => candidate.id === page.id)) {
      windows[0].pages.push({ ...page });
      this.update({ windows });
    }
  }

  private update(next: BrowserState): void {
    if (JSON.stringify(this.state) === JSON.stringify(next)) return;
    this.state = next;
    for (const listener of this.listeners) listener();
  }
}

function updatePage(windows: WindowState[], id: PageId, update: (page: PageState) => void): void {
  for (const window of windows) {
    const page = window.pages.find((candidate) => candidate.id === id);
    if (page) update(page);
  }
}

function copyWindow(window: WindowState): WindowState {
  return { ...window, pages: window.pages.map((page) => ({ ...page })) };
}

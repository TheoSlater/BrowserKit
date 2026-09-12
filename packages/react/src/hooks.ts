import { useCallback, useSyncExternalStore } from "react";
import type { PageState, WindowState } from "@browserkit/core";
import { useBrowser } from "./provider";

export function useWindowState(): WindowState | undefined {
  const browser = useBrowser();
  const subscribe = useCallback((listener: () => void) => browser.subscribe(listener), [browser]);
  const getSnapshot = useCallback(() => browser.getWindowState(), [browser]);
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

export function usePages(): PageState[] {
  return useWindowState()?.pages ?? [];
}

export function useActivePage(): PageState | undefined {
  const window = useWindowState();
  return window?.pages.find((page) => page.id === window.active_page_id);
}

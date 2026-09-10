import { useSyncExternalStore } from "react";
import type { PageState, WindowState } from "@browserkit/core";
import { useBrowser } from "./provider";

function useRevision() {
  const browser = useBrowser();
  return useSyncExternalStore(
    (listener) => browser.subscribe(() => listener()),
    () => browser.getWindowSnapshot(),
    () => browser.getWindowSnapshot(),
  );
}

export function useWindowState(): WindowState {
  return useRevision();
}

export function useActivePage(): PageState | undefined {
  const browser = useBrowser();
  useRevision();
  return browser.getActivePage();
}

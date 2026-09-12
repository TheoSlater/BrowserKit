import { forwardRef, useLayoutEffect, useRef, type HTMLAttributes } from "react";
import type { LogicalRect, PageState } from "@browserkit/core";
import { useBrowser } from "./provider";
import { observeElementRect } from "./geometry";

export interface BrowserViewProps extends HTMLAttributes<HTMLDivElement> {
  page?: PageState;
}

export const BrowserView = forwardRef<HTMLDivElement, BrowserViewProps>(function BrowserView(
  { page, ...props },
  forwardedRef,
) {
  const browser = useBrowser();
  const elementRef = useRef<HTMLDivElement | null>(null);
  const latestRect = useRef<LogicalRect | undefined>(undefined);
  const pageId = page?.id;

  useLayoutEffect(() => {
    const element = elementRef.current;
    if (element === null || pageId === undefined) return;

    const bounds = (rect: LogicalRect) => {
      latestRect.current = rect;
      console.debug("BrowserKit: browser_view_measured", { pageId, rect });
      if (!browser.isConnected()) return;
      browser.pages.setViewBounds(pageId, {
        rect,
        coordinateSpace: "frontend_logical",
        devicePixelRatio: window.devicePixelRatio,
        visualViewportScale: window.visualViewport?.scale ?? 1,
      });
    };

    const register = () => {
      if (!browser.isConnected()) return;
      browser.pages.registerView(pageId);
      if (latestRect.current) bounds(latestRect.current);
    };

    const observation = observeElementRect(element, bounds);
    const unsubscribe = browser.subscribeConnection(() => {
      if (!browser.isConnected()) return;
      register();
      observation.invalidate();
    });
    register();

    return () => {
      unsubscribe();
      observation.stop();
      if (browser.isConnected()) browser.pages.unregisterView(pageId);
    };
  }, [browser, pageId]);

  return (
    <div
      {...props}
      ref={(element) => {
        elementRef.current = element;
        if (typeof forwardedRef === "function") forwardedRef(element);
        else if (forwardedRef) forwardedRef.current = element;
      }}
      data-browserkit-view={pageId ?? "empty"}
    />
  );
});

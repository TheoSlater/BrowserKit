import { useLayoutEffect, useRef, type HTMLAttributes } from "react";
import type { PageState } from "@browserkit/core";
import { useBrowser } from "./provider";

export interface BrowserViewProps extends HTMLAttributes<HTMLDivElement> {
  page?: PageState;
}

export function BrowserView({ page, ...props }: BrowserViewProps) {
  const browser = useBrowser();
  const ref = useRef<HTMLDivElement>(null);
  const last = useRef<{ pageId: number; x: number; y: number; width: number; height: number } | undefined>(undefined);

  useLayoutEffect(() => {
    let frames = 4;
    let frame = 0;
    const measure = () => {
      if (!ref.current || !page) return;
      const rect = ref.current.getBoundingClientRect();
      const next = { pageId: page.pageId, x: rect.x, y: rect.y, width: rect.width, height: rect.height };
      const previous = last.current;
      const changed = !previous || previous.pageId !== next.pageId ||
        ["x", "y", "width", "height"].some((key) => Math.abs(next[key as keyof typeof next] - previous[key as keyof typeof previous]) > 0.01);
      if (changed) {
        last.current = next;
        browser.pages.setViewBounds(page.pageId, {
          x: next.x, y: next.y, width: next.width, height: next.height,
        });
      }
      if (frames-- > 0) frame = requestAnimationFrame(measure);
    };
    const schedule = () => { frames = 4; cancelAnimationFrame(frame); frame = requestAnimationFrame(measure); };
    const observer = new ResizeObserver(schedule);
    if (ref.current) observer.observe(ref.current);
    window.addEventListener("resize", schedule);
    schedule();
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", schedule);
      cancelAnimationFrame(frame);
      if (page) browser.pages.setVisible(page.pageId, false);
    };
  }, [browser, page?.pageId]);

  useLayoutEffect(() => {
    if (page) browser.pages.setVisible(page.pageId, true);
  }, [browser, page?.pageId]);

  return <div ref={ref} data-browserkit-view={page?.pageId ?? ""} {...props} />;
}

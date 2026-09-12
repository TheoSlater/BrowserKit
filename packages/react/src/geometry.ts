import type { LogicalRect } from "@browserkit/core";

export interface ElementRectObservation {
  invalidate(): void;
  stop(): void;
}

const rectFromElement = (element: HTMLElement): LogicalRect => {
  const rect = element.getBoundingClientRect();
  return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
};

const sameRect = (a: LogicalRect, b: LogicalRect, epsilon: number) =>
  Math.abs(a.x - b.x) <= epsilon &&
  Math.abs(a.y - b.y) <= epsilon &&
  Math.abs(a.width - b.width) <= epsilon &&
  Math.abs(a.height - b.height) <= epsilon;

export function observeElementRect(
  element: HTMLElement,
  onRect: (rect: LogicalRect) => void,
  epsilon = 0.05,
): ElementRectObservation {
  let stopped = false;
  let frame: number | undefined;
  let previous: LogicalRect | undefined;
  let settlingFrames = 0;
  let stableFrames = 0;

  const cancelFrame = () => {
    if (frame === undefined) return;
    if (typeof window.cancelAnimationFrame === "function") window.cancelAnimationFrame(frame);
    else clearTimeout(frame);
    frame = undefined;
  };

  const schedule = () => {
    if (stopped || frame !== undefined) return;
    const callback = () => {
      frame = undefined;
      if (stopped) return;
      const next = rectFromElement(element);
      if (!previous || !sameRect(previous, next, epsilon)) {
        previous = next;
        stableFrames = 0;
        onRect(next);
      } else {
        stableFrames += 1;
      }
      if (settlingFrames > 0) {
        settlingFrames -= 1;
        if (stableFrames < 2 && settlingFrames > 0) schedule();
        else settlingFrames = 0;
      }
    };
    if (typeof window.requestAnimationFrame === "function") frame = window.requestAnimationFrame(callback);
    else frame = setTimeout(callback, 0) as unknown as number;
  };

  const invalidate = () => {
    settlingFrames = 20;
    stableFrames = 0;
    schedule();
  };

  const resizeObserver = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(invalidate);
  resizeObserver?.observe(element);
  window.addEventListener("resize", invalidate);
  window.addEventListener("scroll", invalidate, { capture: true, passive: true });
  window.visualViewport?.addEventListener("resize", invalidate);
  window.visualViewport?.addEventListener("scroll", invalidate);

  const mutationObservers: MutationObserver[] = [];
  if (typeof MutationObserver !== "undefined") {
    for (let ancestor: HTMLElement | null = element.parentElement; ancestor; ancestor = ancestor.parentElement) {
      const observer = new MutationObserver(invalidate);
      observer.observe(ancestor, {
        attributes: true,
        attributeFilter: ["class", "style"],
        childList: ancestor === element.parentElement,
        subtree: ancestor === element.parentElement,
      });
      mutationObservers.push(observer);
    }
  }

  const initial = rectFromElement(element);
  previous = initial;
  onRect(initial);
  invalidate();

  return {
    invalidate,
    stop() {
      if (stopped) return;
      stopped = true;
      cancelFrame();
      resizeObserver?.disconnect();
      for (const observer of mutationObservers) observer.disconnect();
      window.removeEventListener("resize", invalidate);
      window.removeEventListener("scroll", invalidate, { capture: true });
      window.visualViewport?.removeEventListener("resize", invalidate);
      window.visualViewport?.removeEventListener("scroll", invalidate);
    },
  };
}

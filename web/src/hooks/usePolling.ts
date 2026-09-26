/**
 * Interval polling that pauses while the tab is hidden.
 * Prevents background tabs from hammering `/api/*` endpoints.
 */
import { useEffect, useRef } from "react";

/**
 * Call `fn` every `intervalMs`. Starts immediately, then on an interval.
 * Pauses when `document.hidden` and runs once on becoming visible again.
 */
export function usePolling(fn: () => void, intervalMs: number, enabled = true): void {
  const fnRef = useRef(fn);
  fnRef.current = fn;

  useEffect(() => {
    if (!enabled) return;

    const run = () => {
      if (!document.hidden) fnRef.current();
    };
    run();
    const id = setInterval(run, intervalMs);
    const onVisible = () => {
      if (!document.hidden) fnRef.current();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [intervalMs, enabled]);
}

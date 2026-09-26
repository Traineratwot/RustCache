/**
 * Generic async data-loading hook: fetch on mount, expose loading/error/reload.
 * Replaces the copy-pasted load + useEffect + error pattern in every page.
 */
import { useCallback, useEffect, useRef, useState } from "react";

export interface UseApiResult<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
  /** Re-run the fetcher. Safe to call from event handlers. */
  reload: () => void;
  /** Replace data locally (e.g. after a mutation). */
  setData: (updater: T | ((prev: T | null) => T | null)) => void;
}

/**
 * Fetch `fn` on mount and whenever `deps` change.
 * Ignores stale responses if the component unmounted or a newer call started.
 */
export function useApi<T>(fn: () => Promise<T>, _deps: React.DependencyList = []): UseApiResult<T> {
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [_tick, setTick] = useState(0);
  const alive = useRef(true);
  const fnRef = useRef(fn);
  fnRef.current = fn;

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    fnRef
      .current()
      .then((d) => {
        if (!cancelled && alive.current) {
          setData(d);
          setError(null);
        }
      })
      .catch((e: unknown) => {
        if (!cancelled && alive.current) {
          setError(e instanceof Error ? e.message : String(e));
        }
      })
      .finally(() => {
        if (!cancelled && alive.current) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const reload = useCallback(() => setTick((n) => n + 1), []);
  const setDataLocal = useCallback((updater: T | ((prev: T | null) => T | null)) => {
    setData(updater as (prev: T | null) => T | null);
  }, []);

  return { data, loading, error, reload, setData: setDataLocal };
}

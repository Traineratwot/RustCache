/**
 * Debounce a rapidly changing value (e.g. search box) before triggering a fetch.
 */
import { useEffect, useState } from "react";

/**
 * Return `value` after it has been stable for `delayMs`.
 * Use for search inputs so typing does not fire one request per keystroke.
 */
export function useDebounced<T>(value: T, delayMs = 300): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(id);
  }, [value, delayMs]);
  return debounced;
}

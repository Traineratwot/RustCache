/**
 * Toast notifications for pages that use PrimeReact Toast.
 * Owns the ref and exposes typed show helpers so pages skip the boilerplate.
 */
import type { Toast } from "primereact/toast";
import { useCallback, useRef } from "react";

export interface ToastApi {
  toastRef: React.RefObject<Toast | null>;
  /** Success toast with optional detail. */
  success: (summary: string, detail?: string) => void;
  /** Error toast with optional detail. */
  error: (summary: string, detail?: string) => void;
  /** Warning toast. */
  warn: (summary: string, detail?: string) => void;
}

/** Create a Toast ref plus show helpers for use with `<Toast ref={toastRef} />`. */
export function useToast(): ToastApi {
  const toastRef = useRef<Toast | null>(null);

  const show = useCallback(
    (severity: "success" | "error" | "warn", summary: string, detail?: string) => {
      toastRef.current?.show({ severity, summary, detail, life: 4000 });
    },
    [],
  );

  return {
    toastRef,
    success: useCallback((s: string, d?: string) => show("success", s, d), [show]),
    error: useCallback((s: string, d?: string) => show("error", s, d), [show]),
    warn: useCallback((s: string, d?: string) => show("warn", s, d), [show]),
  };
}

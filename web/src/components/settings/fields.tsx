/**
 * Shared Settings form building blocks: labeled fields, section titles,
 * apply-mode badges, and path-resolution previews.
 *
 * Markup matches the historical Settings styles (`.field-label-row`,
 * `.apply-badge`, `.field-error`) so the form looks unchanged after extraction.
 */
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { isAbsolutePath, resolveUnder } from "../../lib/paths";

/** Section heading inside the Settings form. */
export function SectionTitle({ children }: { children: ReactNode }) {
  return <h3 style={{ marginTop: 0, marginBottom: "0.75rem", fontSize: "1.05rem" }}>{children}</h3>;
}

/**
 * Badge describing when a setting takes effect (hot-reload vs restart).
 * `mode="hot"` fields apply without rebinding sockets.
 */
export function ApplyBadge({ mode }: { mode: "hot" | "restart" }) {
  const { t } = useTranslation();
  const isRestart = mode === "restart";
  return (
    <span
      className={`apply-badge ${isRestart ? "apply-restart" : "apply-hot"}`}
      title={isRestart ? t("settings.applyRestartTitle") : t("settings.applyHotTitle")}
    >
      <i className={isRestart ? "pi pi-refresh" : "pi pi-bolt"} />
      {isRestart ? t("settings.applyRestart") : t("settings.applyHot")}
    </span>
  );
}

/**
 * Labeled form field with optional hint, apply badge, and validation error.
 * Children is the actual input control.
 */
export function Field({
  label,
  hint,
  apply,
  error,
  children,
}: {
  label: string;
  hint?: string;
  apply?: "hot" | "restart";
  error?: string;
  children: ReactNode;
}) {
  return (
    // biome-ignore lint/a11y/noLabelWithoutControl: label wraps the PrimeReact control
    <label className={`flex flex-column gap-1 mb-3${error ? " field-has-error" : ""}`}>
      <span className="field-label-row">
        <span className="text-color-secondary">{label}</span>
        {apply ? <ApplyBadge mode={apply} /> : null}
      </span>
      {children}
      {error ? (
        <small className="field-error" style={{ lineHeight: 1.35 }}>
          {error}
        </small>
      ) : hint ? (
        <small className="text-color-secondary" style={{ lineHeight: 1.35 }}>
          {hint}
        </small>
      ) : null}
    </label>
  );
}

/**
 * Shows whether a path is absolute/relative and where it will resolve under
 * the data directory. Display only — the backend does the real resolution.
 */
export function PathResolved({ dataDir, value }: { dataDir: string; value: string }) {
  const { t } = useTranslation();
  return (
    <span className="path-resolved">
      <span className="tag">
        {isAbsolutePath(value) ? t("settings.pathAbsolute") : t("settings.pathRelative")}
      </span>
      <code>{resolveUnder(dataDir, value)}</code>
    </span>
  );
}

import { i18n } from "../i18n";

function intlLocale(): string {
  const lang = i18n.resolvedLanguage || i18n.language || "en";
  return lang.startsWith("ru") ? "ru-RU" : "en-US";
}

export function fmtBytes(n: number): string {
  if (n < 1024) return i18n.t("units.bytes.b", { n });
  if (n < 1024 * 1024) return i18n.t("units.bytes.kb", { n: (n / 1024).toFixed(1) });
  if (n < 1024 * 1024 * 1024) return i18n.t("units.bytes.mb", { n: (n / 1024 / 1024).toFixed(1) });
  return i18n.t("units.bytes.gb", { n: (n / 1024 / 1024 / 1024).toFixed(2) });
}

export function fmtMb(bytes: number): string {
  return i18n.t("units.mbRaw", { n: (bytes / 1024 / 1024).toFixed(0) });
}

export function fmtBucket(ms: number): string {
  if (ms < 60_000) return i18n.t("units.time.sec", { n: Math.round(ms / 1000) });
  if (ms < 3_600_000) return i18n.t("units.time.min", { n: Math.round(ms / 60_000) });
  if (ms < 86_400_000) return i18n.t("units.time.hour", { n: Math.round(ms / 3_600_000) });
  return i18n.t("units.time.day", { n: Math.round(ms / 86_400_000) });
}

export function fmtUptime(s: number): string {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const parts: string[] = [];
  if (d > 0) parts.push(i18n.t("units.time.day", { n: d }));
  if (h > 0) parts.push(i18n.t("units.time.hour", { n: h }));
  if (m > 0) parts.push(i18n.t("units.time.min", { n: m }));
  parts.push(i18n.t("units.time.sec", { n: sec }));
  return parts.join(" ");
}

export function fmtMs(n: number, digits = 1): string {
  return i18n.t("common.ms", { n: n.toFixed(digits) });
}

export function fmtDate(d: Date | number): string {
  const date = typeof d === "number" ? new Date(d) : d;
  return date.toLocaleDateString(intlLocale());
}

export function fmtTime(d: Date | number): string {
  const date = typeof d === "number" ? new Date(d) : d;
  return date.toLocaleTimeString(intlLocale(), { hour: "2-digit", minute: "2-digit" });
}

export function fmtDateTime(d: Date | number): string {
  const date = typeof d === "number" ? new Date(d) : d;
  return date.toLocaleString(intlLocale(), {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

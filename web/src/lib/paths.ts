/**
 * Path helpers mirroring the Rust config path resolution rules.
 * Used by the Settings form to preview where a relative path will land.
 * Display only — the backend resolves the real paths.
 */

/**
 * Resolve `p` under `dataDir` the same way the backend does:
 * absolute paths (and `~/...`) are used as-is; relative paths join under dataDir.
 */
export function resolveUnder(dataDir: string, p: string): string {
  const s = p.trim();
  if (s.startsWith("/") || s.startsWith("~")) return s;
  const base = dataDir.replace(/\/+$/, "");
  return `${base}/${s}`;
}

/** True when the path is absolute or home-relative (`~` / `~/...`). */
export function isAbsolutePath(p: string): boolean {
  const s = p.trim();
  return s.startsWith("/") || s.startsWith("~");
}

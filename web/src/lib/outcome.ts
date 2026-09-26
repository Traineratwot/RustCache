/**
 * Canonical cache-outcome labels and display severity.
 * Single source of truth for Dashboard and Requests tables.
 */

/** Outcome labels written by the proxy (stable wire format). */
export const OUTCOMES = [
  "HIT",
  "HIT_REVALIDATED",
  "REVALIDATED",
  "MISS",
  "BYPASS",
  "TUNNEL",
  "ERROR",
  "REJECT_CMD",
] as const;

export type Outcome = (typeof OUTCOMES)[number];

/** PrimeReact Tag severity for an outcome badge. */
export type Severity = "success" | "info" | "warning" | "danger" | "secondary";

/**
 * Map an outcome label to a visual severity (shared by Dashboard and Requests).
 * Unknown labels (legacy rows) render as danger to stand out in the table.
 */
export function outcomeSeverity(outcome: string): Severity {
  switch (outcome) {
    case "HIT":
    case "HIT_REVALIDATED":
      return "success";
    case "MISS":
    case "REVALIDATED":
      return "info";
    case "BYPASS":
    case "REJECT_CMD":
      return "warning";
    case "TUNNEL":
      return "secondary";
    default:
      return "danger";
  }
}

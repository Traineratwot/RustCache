/**
 * Certificate helpers. Kept out of the API client so networking stays pure.
 */

/**
 * Compute the SHA-256 fingerprint (hex, colon-separated) of a PEM certificate.
 * Used by the CA page to show the trust fingerprint users should expect.
 */
export async function caFingerprint(pem: string): Promise<string> {
  const b64 = pem
    .replace(/-----BEGIN CERTIFICATE-----/g, "")
    .replace(/-----END CERTIFICATE-----/g, "")
    .replace(/\s+/g, "");
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0").toUpperCase())
    .join(":");
}

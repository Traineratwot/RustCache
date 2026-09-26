/**
 * Uniform API error type shared by the typed client and pages.
 * Surfaces HTTP status plus the backend's structured field issues when present.
 */

/** One field-level validation problem returned by `PUT /api/config`. */
export interface FieldIssue {
  field: string;
  message: string;
}

/** Error thrown by `api/client` helpers. Prefer this over bare `Error`. */
export class ApiError extends Error {
  readonly status: number;
  readonly url: string;
  /** Field issues from a config validation response, if any. */
  readonly fieldIssues?: FieldIssue[];

  constructor(message: string, status: number, url: string, fieldIssues?: FieldIssue[]) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.url = url;
    this.fieldIssues = fieldIssues;
  }
}

/**
 * Parse a failed response body into an `ApiError`.
 * Recognizes `{error}` and `{errors: [{field, message}]}` shapes used by the API.
 */
export async function parseApiError(url: string, r: Response): Promise<ApiError> {
  let message = `${url} ${r.status}`;
  let fieldIssues: FieldIssue[] | undefined;
  try {
    const body = await r.json();
    if (body && typeof body === "object") {
      if (typeof body.error === "string" && body.error) {
        message = body.error;
      }
      if (Array.isArray(body.errors)) {
        const issues: FieldIssue[] = body.errors.filter(
          (e: unknown): e is FieldIssue =>
            !!e &&
            typeof e === "object" &&
            typeof (e as FieldIssue).field === "string" &&
            typeof (e as FieldIssue).message === "string",
        );
        if (issues.length > 0) {
          fieldIssues = issues;
          if (message === `${url} ${r.status}`) {
            message = issues.map((f) => `${f.field}: ${f.message}`).join("; ");
          }
        }
      }
    }
  } catch {
    // Non-JSON body — keep the status message.
  }
  return new ApiError(message, r.status, url, fieldIssues);
}

/** Type guard: error carries config field issues. */
export function hasFieldIssues(e: unknown): e is ApiError {
  return e instanceof ApiError && Array.isArray(e.fieldIssues) && e.fieldIssues.length > 0;
}

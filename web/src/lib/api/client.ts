// Typed client for the SnapRequest REST API. The types come from `schema.d.ts`, generated from
// `web/openapi.json` (`npm run gen:api`), which a server test keeps in sync with the handlers.
import createClient from "openapi-fetch";
import type { components, paths } from "./schema";

export type Schemas = components["schemas"];
export type ApiClient = ReturnType<typeof createApi>;

/** Error returned by every API endpoint: `{ error: { kind, message } }`. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly kind: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** Header the server requires on cookie-authenticated requests that change something (CSRF). */
export const CSRF_HEADER = "X-SnapReq-CSRF";

/** Header that selects the workspace whose collections a request works on. */
export const WORKSPACE_HEADER = "X-SnapReq-Workspace";
const WORKSPACE_KEY = "snapreq.workspace";

/** The workspace the UI works in (remembered per browser). */
export function currentWorkspace(): string {
  try {
    return localStorage.getItem(WORKSPACE_KEY) || "default";
  } catch {
    return "default";
  }
}

export function rememberWorkspace(id: string) {
  localStorage.setItem(WORKSPACE_KEY, id);
}

/** Called when the server answers 401 (the session ended); set by the auth state. */
let unauthorized: () => void = () => {};
/** Called when something failed in a way an ended session could explain (a refused WebSocket). */
let mayHaveEnded: () => void = () => {};
export function onUnauthorized(handler: () => void, check: () => void = () => {}) {
  unauthorized = handler;
  mayHaveEnded = check;
}
export function sessionEnded() {
  unauthorized();
}
export function sessionMayHaveEnded() {
  mayHaveEnded();
}

/** A client for the server the UI is served from (or `baseUrl`, e.g. in tests). */
export function createApi(baseUrl = "", fetchImpl?: typeof fetch) {
  const client = createClient<paths>({
    baseUrl,
    headers: { [CSRF_HEADER]: "1" },
    ...(fetchImpl ? { fetch: fetchImpl } : {}),
  });
  client.use({
    onRequest({ request }) {
      request.headers.set(WORKSPACE_HEADER, currentWorkspace());
      return request;
    },
    onResponse({ request, response }) {
      if (response.status === 401 && !new URL(request.url, "http://x").pathname.endsWith("/auth/login")) unauthorized();
    },
  });
  return client;
}

/** Unwraps an openapi-fetch result: the data, or an `ApiError`. */
export function unwrap<T>(result: { data?: T; error?: unknown; response: Response }): T {
  if (result.error !== undefined || result.data === undefined) {
    const detail = (result.error as Partial<Schemas["ErrorBody"]> | undefined)?.error;
    throw new ApiError(
      result.response.status,
      detail?.kind ?? "http",
      detail?.message ?? `${result.response.status} ${result.response.statusText}`.trim(),
    );
  }
  return result.data;
}

export interface ServerVersion {
  name: string;
  version: string;
  api: number;
}

/** `GET /api/v1/version`. */
export async function serverVersion(api: ApiClient): Promise<ServerVersion> {
  const data = unwrap(await api.GET("/api/v1/version"));
  return {
    name: String(data.name ?? "snapreq"),
    version: String(data.version ?? "?"),
    api: Number(data.api ?? 0),
  };
}

export const api = createApi();

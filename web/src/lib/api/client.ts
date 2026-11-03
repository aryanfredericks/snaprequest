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

/** A client for the server the UI is served from (or `baseUrl`, e.g. in tests). */
export function createApi(baseUrl = "", fetchImpl?: typeof fetch) {
  return createClient<paths>({ baseUrl, ...(fetchImpl ? { fetch: fetchImpl } : {}) });
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

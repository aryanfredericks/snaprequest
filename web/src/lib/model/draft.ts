// The request builder's editable model, converted to and from request files (collection
// requests) and core requests (requests outside a collection). Keys the builder does not edit
// (`vars`, `capture`, `assert`, unknown settings) are carried through unchanged.
import type {
  AuthSpec,
  BodySpec,
  CoreAuth,
  CoreBody,
  CoreRequest,
  CoreSettings,
  KeyValue,
  KvEntry,
  KvTable,
  PartSpec,
  RequestFile,
  SettingsSpec,
} from "../api/types";

let nextId = 1;
/** A unique id for list items (stable keys while editing). */
export const uid = () => nextId++;

export interface Row {
  id: number;
  name: string;
  value: string;
  enabled: boolean;
}

export const row = (name = "", value = "", enabled = true): Row => ({ id: uid(), name, value, enabled });

export const BODY_TYPES = ["none", "json", "text", "form", "multipart", "file"] as const;
export type BodyType = (typeof BODY_TYPES)[number];

export interface PartDraft {
  id: number;
  name: string;
  kind: "text" | "file";
  value: string;
  file: string;
  filename: string;
  contentType: string;
  enabled: boolean;
}

export const part = (p: Partial<PartDraft> = {}): PartDraft => ({
  id: uid(),
  name: "",
  kind: "text",
  value: "",
  file: "",
  filename: "",
  contentType: "",
  enabled: true,
  ...p,
});

/** Every body type's data at once, so switching types does not lose what was typed. */
export interface BodyDraft {
  type: BodyType;
  /** Text and JSON content. */
  content: string;
  /** Text and JSON bodies read from a file, and `file` bodies (paths on the server). */
  file: string;
  /** Text and file bodies. */
  contentType: string;
  fields: Row[];
  parts: PartDraft[];
}

export const AUTH_TYPES = [
  "inherit",
  "none",
  "basic",
  "bearer",
  "digest",
  "api_key",
  "oauth2",
  "aws_sigv4",
] as const;
export type AuthType = (typeof AUTH_TYPES)[number];

export interface AuthField {
  key: string;
  label: string;
  secret?: boolean;
  optional?: boolean;
  options?: readonly string[];
  /** Only shown for these OAuth 2.0 grants. */
  grants?: readonly string[];
}

/** The fields of each auth type, as the auth tab shows them. */
export const AUTH_FIELDS: Record<AuthType, AuthField[]> = {
  inherit: [],
  none: [],
  basic: [
    { key: "username", label: "Username" },
    { key: "password", label: "Password", secret: true, optional: true },
  ],
  digest: [
    { key: "username", label: "Username" },
    { key: "password", label: "Password", secret: true, optional: true },
  ],
  bearer: [{ key: "token", label: "Token", secret: true }],
  api_key: [
    { key: "name", label: "Name" },
    { key: "value", label: "Value", secret: true },
    { key: "location", label: "Send in", options: ["header", "query"] },
  ],
  oauth2: [
    { key: "grant", label: "Grant", options: ["client_credentials", "password", "authorization_code"] },
    { key: "token_url", label: "Token URL" },
    { key: "auth_url", label: "Authorization URL", optional: true, grants: ["authorization_code"] },
    { key: "client_id", label: "Client ID" },
    { key: "client_secret", label: "Client secret", secret: true, optional: true },
    { key: "scope", label: "Scope", optional: true },
    { key: "username", label: "Username", optional: true, grants: ["password"] },
    { key: "password", label: "Password", secret: true, optional: true, grants: ["password"] },
    { key: "redirect_uri", label: "Redirect URI", optional: true, grants: ["authorization_code"] },
    { key: "client_auth", label: "Client authentication", optional: true, options: ["", "basic", "body"] },
  ],
  aws_sigv4: [
    { key: "access_key", label: "Access key" },
    { key: "secret_key", label: "Secret key", secret: true },
    { key: "session_token", label: "Session token", secret: true, optional: true },
    { key: "region", label: "Region" },
    { key: "service", label: "Service" },
  ],
};

export interface AuthDraft {
  type: AuthType;
  /** Values of every type's fields (kept while switching types). */
  fields: Record<string, string>;
  /** OAuth 2.0 extra parameters. */
  params: Row[];
}

export interface Draft {
  name: string;
  description: string;
  method: string;
  url: string;
  query: Row[];
  headers: Row[];
  body: BodyDraft;
  auth: AuthDraft;
  settings: SettingsSpec;
  /** Requests outside a collection: `host:port:addr[,addr]` lines (curl's `--resolve`). */
  resolve: string;
  /** File keys the builder does not edit. */
  rest: Pick<RequestFile, "vars" | "capture" | "assert">;
}

export const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] as const;

export function emptyBody(): BodyDraft {
  return { type: "none", content: "", file: "", contentType: "", fields: [], parts: [] };
}

export function emptyDraft(inCollection = false): Draft {
  return {
    name: "",
    description: "",
    method: "GET",
    url: "",
    query: [],
    headers: [],
    body: emptyBody(),
    auth: { type: inCollection ? "inherit" : "none", fields: {}, params: [] },
    settings: {},
    resolve: "",
    rest: {},
  };
}

// --- Key/value tables ---------------------------------------------------------------------

export function rowsFromTable(table: KvTable | undefined): Row[] {
  const rows: Row[] = [];
  for (const [name, value] of Object.entries(table ?? {})) {
    for (const v of Array.isArray(value) ? value : [value]) {
      if (typeof v === "string") rows.push(row(name, v));
      else rows.push(row(name, v.value, v.enabled !== false));
    }
  }
  return rows;
}

/**
 * Rows as a table. Rows with the same name are grouped at the first one's position (the file
 * format keeps one entry per name); rows without a name are dropped.
 */
export function rowsToTable(rows: Row[]): KvTable | undefined {
  const table: Record<string, (string | KvEntry)[]> = {};
  for (const r of rows) {
    const name = r.name.trim();
    if (!name) continue;
    (table[name] ??= []).push(r.enabled ? r.value : { value: r.value, enabled: false });
  }
  const out: KvTable = {};
  for (const [name, values] of Object.entries(table)) {
    out[name] = values.length === 1 ? values[0]! : values;
  }
  return Object.keys(out).length ? out : undefined;
}

export const rowsFromList = (list: KeyValue[] | undefined): Row[] =>
  (list ?? []).map((kv) => row(kv.name, kv.value, kv.enabled !== false));

export function rowsToList(rows: Row[]): KeyValue[] {
  return rows
    .filter((r) => r.name.trim())
    .map((r) => (r.enabled ? { name: r.name.trim(), value: r.value } : { name: r.name.trim(), value: r.value, enabled: false }));
}

// --- Request files -----------------------------------------------------------------------

function bodyFromSpec(spec: BodySpec | undefined): BodyDraft {
  const b = emptyBody();
  if (!spec) return b;
  b.type = spec.type;
  switch (spec.type) {
    case "text":
      b.content = spec.content ?? "";
      b.file = spec.file ?? "";
      b.contentType = spec.content_type ?? "";
      break;
    case "json":
      b.content = spec.content ?? "";
      b.file = spec.file ?? "";
      break;
    case "form":
      b.fields = rowsFromTable(spec.fields);
      break;
    case "multipart":
      b.parts = (spec.parts ?? []).map((p) =>
        part({
          name: p.name,
          kind: p.file !== undefined ? "file" : "text",
          value: p.value ?? "",
          file: p.file ?? "",
          filename: p.filename ?? "",
          contentType: p.content_type ?? "",
          enabled: p.enabled !== false,
        }),
      );
      break;
    case "file":
      b.file = spec.file;
      b.contentType = spec.content_type ?? "";
      break;
  }
  return b;
}

const opt = (v: string) => (v.trim() ? v : undefined);

function bodyToSpec(b: BodyDraft): BodySpec | undefined {
  switch (b.type) {
    case "none":
      return undefined;
    case "text":
      return clean<BodySpec>({
        type: "text",
        ...(b.file.trim() ? { file: b.file.trim() } : { content: b.content }),
        content_type: opt(b.contentType),
      });
    case "json":
      return b.file.trim() ? { type: "json", file: b.file.trim() } : { type: "json", content: b.content };
    case "form":
      return clean<BodySpec>({ type: "form", fields: rowsToTable(b.fields) });
    case "multipart":
      return {
        type: "multipart",
        parts: b.parts
          .filter((p) => p.name.trim())
          .map((p) =>
            clean<PartSpec>({
              name: p.name.trim(),
              ...(p.kind === "file"
                ? { file: p.file, filename: opt(p.filename), content_type: opt(p.contentType) }
                : { value: p.value }),
              enabled: p.enabled ? undefined : false,
            }),
          ),
      };
    case "file":
      return clean<BodySpec>({ type: "file", file: b.file.trim(), content_type: opt(b.contentType) });
  }
}

export function authFromSpec(spec: AuthSpec | undefined): AuthDraft {
  if (!spec) return { type: "inherit", fields: {}, params: [] };
  const { type, ...rest } = spec as AuthSpec & Record<string, unknown>;
  const fields: Record<string, string> = {};
  let params: Row[] = [];
  for (const [k, v] of Object.entries(rest)) {
    if (k === "params") params = rowsFromTable(v as KvTable);
    else if (v !== undefined && v !== null) fields[k] = String(v);
  }
  return { type, fields, params };
}

/** The values the selected auth type uses (empty optional fields left out). */
function authValues(a: AuthDraft): Record<string, string> {
  const out: Record<string, string> = {};
  const grant = a.fields.grant ?? "client_credentials";
  for (const f of AUTH_FIELDS[a.type]) {
    if (f.grants && !f.grants.includes(grant)) continue;
    let v = a.fields[f.key] ?? "";
    if (!v && f.options && !f.optional) v = f.options[0]!;
    if (v || !f.optional) out[f.key] = v;
  }
  return out;
}

export function authToSpec(a: AuthDraft): AuthSpec | undefined {
  if (a.type === "inherit") return undefined;
  const values = authValues(a);
  if (a.type === "api_key" && values.location === "header") delete values.location;
  const params = a.type === "oauth2" ? rowsToTable(a.params) : undefined;
  return clean({ type: a.type, ...values, params }) as AuthSpec;
}

export function draftFromFile(file: RequestFile): Draft {
  return {
    name: file.name ?? "",
    description: file.description ?? "",
    method: (file.method ?? "GET").toUpperCase(),
    url: file.url,
    query: rowsFromTable(file.query),
    headers: rowsFromTable(file.headers),
    body: bodyFromSpec(file.body),
    auth: authFromSpec(file.auth),
    settings: { ...(file.settings ?? {}) },
    resolve: "",
    rest: clean({ vars: file.vars, capture: file.capture, assert: file.assert }),
  };
}

export function draftToFile(d: Draft): RequestFile {
  const settings = clean({ ...d.settings });
  return clean({
    name: opt(d.name),
    description: opt(d.description),
    method: d.method === "GET" ? undefined : d.method,
    url: d.url,
    vars: d.rest.vars,
    query: rowsToTable(d.query),
    headers: rowsToTable(d.headers),
    auth: authToSpec(d.auth),
    body: bodyToSpec(d.body),
    settings: Object.keys(settings).length ? settings : undefined,
    capture: d.rest.capture,
    assert: d.rest.assert,
  });
}

// --- Core requests (outside a collection) ------------------------------------------------

function bodyToCore(b: BodyDraft): CoreBody | undefined {
  switch (b.type) {
    case "none":
      return undefined;
    case "text":
      return clean<CoreBody>({ type: "text", content: b.content, content_type: opt(b.contentType) });
    case "json":
      return { type: "json", content: b.content };
    case "form":
      return { type: "form", fields: rowsToList(b.fields) };
    case "multipart":
      return {
        type: "multipart",
        parts: b.parts
          .filter((p) => p.name.trim())
          .map((p) =>
            clean(
              p.kind === "file"
                ? {
                    name: p.name.trim(),
                    kind: "file" as const,
                    path: p.file,
                    filename: opt(p.filename),
                    content_type: opt(p.contentType),
                    enabled: p.enabled ? undefined : false,
                  }
                : { name: p.name.trim(), kind: "text" as const, value: p.value, enabled: p.enabled ? undefined : false },
            ),
          ),
      };
    case "file":
      return clean<CoreBody>({ type: "file", path: b.file.trim(), content_type: opt(b.contentType) });
  }
}

function authToCore(a: AuthDraft): CoreAuth | undefined {
  if (a.type === "inherit" || a.type === "none") return undefined;
  const values = authValues(a);
  const params = a.type === "oauth2" ? rowsToList(a.params) : [];
  return clean({ type: a.type, ...values, params: params.length ? params : undefined }) as CoreAuth;
}

/** Parses `host:port:addr[,addr]` lines (port `*` = any); throws on a malformed line. */
export function parseResolve(text: string): NonNullable<CoreSettings["resolve"]> {
  return text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => {
      const m = /^([^:]+):(\*|\d+):(.+)$/.exec(l);
      if (!m) throw new Error(`resolve: expected HOST:PORT:ADDR[,ADDR], got \`${l}\``);
      const addrs = m[3]!.split(",").map((a) => a.trim().replace(/^\[(.*)\]$/, "$1"));
      return clean({ host: m[1]!, port: m[2] === "*" ? undefined : Number(m[2]), addrs });
    });
}

function settingsToCore(s: SettingsSpec, resolve: string): CoreSettings | undefined {
  const tls = clean({
    verify: s.verify_tls,
    ca_cert: s.ca_cert,
    client_cert: s.client_cert && s.client_key ? { cert: s.client_cert, key: s.client_key } : undefined,
    min_version: s.tls_min_version,
    max_version: s.tls_max_version,
  });
  const pins = parseResolve(resolve);
  const out: CoreSettings = clean({
    timeout_ms: s.timeout_ms === undefined ? undefined : s.timeout_ms || null,
    connect_timeout_ms: s.connect_timeout_ms === undefined ? undefined : s.connect_timeout_ms || null,
    follow_redirects: s.follow_redirects,
    max_redirects: s.max_redirects,
    decompress: s.decompress,
    cookies: s.cookies,
    http_version: s.http_version,
    proxy: s.proxy,
    tls: Object.keys(tls).length ? tls : undefined,
    resolve: pins.length ? pins : undefined,
  });
  return Object.keys(out).length ? out : undefined;
}

/** A request outside a collection: placeholders are sent as written. */
export function draftToCore(d: Draft): CoreRequest {
  const query = rowsToList(d.query);
  const headers = rowsToList(d.headers);
  return clean({
    method: d.method,
    url: d.url.trim(),
    query: query.length ? query : undefined,
    headers: headers.length ? headers : undefined,
    body: bodyToCore(d.body),
    auth: authToCore(d.auth),
    settings: settingsToCore(d.settings, d.resolve),
  });
}

/** A copy without `undefined` values (they would otherwise reach JSON as missing keys anyway, but
 * comparisons of saved and edited files should not see them). */
export function clean<T extends object>(o: T): T {
  return Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined)) as T;
}

/** Whether two files are the same (for the "unsaved changes" marker). */
export function sameFile(a: RequestFile, b: RequestFile): boolean {
  return stableJson(a) === stableJson(b);
}

function stableJson(v: unknown): string {
  return JSON.stringify(v, (_, value: unknown) =>
    value && typeof value === "object" && !Array.isArray(value)
      ? Object.fromEntries(Object.entries(value as Record<string, unknown>).filter(([, x]) => x !== undefined))
      : value,
  );
}

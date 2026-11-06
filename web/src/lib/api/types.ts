// Shapes of the core JSON documents (requests, responses, traces, files). The OpenAPI document
// describes these as free-form objects (ADR-021), so they are written by hand here, following
// snapreq-core's and snapreq-collection's serde attributes (docs/FORMAT.md, docs/SERVER.md).

export interface KeyValue {
  name: string;
  value: string;
  /** Absent means `true`. */
  enabled?: boolean;
}

/** Bytes as JSON: UTF-8 text as-is, anything else base64. */
export interface RawBytes {
  encoding: "utf8" | "base64";
  data: string;
}

export type CoreBody =
  | { type: "none" }
  | { type: "text"; content: string; content_type?: string }
  | { type: "json"; content: string }
  | { type: "form"; fields: KeyValue[] }
  | { type: "multipart"; parts: CorePart[] }
  | { type: "file"; path: string; content_type?: string };

export type CorePart = { name: string; enabled?: boolean } & (
  | { kind: "text"; value: string }
  | { kind: "file"; path: string; filename?: string; content_type?: string }
);

export type CoreAuth =
  | { type: "none" }
  | { type: "basic" | "digest"; username: string; password?: string }
  | { type: "bearer"; token: string }
  | { type: "api_key"; name: string; value: string; location?: "header" | "query" }
  | ({ type: "oauth2" } & OAuth2Fields & { params?: KeyValue[] })
  | {
      type: "aws_sigv4";
      access_key: string;
      secret_key: string;
      session_token?: string;
      region: string;
      service: string;
    };

export interface OAuth2Fields {
  grant: "client_credentials" | "password" | "authorization_code";
  token_url: string;
  auth_url?: string;
  client_id: string;
  client_secret?: string;
  scope?: string;
  username?: string;
  password?: string;
  redirect_uri?: string;
  client_auth?: "basic" | "body";
}

export type HttpVersionPref = "auto" | "http1" | "http2";

export interface CoreSettings {
  timeout_ms?: number | null;
  connect_timeout_ms?: number | null;
  follow_redirects?: boolean;
  max_redirects?: number;
  tls?: {
    verify?: boolean;
    ca_cert?: string;
    client_cert?: { cert: string; key: string };
    min_version?: "1.2" | "1.3";
    max_version?: "1.2" | "1.3";
  };
  http_version?: HttpVersionPref;
  decompress?: boolean;
  cookies?: boolean;
  proxy?: string;
  resolve?: { host: string; port?: number; addrs: string[] }[];
  capture_wire?: boolean;
}

/** `snapreq_core::Request`. */
export interface CoreRequest {
  method: string;
  url: string;
  query?: KeyValue[];
  headers?: KeyValue[];
  body?: CoreBody;
  auth?: CoreAuth;
  settings?: CoreSettings;
}

export type HttpVersion = "HTTP/0.9" | "HTTP/1.0" | "HTTP/1.1" | "HTTP/2" | "HTTP/3";

export interface Header {
  name: string;
  value: string;
}

export interface Timings {
  dns_ms: number | null;
  connect_ms: number | null;
  tls_ms: number | null;
  send_ms: number | null;
  wait_ms: number | null;
  download_ms: number | null;
  total_ms: number;
}

export interface Sizes {
  request_headers: number;
  request_body: number;
  response_headers: number;
  response_body: number;
  response_body_decoded: number;
}

export interface CertificateInfo {
  subject: string;
  issuer: string;
  not_before: string;
  not_after: string;
  serial: string;
  subject_alt_names?: string[];
  sha256_fingerprint: string;
}

export interface TlsInfo {
  version: string;
  cipher: string;
  alpn?: string;
  peer_certificates?: CertificateInfo[];
}

export type Phase =
  | "prepare"
  | "dns"
  | "connect"
  | "tls"
  | "send"
  | "wait"
  | "download"
  | "redirect"
  | "auth";

export type TraceEvent =
  | { event: "dns_start"; host: string }
  | { event: "dns_end"; host: string; addrs: string[]; duration_ms: number }
  | { event: "connect_start"; addr: string }
  | { event: "connect_end"; addr: string; local_addr: string; duration_ms: number }
  | { event: "connection_reused"; addr: string }
  | { event: "tls_start"; server_name: string }
  | { event: "tls_end"; info: TlsInfo; duration_ms: number }
  | { event: "request_headers"; method: string; url: string; version: HttpVersion; headers: Header[] }
  | { event: "request_sent"; body_bytes: number; duration_ms: number }
  | { event: "response_first_byte"; wait_ms: number }
  | { event: "response_headers"; status: number; version: HttpVersion; headers: Header[] }
  | { event: "response_complete"; body_bytes: number; download_ms: number }
  | { event: "wire"; sent: RawBytes; received: RawBytes; truncated: boolean }
  | { event: "redirect"; status: number; from: string; to: string }
  | { event: "auth_challenge"; status: number; scheme: string }
  | { event: "info"; message: string }
  | { event: "error"; phase: Phase; message: string };

export type TraceEntry = { at_ms: number } & TraceEvent;

/** `snapreq_core::Response`. */
export interface CoreResponse {
  status: number;
  version: HttpVersion;
  url: string;
  headers: Header[];
  body: RawBytes;
  remote_addr?: string;
  timings: Timings;
  sizes: Sizes;
  trace: TraceEntry[];
}

export interface ExecutionError {
  kind: "execution" | "invalid_request";
  message: string;
  phase?: Phase;
  timed_out?: boolean;
  trace?: TraceEntry[];
}

export interface AssertionOutcome {
  subject: string;
  check: string;
  passed: boolean;
  actual?: string;
  message?: string;
}

export interface CaptureOutcome {
  name: string;
  value: string | null;
  secret: boolean;
  error?: string;
}

/** The result of `POST /requests/execute` (and a WebSocket `result` for `execute`). */
export interface ExecuteResult {
  history_id: number | null;
  response: CoreResponse | null;
  error: ExecutionError | null;
  assertions?: AssertionOutcome[];
  captures?: CaptureOutcome[];
  /** `console.log` output of the request's scripts. */
  logs?: string[];
}

// --- Collection files (docs/FORMAT.md) ---------------------------------------------------

/** A key/value table: `Name = "v"`, `Name = ["a", "b"]`, `Name = { value, enabled = false }`. */
export type KvTableValue = string | KvEntry | (string | KvEntry)[];
export interface KvEntry {
  value: string;
  enabled?: boolean;
}
export type KvTable = Record<string, KvTableValue>;

/** Variables: plain values or `{ secret = true }` markers. */
export type VarsTable = Record<string, string | { secret: true }>;

export type AuthSpec =
  | { type: "inherit" }
  | Exclude<CoreAuth, { type: "oauth2" }>
  | ({ type: "oauth2" } & OAuth2Fields & { params?: KvTable });

export type BodySpec =
  | { type: "text"; content?: string; file?: string; content_type?: string }
  | { type: "json"; content?: string; file?: string }
  | { type: "form"; fields?: KvTable }
  | { type: "multipart"; parts?: PartSpec[] }
  | { type: "file"; file: string; content_type?: string };

export interface PartSpec {
  name: string;
  value?: string;
  file?: string;
  filename?: string;
  content_type?: string;
  enabled?: boolean;
}

export interface SettingsSpec {
  timeout_ms?: number;
  connect_timeout_ms?: number;
  follow_redirects?: boolean;
  max_redirects?: number;
  decompress?: boolean;
  cookies?: boolean;
  http_version?: HttpVersionPref;
  proxy?: string;
  verify_tls?: boolean;
  ca_cert?: string;
  client_cert?: string;
  client_key?: string;
  tls_min_version?: "1.2" | "1.3";
  tls_max_version?: "1.2" | "1.3";
}

/** A request file as JSON. `capture` and `assert` are kept as they are. */
export interface RequestFile {
  name?: string;
  description?: string;
  method?: string;
  url: string;
  vars?: VarsTable;
  query?: KvTable;
  headers?: KvTable;
  auth?: AuthSpec;
  body?: BodySpec;
  settings?: SettingsSpec;
  capture?: Record<string, unknown>;
  assert?: Record<string, unknown>;
  script?: ScriptSpec;
  websocket?: WebSocketSpec;
}

/** `[websocket]`: makes a request a WebSocket (ADR-033). */
export interface WebSocketSpec {
  subprotocols?: string[];
  messages?: WsMessageSpec[];
}

export interface WsMessageSpec {
  name: string;
  /** Default `text`; `binary` content is base64. */
  kind?: "text" | "json" | "binary";
  content: string;
}

/** A WebSocket message as the server relays it (binary data is base64). */
export type WsMessage =
  | { kind: "text"; data: string }
  | { kind: "binary"; data: string }
  | { kind: "ping"; data: string }
  | { kind: "pong"; data: string }
  | { kind: "close"; code?: number; reason?: string };

/** `[script]`: JavaScript run before the request is built and after the response (ADR-031). */
export interface ScriptSpec {
  pre_request?: string;
  pre_request_file?: string;
  test?: string;
  test_file?: string;
}

export interface CollectionFile {
  format: number;
  name: string;
  description?: string;
  order?: string[];
  vars?: VarsTable;
  headers?: KvTable;
  auth?: AuthSpec;
  settings?: SettingsSpec;
  script?: ScriptSpec;
}

export type TreeItem =
  | { type: "request"; key: string; name: string; method: string }
  | { type: "folder"; key: string; name: string; items: TreeItem[] };

export interface FlowSummary {
  key: string;
  name: string;
  steps: number;
}

/** `GET /collections/{id}`. */
export interface CollectionDoc {
  id: string;
  file: CollectionFile;
  items: TreeItem[];
  flows: FlowSummary[];
  environments: string[];
}

export interface CollectionListEntry {
  id: string;
  name?: string;
  description?: string;
  requests?: number;
  flows?: number;
  error?: string;
}

export interface VariableInfo {
  name: string;
  value: string | null;
  secret: boolean;
  has_value: boolean;
  scope: string;
}

/** A flow step as JSON: a collection request, or an inline request (request file keys). */
export interface StepSpec extends Partial<RequestFile> {
  request?: string;
  name?: string;
  vars?: VarsTable;
  delay_ms?: number;
  retry?: { max?: number; every_ms?: number; until?: Record<string, unknown> };
}

export interface FlowFile {
  name?: string;
  description?: string;
  vars?: VarsTable;
  step: StepSpec[];
}

// --- History and runs --------------------------------------------------------------------

export interface HistorySummary {
  id: number;
  created_at: number;
  method: string;
  url: string;
  status: number | null;
  duration_ms: number | null;
  error: string | null;
  collection: string | null;
  request_key: string | null;
  /** The workspace of `collection`. */
  workspace?: string | null;
}

/** A stored request: redacted, with the auth reduced to its kind. */
export interface HistoryRecord extends HistorySummary {
  request: {
    method: string;
    url: string;
    /** The effective URL (query included), secrets hidden. */
    headers?: Header[];
    body?: CoreBody | null;
    /** The auth kind only (`bearer`, `basic`, ...). */
    auth?: string;
    [key: string]: unknown;
  };
  response: (Omit<CoreResponse, "body" | "remote_addr"> & { body: (RawBytes & { truncated?: boolean }) | null }) | null;
}

export interface RunSummary {
  id: number;
  created_at: number;
  workspace?: string | null;
  kind: string;
  target_key: string;
  name: string;
  passed: boolean;
}

export interface RunStep {
  number: number;
  title: string;
  key: string;
  passed: boolean;
  attempts: number;
  request: { method: string; url: string } | null;
  response: {
    status: number;
    duration_ms: number;
    version?: HttpVersion;
    headers?: Header[];
    body?: string;
    timings?: Timings;
    trace?: TraceEntry[];
  } | null;
  error: { kind: string; message: string; trace?: TraceEntry[] } | null;
  assertions: AssertionOutcome[];
  until: AssertionOutcome[];
  captures: CaptureOutcome[];
  logs?: string[];
}

export interface RunIteration {
  number: number;
  data: Record<string, string> | null;
  passed: boolean;
  total: number;
  skipped: number;
  steps: RunStep[];
}

/** A run report (`format = 1`); the summary from `POST /runs` adds `id` and `collection`. */
export interface RunReport {
  format: number;
  target: { kind: string; key: string; name: string };
  passed: boolean;
  planned_iterations: number;
  iterations: RunIteration[];
  id?: number;
  collection?: string;
}

/** Decodes a `RawBytes` value to text (binary bodies decode lossily). */
export function rawText(raw: RawBytes | null | undefined): string {
  if (!raw) return "";
  if (raw.encoding === "utf8") return raw.data;
  return new TextDecoder("utf-8", { fatal: false }).decode(rawBytes(raw));
}

/** Decodes a `RawBytes` value to bytes. */
export function rawBytes(raw: RawBytes | null | undefined): Uint8Array {
  if (!raw) return new Uint8Array();
  if (raw.encoding === "utf8") return new TextEncoder().encode(raw.data);
  const bin = atob(raw.data);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

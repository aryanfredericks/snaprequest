// Importing files from other tools (POST /api/v1/collections/import).

export type ImportFormat = "curl" | "postman" | "openapi" | "insomnia" | "har" | "hurl";

export const IMPORT_FORMATS: { value: ImportFormat; label: string }[] = [
  { value: "curl", label: "cURL command" },
  { value: "postman", label: "Postman collection (v2.1)" },
  { value: "openapi", label: "OpenAPI 3 (JSON or YAML)" },
  { value: "insomnia", label: "Insomnia export (v4 or v5)" },
  { value: "har", label: "HAR recording" },
  { value: "hurl", label: "Hurl file (as a flow)" },
];

export interface ImportRequest {
  content: string;
  format?: ImportFormat | null;
  id?: string | null;
  name?: string | null;
  environments?: string[];
}

export interface ImportSummary {
  id: string;
  format: ImportFormat;
  name: string;
  requests: number;
  folders: number;
  flows: number;
  environments: string[];
  secrets: number;
  warnings: string[];
}

/** The server refuses larger bodies. */
export const MAX_IMPORT_BYTES = 32 * 1024 * 1024;

/** "3 requests, 1 folder, 1 flow". */
export function describe(s: ImportSummary): string {
  const n = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;
  const parts = [n(s.requests, "request"), n(s.folders, "folder")];
  if (s.flows) parts.push(n(s.flows, "flow"));
  if (s.environments.length) parts.push(n(s.environments.length, "environment"));
  return parts.join(", ");
}

/** A body for the API: empty optional fields are left out. */
export function importBody(content: string, format: string, id: string, name: string, environments: string[]): ImportRequest {
  const body: ImportRequest = { content };
  if (format) body.format = format as ImportFormat;
  if (id.trim()) body.id = id.trim();
  if (name.trim()) body.name = name.trim();
  if (environments.length) body.environments = environments;
  return body;
}

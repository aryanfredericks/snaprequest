// Requests as code (POST /api/v1/requests/export).

export type ExportFormat = "curl" | "python" | "javascript" | "go" | "rust";

export const EXPORT_FORMATS: { value: ExportFormat; label: string; lang: "javascript" | "text" }[] = [
  { value: "curl", label: "cURL", lang: "text" },
  { value: "python", label: "Python (requests)", lang: "text" },
  { value: "javascript", label: "JavaScript (fetch)", lang: "javascript" },
  { value: "go", label: "Go (net/http)", lang: "text" },
  { value: "rust", label: "Rust (reqwest)", lang: "text" },
];

const KEY = "snapreq.exportFormat";

/** The format chosen last time (cURL at first). */
export function lastFormat(storage: Pick<Storage, "getItem"> = localStorage): ExportFormat {
  const v = storage.getItem(KEY);
  return EXPORT_FORMATS.some((f) => f.value === v) ? (v as ExportFormat) : "curl";
}

export function rememberFormat(format: ExportFormat, storage: Pick<Storage, "setItem"> = localStorage) {
  storage.setItem(KEY, format);
}

// Editable models for the settings of a collection (`snapreq.toml`), a folder (`folder.toml`)
// and an environment (`environments/<name>.toml`).
import type { AuthSpec, CollectionFile, SettingsSpec, VarsTable } from "../api/types";
import {
  authFromSpec,
  authToSpec,
  clean,
  rowsFromTable,
  rowsToTable,
  sameFile,
  uid,
  type AuthDraft,
  type Row,
} from "./draft";

export interface VarRow {
  id: number;
  name: string;
  value: string;
  secret: boolean;
}

export const varRow = (name = "", value = "", secret = false): VarRow => ({ id: uid(), name, value, secret });

export function varsFromTable(t: VarsTable | undefined): VarRow[] {
  return Object.entries(t ?? {}).map(([name, v]) =>
    typeof v === "string" ? varRow(name, v) : varRow(name, "", true),
  );
}

/** Rows as a table; secret rows become `{ secret = true }` markers (their values live elsewhere). */
export function varsToTable(rows: VarRow[]): VarsTable | undefined {
  const out: VarsTable = {};
  for (const r of rows) {
    const name = r.name.trim();
    if (!name) continue;
    out[name] = r.secret ? { secret: true } : r.value;
  }
  return Object.keys(out).length ? out : undefined;
}

/** Variable names: letters, digits, `_`, `-` and `.`, not starting with `$` (docs/FORMAT.md). */
export const validVarName = (n: string) => /^[A-Za-z0-9_.-]+$/.test(n);

/** `snapreq.toml` (`format` set) or `folder.toml` as JSON. */
export type ScopeFile = Partial<CollectionFile> & { name?: string };

export interface ScopeDraft {
  name: string;
  description: string;
  vars: VarRow[];
  headers: Row[];
  auth: AuthDraft;
  settings: SettingsSpec;
  /** Kept as they are: `format`, `order`. */
  rest: { format?: number; order?: string[] };
}

export function scopeFromFile(f: ScopeFile, isCollection: boolean): ScopeDraft {
  const auth = authFromSpec(f.auth);
  // A collection has nothing to inherit from: no auth means none.
  if (isCollection && auth.type === "inherit") auth.type = "none";
  return {
    name: f.name ?? "",
    description: f.description ?? "",
    vars: varsFromTable(f.vars),
    headers: rowsFromTable(f.headers),
    auth,
    settings: { ...(f.settings ?? {}) },
    rest: clean({ format: f.format, order: f.order }),
  };
}

export function scopeToFile(d: ScopeDraft, isCollection: boolean): ScopeFile {
  let auth: AuthSpec | undefined = authToSpec(d.auth);
  if (isCollection && auth?.type === "none") auth = undefined;
  const settings = clean({ ...d.settings });
  return clean({
    format: d.rest.format ?? (isCollection ? 1 : undefined),
    name: d.name.trim() || (isCollection ? "Untitled collection" : undefined),
    description: d.description.trim() || undefined,
    order: d.rest.order?.length ? d.rest.order : undefined,
    vars: varsToTable(d.vars),
    headers: rowsToTable(d.headers),
    auth,
    settings: Object.keys(settings).length ? settings : undefined,
  });
}

export const sameScope = (a: ScopeFile, b: ScopeFile) => sameFile(a as never, b as never);

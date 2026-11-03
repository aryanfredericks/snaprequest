// `{{name}}` placeholders (docs/FORMAT.md, "Placeholders"), for highlighting and hints.
import type { VariableInfo } from "../api/types";

export const DYNAMIC = ["$uuid", "$timestamp", "$isoTimestamp", "$randomInt"] as const;

export interface Placeholder {
  /** Offsets of `{{` and just after `}}`. */
  from: number;
  to: number;
  name: string;
}

/** Every placeholder in `text`; `\{{` is a literal and skipped. */
export function placeholders(text: string): Placeholder[] {
  const out: Placeholder[] = [];
  const re = /\{\{\s*([^{}\s]+)\s*\}\}/g;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > 0 && text[m.index - 1] === "\\") continue;
    out.push({ from: m.index, to: m.index + m[0].length, name: m[1]! });
  }
  return out;
}

export type VarState = "defined" | "secret" | "dynamic" | "undefined" | "unknown";

/**
 * How a placeholder resolves. Without a variable list (a request outside a collection) every
 * name is `unknown`: it is sent as written.
 */
export function varState(name: string, vars: VariableInfo[] | null): VarState {
  if ((DYNAMIC as readonly string[]).includes(name)) return "dynamic";
  if (!vars) return "unknown";
  const v = vars.find((x) => x.name === name);
  if (!v) return "undefined";
  return v.secret ? "secret" : "defined";
}

/** A one-line description for a tooltip: the value (never a secret's) and its scope. */
export function describeVar(name: string, vars: VariableInfo[] | null): string {
  const state = varState(name, vars);
  switch (state) {
    case "dynamic":
      return `${name}: generated for every request`;
    case "unknown":
      return `${name}: sent as written (not in a collection)`;
    case "undefined":
      return `${name}: not defined in any scope`;
  }
  const v = vars!.find((x) => x.name === name)!;
  if (state === "secret") return `${name}: secret (${v.has_value ? "set" : "no value"}) from ${v.scope}`;
  return `${name} = ${v.value ?? ""} (${v.scope})`;
}

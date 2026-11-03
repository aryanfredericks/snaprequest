// The flow editor's model: steps of a `*.flow.toml` file, each a collection request or an
// inline request (docs/FORMAT.md, "Flows").
import type { FlowFile, RequestFile, StepSpec } from "../api/types";
import { clean, draftFromFile, draftToFile, emptyDraft, uid, type Draft } from "./draft";
import { varsFromTable, varsToTable, type VarRow } from "./scope";

export interface Retry {
  max?: number;
  every_ms?: number;
  until?: Record<string, unknown>;
}

export interface StepDraft {
  id: number;
  kind: "request" | "inline";
  name: string;
  /** Request steps: the collection request's key. */
  request: string;
  /** Inline steps: the request. Both kinds keep `vars`, `capture` and `assert` in `draft.rest`. */
  draft: Draft;
  delayMs: number | null;
  retry: Retry | null;
}

export function stepFromSpec(s: StepSpec): StepDraft {
  const { name, request, delay_ms, retry, vars, capture, assert, ...inline } = s;
  const draft = request
    ? emptyDraft(true)
    : draftFromFile({ ...(inline as RequestFile), url: inline.url ?? "" });
  draft.name = "";
  draft.rest = clean({ vars, capture, assert });
  return {
    id: uid(),
    kind: request ? "request" : "inline",
    name: name ?? "",
    request: request ?? "",
    draft,
    delayMs: delay_ms ?? null,
    retry: retry ?? null,
  };
}

export function specFromStep(s: StepDraft): StepSpec {
  const common = clean({
    name: s.name.trim() || undefined,
    vars: s.draft.rest.vars,
    capture: s.draft.rest.capture,
    assert: s.draft.rest.assert,
    delay_ms: s.delayMs ?? undefined,
    retry: s.retry ?? undefined,
  });
  if (s.kind === "request") return clean({ request: s.request.trim(), ...common });
  const { name: _n, description: _d, vars: _v, capture: _c, assert: _a, ...req } = draftToFile(s.draft);
  return clean({ ...common, ...req, name: common.name });
}

export function newStep(kind: StepDraft["kind"], request = ""): StepDraft {
  const draft = emptyDraft(true);
  if (kind === "inline") draft.url = "{{base_url}}/";
  return { id: uid(), kind, name: "", request, draft, delayMs: null, retry: null };
}

export interface FlowDraft {
  name: string;
  description: string;
  vars: VarRow[];
  steps: StepDraft[];
}

export function flowFromFile(f: FlowFile): FlowDraft {
  return {
    name: f.name ?? "",
    description: f.description ?? "",
    vars: varsFromTable(f.vars),
    steps: (f.step ?? []).map(stepFromSpec),
  };
}

export function flowToFile(d: FlowDraft): FlowFile {
  return clean({
    name: d.name.trim() || undefined,
    description: d.description.trim() || undefined,
    vars: varsToTable(d.vars),
    step: d.steps.map(specFromStep),
  });
}

/** A step's title as the run report shows it: its name, the request key, or `METHOD url`. */
export function stepTitle(s: StepDraft): string {
  if (s.name.trim()) return s.name.trim();
  if (s.kind === "request") return s.request || "(no request)";
  return `${s.draft.method} ${s.draft.url}`;
}

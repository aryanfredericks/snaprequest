// CodeMirror setup shared by every editor: theme from the page's CSS variables, languages,
// and `{{variable}}` highlighting with hover descriptions.
import { autocompletion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { html } from "@codemirror/lang-html";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { xml } from "@codemirror/lang-xml";
import {
  bracketMatching,
  foldGutter,
  foldKeymap,
  HighlightStyle,
  indentOnInput,
  syntaxHighlighting,
} from "@codemirror/language";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension, StateEffect, StateField } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  hoverTooltip,
  keymap,
  lineNumbers,
  placeholder as placeholderExt,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";
import { tags as t } from "@lezer/highlight";
import type { VariableInfo } from "../api/types";
import { describeVar, DYNAMIC, placeholders, varState } from "../model/vars";

/** `graphql` is loaded on demand by the editor component (see `editor/graphql.ts`). */
export type Language = "json" | "xml" | "html" | "javascript" | "graphql" | "text";

export function language(lang: Language): Extension {
  switch (lang) {
    case "json":
      return json();
    case "xml":
      return xml();
    case "html":
      return html();
    case "javascript":
      return javascript();
    default:
      return [];
  }
}

/** The language for a content type (`application/problem+json` → json). */
export function languageFor(contentType: string | undefined): Language {
  const ct = (contentType ?? "").toLowerCase();
  if (/[/+]json\b/.test(ct)) return "json";
  if (ct.includes("html")) return "html";
  if (/[/+]xml\b/.test(ct)) return "xml";
  if (ct.includes("javascript") || ct.includes("ecmascript")) return "javascript";
  return "text";
}

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.bool, t.null], color: "var(--syn-keyword)" },
  { tag: [t.string, t.special(t.string)], color: "var(--syn-string)" },
  { tag: [t.number], color: "var(--syn-number)" },
  { tag: [t.propertyName, t.attributeName], color: "var(--syn-property)" },
  { tag: [t.tagName, t.typeName], color: "var(--syn-tag)" },
  { tag: [t.comment], color: "var(--muted)", fontStyle: "italic" },
  { tag: [t.punctuation, t.bracket], color: "var(--fg-soft)" },
]);

export const theme = EditorView.theme({
  "&": { color: "var(--fg)", backgroundColor: "var(--bg-input)", fontSize: "0.85rem" },
  ".cm-content": { fontFamily: "var(--mono)", caretColor: "var(--fg)" },
  // CodeMirror's default placeholder grey is below 4.5:1 on the active-line background.
  ".cm-placeholder": { color: "var(--muted)" },
  ".cm-gutters": { backgroundColor: "var(--bg-soft)", color: "var(--muted)", border: "none" },
  ".cm-activeLine, .cm-activeLineGutter": { backgroundColor: "var(--bg-hover)" },
  "&.cm-focused": { outline: "2px solid var(--focus)", outlineOffset: "-1px" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--selection) !important",
  },
  ".cm-searchMatch": { backgroundColor: "var(--search-match)", outline: "1px solid var(--warn)" },
  ".cm-searchMatch-selected": { backgroundColor: "var(--search-current)" },
  ".cm-panels": { backgroundColor: "var(--bg-soft)", color: "var(--fg)" },
  ".cm-panels input, .cm-panels button": { font: "inherit", fontSize: "0.8rem" },
  ".cm-tooltip": { backgroundColor: "var(--bg-soft)", border: "1px solid var(--border)", color: "var(--fg)" },
  ".cm-var": { borderRadius: "3px", padding: "0 1px" },
  ".cm-var-defined, .cm-var-dynamic": { color: "var(--ok)", backgroundColor: "var(--ok-bg)" },
  ".cm-var-secret": { color: "var(--secret)", backgroundColor: "var(--secret-bg)" },
  ".cm-var-undefined": { color: "var(--danger)", backgroundColor: "var(--danger-bg)", textDecoration: "underline wavy" },
  ".cm-var-unknown": { color: "var(--accent)", backgroundColor: "var(--accent-bg)" },
  ".cm-var-tip": { padding: "0.25rem 0.5rem", fontFamily: "var(--mono)", fontSize: "0.8rem" },
});

// --- Variables --------------------------------------------------------------------------

/** Replaces the variables the highlighting resolves against (`null`: not in a collection). */
export const setVars = StateEffect.define<VariableInfo[] | null>();

const varsField = StateField.define<VariableInfo[] | null>({
  create: () => null,
  update: (value, tr) => tr.effects.reduce((v, e) => (e.is(setVars) ? e.value : v), value),
});

function decorate(view: EditorView): DecorationSet {
  const vars = view.state.field(varsField);
  const marks = [];
  for (const { from, to } of view.visibleRanges) {
    const text = view.state.doc.sliceString(from, to);
    for (const p of placeholders(text)) {
      marks.push(Decoration.mark({ class: `cm-var cm-var-${varState(p.name, vars)}` }).range(from + p.from, from + p.to));
    }
  }
  return Decoration.set(marks, true);
}

const varPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = decorate(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged || u.transactions.some((tr) => tr.effects.some((e) => e.is(setVars)))) {
        this.decorations = decorate(u.view);
      }
    }
  },
  { decorations: (v) => v.decorations },
);

const varTooltip = hoverTooltip((view, pos) => {
  const line = view.state.doc.lineAt(pos);
  const p = placeholders(line.text).find((x) => line.from + x.from <= pos && pos <= line.from + x.to);
  if (!p) return null;
  return {
    pos: line.from + p.from,
    end: line.from + p.to,
    above: true,
    create: () => {
      const dom = document.createElement("div");
      dom.className = "cm-var-tip";
      dom.textContent = describeVar(p.name, view.state.field(varsField));
      return { dom };
    },
  };
});

function completeVars(ctx: CompletionContext): CompletionResult | null {
  const before = ctx.matchBefore(/\{\{\s*[^{}\s]*$/);
  if (!before) return null;
  const vars = ctx.state.field(varsField) ?? [];
  const start = before.from + before.text.search(/[^{\s]|$/);
  return {
    from: start,
    options: [
      ...vars.map((v) => ({
        label: v.name,
        detail: v.secret ? "secret" : (v.value ?? ""),
        info: v.scope,
        apply: `${v.name}}}`,
      })),
      ...DYNAMIC.map((d) => ({ label: d, detail: "dynamic", apply: `${d}}}` })),
    ],
    validFor: /^[^{}\s]*$/,
  };
}

export function variables(): Extension {
  return [varsField, varPlugin, varTooltip, autocompletion({ override: [completeVars], activateOnTyping: true })];
}

// --- Editors ----------------------------------------------------------------------------

export interface EditorOptions {
  doc: string;
  lang?: Language;
  readonly?: boolean;
  /** Single line: Enter does not insert a newline (`onEnter` is called), no gutters. */
  singleLine?: boolean;
  placeholder?: string;
  vars?: boolean;
  label?: string;
  onChange?: (value: string) => void;
  onEnter?: () => void;
}

/** CodeMirror's keys without Mod-Enter, which the app uses for "send" (it would insert a line). */
const editorKeymap = defaultKeymap.filter((b) => b.key !== "Mod-Enter");

export const languageSlot = new Compartment();
export const readonlySlot = new Compartment();

export function createState(o: EditorOptions): EditorState {
  const ext: Extension[] = [
    theme,
    syntaxHighlighting(highlight),
    history(),
    drawSelection(),
    languageSlot.of(language(o.lang ?? "text")),
    // Read-only documents stay focusable, so they can be searched and selected.
    readonlySlot.of(EditorState.readOnly.of(!!o.readonly)),
    EditorView.contentAttributes.of({ "aria-label": o.label ?? "editor", ...(o.singleLine ? { "aria-multiline": "false" } : {}) }),
    search({ top: true }),
    highlightSelectionMatches(),
  ];
  if (o.vars) ext.push(variables());
  if (o.placeholder) ext.push(placeholderExt(o.placeholder));
  if (o.singleLine) {
    ext.push(
      // Newlines typed or pasted become spaces.
      EditorState.transactionFilter.of((tr) => {
        if (!tr.docChanged || !tr.newDoc.toString().includes("\n")) return tr;
        return [tr, { changes: { from: 0, to: tr.newDoc.length, insert: tr.newDoc.toString().replace(/\r?\n/g, " ") }, sequential: true }];
      }),
      keymap.of([
        { key: "Enter", run: () => (o.onEnter?.(), true) },
        ...historyKeymap,
        ...editorKeymap.filter((b) => b.key !== "Enter"),
      ]),
      EditorView.theme({ ".cm-content": { padding: "0.3rem 0" }, ".cm-line": { padding: "0 0.4rem" } }),
    );
  } else {
    ext.push(
      lineNumbers(),
      foldGutter(),
      highlightActiveLine(),
      highlightActiveLineGutter(),
      bracketMatching(),
      indentOnInput(),
      EditorView.lineWrapping,
      keymap.of([...editorKeymap, ...historyKeymap, ...foldKeymap, ...searchKeymap, indentWithTab]),
    );
  }
  if (o.onChange) {
    const cb = o.onChange;
    ext.push(EditorView.updateListener.of((u) => u.docChanged && cb(u.state.doc.toString())));
  }
  return EditorState.create({ doc: o.doc, extensions: ext });
}

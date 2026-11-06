<script lang="ts">
  import { onMount } from "svelte";
  import { EditorView } from "@codemirror/view";
  import { EditorState } from "@codemirror/state";
  import type { VariableInfo } from "../api/types";
  import { createState, language, languageSlot, readonlySlot, setVars, type Language } from "../editor/cm";

  interface Props {
    value: string;
    lang?: Language;
    readonly?: boolean;
    singleLine?: boolean;
    /** Highlight `{{placeholders}}` against these (null: outside a collection). */
    vars?: VariableInfo[] | null;
    highlightVars?: boolean;
    placeholder?: string;
    label: string;
    onchange?: (value: string) => void;
    onenter?: () => void;
    /** For `graphql`: a schema's introspection `__schema` object (completion and checks). */
    schema?: unknown;
    /** Receives the editor view (for commands such as search). */
    onview?: (view: EditorView) => void;
    class?: string;
  }

  let {
    value,
    lang = "text",
    readonly = false,
    singleLine = false,
    vars = null,
    highlightVars = false,
    placeholder,
    label,
    onchange,
    onenter,
    onview,
    schema = null,
    class: cls = "",
  }: Props = $props();

  let host: HTMLDivElement;
  let view: EditorView | undefined;

  onMount(() => {
    view = new EditorView({
      parent: host,
      state: createState({
        doc: value,
        lang,
        readonly,
        singleLine,
        placeholder,
        label,
        vars: highlightVars,
        onChange: (v) => onchange?.(v),
        onEnter: () => onenter?.(),
      }),
    });
    if (highlightVars) view.dispatch({ effects: setVars.of(vars) });
    onview?.(view);
    return () => view?.destroy();
  });

  // Outside changes (switching tabs, formatting, a new response).
  $effect(() => {
    const v = value;
    if (view && v !== view.state.doc.toString()) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: v } });
    }
  });
  $effect(() => {
    const l = lang;
    const s = schema;
    if (l !== "graphql") {
      view?.dispatch({ effects: languageSlot.reconfigure(language(l)) });
      return;
    }
    let current = true;
    void import("../editor/graphql").then((m) => {
      if (current) view?.dispatch({ effects: languageSlot.reconfigure(m.graphqlLanguage(s ? m.clientSchema(s) : null)) });
    });
    return () => (current = false);
  });
  $effect(() => {
    const r = readonly;
    view?.dispatch({ effects: readonlySlot.reconfigure(EditorState.readOnly.of(r)) });
  });
  $effect(() => {
    const v = vars;
    if (highlightVars) view?.dispatch({ effects: setVars.of(v) });
  });
</script>

<div class="code {cls}" class:single={singleLine} bind:this={host}></div>

<style>
  .code {
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .code:not(.single) {
    display: flex;
    min-height: 8rem;
  }
  .code :global(.cm-editor) {
    flex: 1;
    min-width: 0;
    max-height: 100%;
  }
  .code :global(.cm-scroller) {
    overflow: auto;
  }
  .single :global(.cm-scroller) {
    overflow-x: auto;
    overflow-y: hidden;
  }
</style>

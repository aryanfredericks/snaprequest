<script lang="ts">
  // Edits a JSON value as text; the value only changes while the text parses.
  import CodeEditor from "./CodeEditor.svelte";

  interface Props {
    value: unknown;
    label: string;
    onchange: (value: unknown) => void;
    placeholder?: string;
  }
  let { value, label, onchange, placeholder }: Props = $props();

  const show = (v: unknown) => (v === undefined ? "" : JSON.stringify(v, null, 2));
  let text = $state("");
  let error = $state<string | null>(null);
  let shown: unknown = Symbol();
  $effect.pre(() => {
    if (value !== shown) {
      shown = value;
      text = show(value);
      error = null;
    }
  });

  function edit(t: string) {
    text = t;
    if (!t.trim()) {
      error = null;
      shown = undefined;
      onchange(undefined);
      return;
    }
    try {
      const v: unknown = JSON.parse(t);
      error = null;
      shown = v;
      onchange(v);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }
</script>

<CodeEditor value={text} lang="json" {label} {placeholder} onchange={edit} />
{#if error}<p class="err" role="status">Not saved until it is valid JSON: {error}</p>{/if}

<style>
  .err {
    color: var(--warn);
    font-size: 0.8rem;
    margin: 0.2rem 0 0;
  }
</style>

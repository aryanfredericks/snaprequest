<script lang="ts">
  import type { VariableInfo } from "../api/types";
  import { row, type Row } from "../model/draft";
  import CodeEditor from "./CodeEditor.svelte";

  interface Props {
    rows: Row[];
    label: string;
    vars?: VariableInfo[] | null;
    names?: readonly string[];
    namePlaceholder?: string;
    valuePlaceholder?: string;
  }
  let { rows = $bindable(), label, vars = null, names = [], namePlaceholder = "name", valuePlaceholder = "value" }: Props = $props();

  let bulk = $state(false);
  let bulkText = $state("");
  const listId = `kv-names-${Math.random().toString(36).slice(2)}`;

  function toggleBulk() {
    if (!bulk) {
      bulkText = rows.map((r) => `${r.enabled ? "" : "# "}${r.name}: ${r.value}`).join("\n");
    } else {
      rows = parseBulk(bulkText);
    }
    bulk = !bulk;
  }

  /** `name: value` per line; `# ` disables a line. */
  function parseBulk(text: string): Row[] {
    return text
      .split("\n")
      .filter((l) => l.trim())
      .map((l) => {
        const disabled = /^\s*#\s?/.test(l);
        const line = l.replace(/^\s*#\s?/, "");
        const i = line.indexOf(":");
        return i < 0 ? row(line.trim(), "", !disabled) : row(line.slice(0, i).trim(), line.slice(i + 1).trim(), !disabled);
      });
  }
</script>

<div class="kv">
  <div class="tools">
    <button type="button" class="link" onclick={toggleBulk}>{bulk ? "Table" : "Bulk edit"}</button>
  </div>
  {#if bulk}
    <textarea
      class="bulk"
      aria-label="{label} as text, one name: value per line"
      bind:value={bulkText}
      onblur={() => (rows = parseBulk(bulkText))}
      rows={Math.max(4, rows.length + 2)}
    ></textarea>
  {:else}
    <table aria-label={label}>
      <thead>
        <tr><th class="on"><span class="sr-only">Enabled</span></th><th>Name</th><th>Value</th><th class="del"><span class="sr-only">Remove</span></th></tr>
      </thead>
      <tbody>
        {#each rows as r, i (r.id)}
          <tr class:off={!r.enabled}>
            <td class="on"><input type="checkbox" bind:checked={r.enabled} aria-label="Send {r.name || `row ${i + 1}`}" /></td>
            <td><input class="name" bind:value={r.name} placeholder={namePlaceholder} list={names.length ? listId : undefined} aria-label="{label} name {i + 1}" /></td>
            <td>
              <CodeEditor
                value={r.value}
                singleLine
                highlightVars
                {vars}
                placeholder={valuePlaceholder}
                label="{label} value {i + 1}"
                onchange={(v) => (r.value = v)}
              />
            </td>
            <td class="del"><button type="button" class="icon" onclick={() => rows.splice(i, 1)} aria-label="Remove {r.name || `row ${i + 1}`}">✕</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
    <button type="button" class="add" onclick={() => rows.push(row())}>+ Add</button>
  {/if}
  {#if names.length}
    <datalist id={listId}>
      {#each names as n (n)}<option value={n}></option>{/each}
    </datalist>
  {/if}
</div>

<style>
  .kv {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .tools {
    display: flex;
    justify-content: flex-end;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
  }
  th {
    text-align: left;
    font-weight: 500;
    font-size: 0.75rem;
    color: var(--muted);
    padding: 0.2rem 0.3rem;
  }
  td {
    padding: 0.15rem 0.3rem;
    vertical-align: middle;
  }
  .on {
    width: 1.8rem;
  }
  .del {
    width: 2rem;
  }
  th:nth-child(2) {
    width: 35%;
  }
  tr.off .name,
  tr.off :global(.cm-content) {
    opacity: 0.55;
    text-decoration: line-through;
  }
  .name {
    width: 100%;
  }
  .bulk {
    width: 100%;
    font-family: var(--mono);
    font-size: 0.85rem;
  }
  .add {
    align-self: flex-start;
  }
</style>

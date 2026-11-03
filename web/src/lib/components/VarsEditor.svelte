<script lang="ts">
  import { validVarName, varRow, type VarRow } from "../model/scope";

  interface Props {
    rows: VarRow[];
    label: string;
    /** Whether each secret has a stored value (name → set). */
    secretsSet?: Record<string, boolean>;
    /** Stores a value for a secret (a dialog). */
    onsetsecret?: (name: string) => void;
    secretHint: string;
  }
  let { rows = $bindable(), label, secretsSet = {}, onsetsecret, secretHint }: Props = $props();
</script>

<table aria-label={label}>
  <thead>
    <tr><th>Name</th><th>Value</th><th class="sec">Secret</th><th class="del"><span class="sr-only">Remove</span></th></tr>
  </thead>
  <tbody>
    {#each rows as r, i (r.id)}
      {@const bad = r.name.trim() !== "" && !validVarName(r.name.trim())}
      <tr>
        <td>
          <input bind:value={r.name} placeholder="name" aria-label="{label} name {i + 1}" aria-invalid={bad} class:bad />
        </td>
        <td>
          {#if r.secret}
            <span class="secret">
              {#if r.name.trim() && secretsSet[r.name.trim()] !== undefined}
                <span class={secretsSet[r.name.trim()] ? "s-2" : "warn"}>{secretsSet[r.name.trim()] ? "value set" : "no value yet"}</span>
              {:else}
                <span class="muted">value kept outside the file</span>
              {/if}
              {#if onsetsecret && r.name.trim() && !bad}
                <button type="button" class="link" onclick={() => onsetsecret(r.name.trim())}>Set value…</button>
              {/if}
            </span>
          {:else}
            <input bind:value={r.value} placeholder="value" aria-label="{label} value {i + 1}" />
          {/if}
        </td>
        <td class="sec"><input type="checkbox" bind:checked={r.secret} aria-label="{r.name || `row ${i + 1}`} is secret" /></td>
        <td class="del"><button type="button" class="icon" onclick={() => rows.splice(i, 1)} aria-label="Remove {r.name || `row ${i + 1}`}">✕</button></td>
      </tr>
    {/each}
  </tbody>
</table>
<button type="button" class="add" onclick={() => rows.push(varRow())}>+ Add variable</button>
<p class="muted hint">{secretHint}</p>

<style>
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }
  th {
    text-align: left;
    font-weight: 500;
    font-size: 0.75rem;
    color: var(--muted);
    padding: 0.2rem 0.3rem;
  }
  th:first-child {
    width: 30%;
  }
  td {
    padding: 0.15rem 0.3rem;
  }
  td input:not([type="checkbox"]) {
    width: 100%;
    font-family: var(--mono);
  }
  .sec {
    width: 4rem;
    text-align: center;
  }
  .del {
    width: 2rem;
  }
  .secret {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    font-size: 0.8rem;
  }
  .warn {
    color: var(--warn);
  }
  .bad {
    border-color: var(--danger);
  }
  .add {
    margin-top: 0.3rem;
  }
  .hint {
    font-size: 0.78rem;
  }
</style>

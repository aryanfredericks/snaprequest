<script lang="ts">
  import type { Header } from "../api/types";

  let { headers, label }: { headers: Header[]; label: string } = $props();
  let filter = $state("");
  const shown = $derived(
    filter.trim()
      ? headers.filter((h) => `${h.name}: ${h.value}`.toLowerCase().includes(filter.trim().toLowerCase()))
      : headers,
  );
</script>

<div class="headers">
  {#if headers.length > 8}
    <input type="search" bind:value={filter} placeholder="Filter headers" aria-label="Filter {label}" />
  {/if}
  <table aria-label={label}>
    <thead><tr><th>Name</th><th>Value</th></tr></thead>
    <tbody>
      {#each shown as h, i (i)}
        <tr><td class="n">{h.name}</td><td class="v">{h.value}</td></tr>
      {:else}
        <tr><td colspan="2" class="muted">No headers</td></tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .headers {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  input {
    max-width: 20rem;
  }
  table {
    border-collapse: collapse;
    width: 100%;
    font-size: 0.82rem;
  }
  th {
    text-align: left;
    font-weight: 500;
    color: var(--muted);
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
  }
  td {
    padding: 0.25rem 0.5rem;
    border-top: 1px solid var(--border);
    vertical-align: top;
  }
  .n {
    font-weight: 600;
    white-space: nowrap;
    width: 1%;
  }
  .v {
    font-family: var(--mono);
    word-break: break-all;
  }
</style>

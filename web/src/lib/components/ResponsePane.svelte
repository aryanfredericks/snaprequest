<script lang="ts">
  import type { RequestTab } from "../state/app.svelte";
  import { rawText } from "../api/types";

  interface Props {
    tab: RequestTab;
  }
  let { tab }: Props = $props();
  const r = $derived(tab.result?.response ?? null);
</script>

<section class="response" aria-label="Response" aria-busy={tab.running}>
  {#if tab.running}
    <p class="muted">Sending… {tab.live.length} trace events</p>
  {:else if tab.error}
    <p class="s-err" role="alert">{tab.error}</p>
  {:else if tab.result?.error}
    <p class="s-err">{tab.result.error.message}</p>
  {:else if r}
    <p><strong class="s-{Math.floor(r.status / 100)}">{r.status}</strong> · {r.timings.total_ms.toFixed(0)} ms</p>
    <pre>{rawText(r.body)}</pre>
  {:else}
    <p class="muted">Send the request to see the response.</p>
  {/if}
</section>

<style>
  .response {
    overflow: auto;
    height: 100%;
  }
  pre {
    font-family: var(--mono);
    font-size: 0.8rem;
    white-space: pre-wrap;
  }
</style>

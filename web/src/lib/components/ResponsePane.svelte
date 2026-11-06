<script lang="ts">
  import type { RequestTab } from "../state/app.svelte";
  import { isWebSocket } from "../model/draft";
  import ResultView from "./ResultView.svelte";
  import WebSocketPane from "./WebSocketPane.svelte";

  let { tab }: { tab: RequestTab } = $props();
  const result = $derived(tab.result);
</script>

{#if isWebSocket(tab.draft)}
  <WebSocketPane {tab} />
{:else}
<ResultView
  response={result?.response ?? null}
  failure={result?.error ?? null}
  problem={tab.error}
  assertions={result?.assertions ?? []}
  captures={result?.captures ?? []}
  logs={result?.logs ?? []}
  running={tab.running}
  live={tab.live}
  name={tab.draft.name || tab.key?.split("/").pop() || "response"}
  idPrefix="res-{tab.id}"
/>
{/if}

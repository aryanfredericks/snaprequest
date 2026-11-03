<script lang="ts">
  import { rawBytes, rawText, type RawBytes } from "../api/types";
  import { formatBytes } from "../model/http";
  import CodeEditor from "./CodeEditor.svelte";
  import HexView from "./HexView.svelte";

  let { sent, received, truncated }: { sent: RawBytes; received: RawBytes; truncated: boolean } = $props();
  let hex = $state(false);
  const binary = $derived(sent.encoding !== "utf8" || received.encoding !== "utf8");
  const show = (r: RawBytes) => rawText(r).replace(/\r\n/g, "␍␊\n");
</script>

<div class="wire">
  <div class="bar">
    <span class="muted">
      sent {formatBytes(rawBytes(sent).length)} · received {formatBytes(rawBytes(received).length)}{truncated ? " · shortened (the capture keeps the first part of each direction)" : ""}
    </span>
    <label class="check"><input type="checkbox" bind:checked={hex} /> hex</label>
  </div>
  <div class="cols">
    <section aria-label="Bytes sent">
      <h4>→ Sent</h4>
      {#if hex || binary}<HexView bytes={rawBytes(sent)} />{:else}<CodeEditor value={show(sent)} readonly label="Bytes sent" />{/if}
    </section>
    <section aria-label="Bytes received">
      <h4>← Received</h4>
      {#if hex || binary}<HexView bytes={rawBytes(received)} />{:else}<CodeEditor value={show(received)} readonly label="Bytes received" />{/if}
    </section>
  </div>
  <p class="muted small">Credentials are redacted in the capture. Line ends are shown as ␍␊.</p>
</div>

<style>
  .wire {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .bar {
    display: flex;
    justify-content: space-between;
    font-size: 0.8rem;
  }
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.6rem;
  }
  section {
    min-width: 0;
    display: flex;
    flex-direction: column;
    max-height: 28rem;
  }
  h4 {
    margin: 0 0 0.3rem;
    font-size: 0.8rem;
  }
  .small {
    font-size: 0.75rem;
    margin: 0;
  }
</style>

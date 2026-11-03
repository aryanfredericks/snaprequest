<script lang="ts">
  import { hexRow } from "../model/http";
  import VirtualList from "./VirtualList.svelte";

  let { bytes }: { bytes: Uint8Array } = $props();
</script>

<div class="hex">
  <VirtualList count={Math.ceil(bytes.length / 16)} rowHeight={20} label="Hex dump">
    {#snippet row(i)}
      {@const r = hexRow(bytes, i)}
      <span class="off">{r.offset}</span>  <span>{r.hex}</span>  <span class="txt">{r.text}</span>
    {/snippet}
  </VirtualList>
</div>

<style>
  .hex {
    font-family: var(--mono);
    font-size: 0.8rem;
    line-height: 20px;
    height: 100%;
    min-height: 10rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0 0.5rem;
  }
  .off {
    color: var(--muted);
  }
  .txt {
    color: var(--syn-string);
  }
</style>

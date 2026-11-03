<script lang="ts" generics="T">
  // Renders only the rows in view (fixed row height), for lists of any length.
  import type { Snippet } from "svelte";

  interface Props {
    count: number;
    rowHeight: number;
    row: Snippet<[number]>;
    label: string;
    overscan?: number;
  }
  let { count, rowHeight, row, label, overscan = 10 }: Props = $props();

  let scrollTop = $state(0);
  let height = $state(400);
  const first = $derived(Math.max(0, Math.floor(scrollTop / rowHeight) - overscan));
  const last = $derived(Math.min(count, Math.ceil((scrollTop + height) / rowHeight) + overscan));
  const visible = $derived(Array.from({ length: Math.max(0, last - first) }, (_, i) => first + i));
</script>

<!-- Scrollable content must be reachable with the keyboard (WCAG 2.1.1), hence the tabindex. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="viewport"
  role="list"
  aria-label={label}
  tabindex="0"
  bind:clientHeight={height}
  onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
>
  <div class="spacer" style="height: {count * rowHeight}px">
    <div class="rows" style="transform: translateY({first * rowHeight}px)">
      {#each visible as i (i)}
        <div class="row" role="listitem" style="height: {rowHeight}px">{@render row(i)}</div>
      {/each}
    </div>
  </div>
</div>

<style>
  .viewport {
    overflow: auto;
    height: 100%;
    min-height: 0;
    contain: strict;
  }
  .spacer {
    position: relative;
  }
  .rows {
    position: absolute;
    inset: 0 0 auto 0;
  }
  .row {
    overflow: hidden;
    white-space: pre;
  }
</style>

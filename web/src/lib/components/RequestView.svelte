<script lang="ts">
  import type { RequestTab } from "../state/app.svelte";
  import RequestBuilder from "./RequestBuilder.svelte";
  import ResponsePane from "./ResponsePane.svelte";

  interface Props {
    tab: RequestTab;
  }
  let { tab }: Props = $props();

  let builder: RequestBuilder | undefined = $state();
  /** Share of the height given to the builder. */
  let split = $state(Number(localStorage.getItem("snapreq.split")) || 0.45);
  let box: HTMLDivElement;

  export function save() {
    builder?.save();
  }

  function drag(e: PointerEvent) {
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      const rect = box.getBoundingClientRect();
      split = Math.min(0.85, Math.max(0.15, (ev.clientY - rect.top) / rect.height));
    };
    const up = () => {
      el.removeEventListener("pointermove", move);
      localStorage.setItem("snapreq.split", String(split));
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up, { once: true });
  }

  function key(e: KeyboardEvent) {
    if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      e.preventDefault();
      split = Math.min(0.85, Math.max(0.15, split + (e.key === "ArrowUp" ? -0.05 : 0.05)));
      localStorage.setItem("snapreq.split", String(split));
    }
  }
</script>

<div class="view" bind:this={box} style="grid-template-rows: {split}fr auto {1 - split}fr">
  <div class="pane"><RequestBuilder {tab} bind:this={builder} /></div>
  <!-- A focusable separator is an interactive ARIA widget (arrow keys resize). -->
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    class="splitter"
    role="separator"
    aria-orientation="horizontal"
    aria-label="Resize request and response"
    aria-valuenow={Math.round(split * 100)}
    aria-valuemin={15}
    aria-valuemax={85}
    tabindex="0"
    onpointerdown={drag}
    onkeydown={key}
  ></div>
  <div class="pane"><ResponsePane {tab} /></div>
</div>

<style>
  .view {
    display: grid;
    height: 100%;
    min-height: 0;
  }
  .pane {
    min-height: 0;
    overflow: hidden;
    padding: 0.6rem 0.9rem;
  }
  .splitter {
    height: 6px;
    cursor: row-resize;
    border-top: 1px solid var(--border);
    background: transparent;
  }
  .splitter:hover,
  .splitter:focus-visible {
    background: var(--accent-bg);
  }
</style>

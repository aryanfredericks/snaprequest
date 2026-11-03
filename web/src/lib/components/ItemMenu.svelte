<script lang="ts" module>
  export interface MenuItem {
    label: string;
    action: () => void;
    danger?: boolean;
    disabled?: boolean;
  }
</script>

<script lang="ts">
  // A "⋯" button with a menu (WAI-ARIA menu button: arrows, Home/End, Escape).
  import { tick } from "svelte";

  interface Props {
    items: MenuItem[];
    label: string;
  }
  let { items, label }: Props = $props();
  let open = $state(false);
  let button: HTMLButtonElement;
  let menu: HTMLUListElement | undefined = $state();

  async function show() {
    open = true;
    await tick();
    focusItem(0);
  }

  function hide(refocus = true) {
    open = false;
    if (refocus) button.focus();
  }

  function focusItem(i: number) {
    const els = [...(menu?.querySelectorAll<HTMLButtonElement>("[role=menuitem]:not(:disabled)") ?? [])];
    if (els.length) els[(i + els.length) % els.length]!.focus();
  }

  function key(e: KeyboardEvent) {
    const els = [...(menu?.querySelectorAll<HTMLButtonElement>("[role=menuitem]:not(:disabled)") ?? [])];
    const i = els.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "ArrowDown") focusItem(i + 1);
    else if (e.key === "ArrowUp") focusItem(i - 1);
    else if (e.key === "Home") focusItem(0);
    else if (e.key === "End") focusItem(-1);
    else if (e.key === "Escape" || e.key === "Tab") hide(e.key === "Escape");
    else return;
    e.preventDefault();
    e.stopPropagation();
  }

  function run(item: MenuItem) {
    hide();
    item.action();
  }
</script>

<span class="wrap">
  <button
    type="button"
    class="icon more"
    aria-label={label}
    aria-haspopup="menu"
    aria-expanded={open}
    bind:this={button}
    onclick={(e) => (e.stopPropagation(), open ? hide() : show())}
  >⋯</button>
  {#if open}
    <ul class="menu" role="menu" aria-label={label} bind:this={menu} onkeydown={key} onfocusout={(e) => !menu?.contains(e.relatedTarget as Node) && e.relatedTarget !== button && hide(false)}>
      {#each items as item (item.label)}
        <li role="none">
          <button type="button" role="menuitem" class:danger={item.danger} disabled={item.disabled} tabindex="-1" onclick={(e) => (e.stopPropagation(), run(item))}>
            {item.label}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</span>

<style>
  .wrap {
    position: relative;
  }
  .more {
    opacity: 0;
    padding: 0 0.35rem;
  }
  :global(.row-wrap:hover) .more,
  :global(.row-wrap:focus-within) .more,
  .more[aria-expanded="true"] {
    opacity: 1;
  }
  .menu {
    position: absolute;
    right: 0;
    top: 100%;
    z-index: 20;
    list-style: none;
    margin: 0;
    padding: 0.25rem;
    min-width: 11rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 6px 18px rgb(0 0 0 / 0.18);
  }
  .menu button {
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    padding: 0.3rem 0.6rem;
  }
  .menu button:hover,
  .menu button:focus {
    background: var(--bg-hover);
  }
  .danger {
    color: var(--danger);
  }
</style>

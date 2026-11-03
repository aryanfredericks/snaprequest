<script lang="ts">
  import { untrack } from "svelte";
  import { app, type RequestTab } from "../state/app.svelte";

  interface Props {
    tab: RequestTab;
    onclose: () => void;
  }
  let { tab, onclose }: Props = $props();

  let dialog: HTMLDialogElement;
  // Initial values only: the dialog belongs to one tab.
  let collection = $state(untrack(() => tab.collection ?? app.collections[0]?.id ?? ""));
  let key = $state(
    untrack(() => tab.key ?? slug(tab.draft.name || tab.draft.url.replace(/^.*\/\/[^/]*/, "") || "new-request")),
  );
  let busy = $state(false);

  function slug(s: string) {
    return s.toLowerCase().replace(/\{\{[^}]*\}\}/g, "").replace(/[^a-z0-9/_-]+/g, "-").replace(/^[-/]+|[-/]+$/g, "") || "new-request";
  }

  $effect(() => {
    dialog.showModal();
    void app.loadCollections();
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    if (await app.saveAs(tab, collection, key)) onclose();
    busy = false;
  }
</script>

<dialog bind:this={dialog} onclose={onclose} aria-labelledby="save-title">
  <form onsubmit={submit}>
    <h2 id="save-title">Save request</h2>
    {#if app.collections.length === 0}
      <p>There are no collections yet. Create one in the sidebar first.</p>
    {:else}
      <label>Collection
        <select bind:value={collection} required>
          {#each app.collections as c (c.id)}<option value={c.id}>{c.name ?? c.id}</option>{/each}
        </select>
      </label>
      <label>Key (folders separated by <code>/</code>)
        <input bind:value={key} required pattern="[^\s]+(/[^\s]+)*" placeholder="users/create-user" />
      </label>
    {/if}
    <div class="actions">
      <button type="button" onclick={() => dialog.close()}>Cancel</button>
      <button type="submit" class="primary" disabled={busy || app.collections.length === 0}>Save</button>
    </div>
  </form>
</dialog>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
    min-width: 22rem;
  }
  h2 {
    margin: 0;
    font-size: 1rem;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.85rem;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }
</style>

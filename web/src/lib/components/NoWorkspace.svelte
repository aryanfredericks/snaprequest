<script lang="ts">
  import { auth } from "../state/auth.svelte";
  import { workspaces } from "../state/workspaces.svelte";

  let id = $state("");
  let name = $state("");
  let error = $state<string | null>(null);

  async function create(e: SubmitEvent) {
    e.preventDefault();
    try {
      await workspaces.create(id.trim(), name.trim());
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    }
  }
</script>

<main class="none">
  <form onsubmit={create} aria-labelledby="none-title">
    <h1 id="none-title">No workspace yet</h1>
    <p>
      Hello {auth.user?.username}. You do not belong to any workspace. Ask a workspace owner to add you, or create
      your own.
    </p>
    <label for="none-id">Id (directory)</label>
    <input id="none-id" bind:value={id} required pattern="[A-Za-z0-9_\-][A-Za-z0-9_.\-]*" placeholder="my-apis" />
    <label for="none-name">Name</label>
    <input id="none-name" bind:value={name} maxlength="100" placeholder="My APIs" />
    {#if error}<p class="err" role="alert">{error}</p>{/if}
    <button type="submit" class="primary">Create workspace</button>
    <button type="button" onclick={() => void auth.logout()}>Log out</button>
  </form>
</main>

<style>
  .none {
    display: grid;
    place-items: center;
    height: 100%;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    width: min(26rem, 90vw);
    padding: 1.6rem;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-soft);
  }
  h1 {
    margin: 0;
    font-size: 1.1rem;
  }
  p {
    color: var(--fg-soft);
    font-size: 0.9rem;
  }
  label {
    font-size: 0.85rem;
    margin-top: 0.3rem;
  }
  button {
    margin-top: 0.5rem;
  }
  .err {
    color: var(--danger);
  }
</style>

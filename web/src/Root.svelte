<script lang="ts">
  // Shows the login when the server has accounts and nobody is logged in, a "no workspace" page
  // for users without one, and the app otherwise.
  import App from "./App.svelte";
  import LoginView from "./lib/components/LoginView.svelte";
  import NoWorkspace from "./lib/components/NoWorkspace.svelte";
  import { auth } from "./lib/state/auth.svelte";
  import { workspaces } from "./lib/state/workspaces.svelte";

  $effect(() => {
    void auth.check();
  });

  // Once logged in: which workspaces the user has (and which one the UI works in).
  $effect(() => {
    if (auth.ready && !workspaces.loaded) void workspaces.load().catch((e: unknown) => (auth.error = String(e)));
  });
</script>

{#if auth.ready && workspaces.loaded && workspaces.current}
  <App />
{:else if auth.ready && workspaces.loaded && workspaces.list.length === 0}
  <NoWorkspace />
{:else if auth.mode === "accounts"}
  <LoginView />
{:else if auth.error}
  <main class="down">
    <p role="alert">The SnapRequest server is unreachable: {auth.error}</p>
    <button type="button" onclick={() => void auth.check()}>Try again</button>
  </main>
{/if}

<style>
  .down {
    display: grid;
    place-content: center;
    gap: 0.8rem;
    height: 100%;
    color: var(--muted);
  }
</style>

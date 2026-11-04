<script lang="ts">
  // Shows the login when the server has accounts and nobody is logged in, the app otherwise.
  import App from "./App.svelte";
  import LoginView from "./lib/components/LoginView.svelte";
  import { auth } from "./lib/state/auth.svelte";

  $effect(() => {
    void auth.check();
  });
</script>

{#if auth.ready}
  <App />
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

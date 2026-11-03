<script lang="ts">
  import { api, serverVersion, type ServerVersion } from "./lib/api/client";

  let version = $state<ServerVersion | null>(null);
  let error = $state<string | null>(null);

  $effect(() => {
    serverVersion(api)
      .then((v) => (version = v))
      .catch((e: unknown) => (error = e instanceof Error ? e.message : String(e)));
  });
</script>

<header>
  <h1>SnapRequest</h1>
  <span class="status" aria-live="polite">
    {#if version}
      server {version.version} · API v{version.api}
    {:else if error}
      <span class="error">server unreachable: {error}</span>
    {:else}
      connecting…
    {/if}
  </span>
</header>

<main>
  <p>The request builder, response viewer and debug view arrive in the next checkpoints.</p>
</main>

<style>
  header {
    display: flex;
    align-items: baseline;
    gap: 1rem;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid var(--border);
  }
  h1 {
    margin: 0;
    font-size: 1.1rem;
  }
  .status {
    color: var(--muted);
    font-size: 0.85rem;
  }
  .error {
    color: var(--danger);
  }
  main {
    padding: 1rem;
  }
</style>

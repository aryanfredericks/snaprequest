<script lang="ts">
  import { auth } from "../state/auth.svelte";

  let username = $state("");
  let password = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    busy = true;
    error = null;
    try {
      await auth.login(username.trim(), password);
      password = "";
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      busy = false;
    }
  }
</script>

<main class="login">
  <form onsubmit={submit} aria-labelledby="login-title">
    <h1 id="login-title"><span class="logo" aria-hidden="true">⚡</span> SnapRequest</h1>
    {#if auth.expired}
      <p class="note" role="status">Your session has ended. Log in again to continue; your open tabs are kept.</p>
    {/if}
    <label for="login-user">User name</label>
    <!-- svelte-ignore a11y_autofocus -->
    <input id="login-user" bind:value={username} autocomplete="username" autocapitalize="none" required autofocus />
    <label for="login-password">Password</label>
    <input id="login-password" type="password" bind:value={password} autocomplete="current-password" required />
    {#if error}<p class="err" role="alert">{error}</p>{/if}
    <button type="submit" class="primary" disabled={busy}>{busy ? "Logging in…" : "Log in"}</button>
    <p class="hint">Accounts are created on the server with <code>snapreq user add</code>.</p>
  </form>
</main>

<style>
  .login {
    display: grid;
    place-items: center;
    height: 100%;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    width: min(22rem, 90vw);
    padding: 1.6rem;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-soft);
  }
  h1 {
    margin: 0 0 0.8rem;
    font-size: 1.2rem;
  }
  .logo {
    color: var(--accent);
  }
  label {
    font-size: 0.85rem;
    color: var(--fg-soft);
    margin-top: 0.3rem;
  }
  button {
    margin-top: 0.8rem;
  }
  .err {
    color: var(--danger);
    margin: 0.3rem 0 0;
    font-size: 0.88rem;
  }
  .note {
    margin: 0 0 0.4rem;
    padding: 0.5rem 0.6rem;
    border-radius: 6px;
    background: var(--accent-bg);
    font-size: 0.85rem;
  }
  .hint {
    margin: 0.8rem 0 0;
    color: var(--muted);
    font-size: 0.8rem;
  }
</style>

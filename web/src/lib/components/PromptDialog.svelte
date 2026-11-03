<script lang="ts">
  import { prompt } from "../state/prompt.svelte";

  let dialog: HTMLDialogElement;
  let values = $state<Record<string, string>>({});
  const p = $derived(prompt.current);

  $effect(() => {
    if (p) {
      values = Object.fromEntries(p.fields.map((f) => [f.name, f.value ?? f.options?.[0]?.value ?? ""]));
      if (!dialog.open) dialog.showModal();
    } else if (dialog.open) {
      dialog.close();
    }
  });

  function submit(e: SubmitEvent) {
    e.preventDefault();
    prompt.finish({ ...values });
  }
</script>

<dialog bind:this={dialog} aria-labelledby="prompt-title" onclose={() => prompt.current && prompt.finish(null)}>
  {#if p}
    <form onsubmit={submit}>
      <h2 id="prompt-title">{p.title}</h2>
      {#if p.message}<p class="msg">{p.message}</p>{/if}
      {#each p.fields as f, i (f.name)}
        <div class="field">
          <label for="prompt-{f.name}">{f.label}</label>
          {#if f.type === "select"}
            <select id="prompt-{f.name}" bind:value={values[f.name]}>
              {#each f.options ?? [] as o (o.value)}<option value={o.value}>{o.label}</option>{/each}
            </select>
          {:else}
            <!-- svelte-ignore a11y_autofocus -->
            <input
              id="prompt-{f.name}"
              type={f.type ?? "text"}
              bind:value={values[f.name]}
              required={f.required ?? true}
              pattern={f.pattern}
              placeholder={f.placeholder}
              autocomplete="off"
              autofocus={i === 0}
            />
          {/if}
          {#if f.hint}<span class="hint">{f.hint}</span>{/if}
        </div>
      {/each}
      <div class="actions">
        <button type="button" onclick={() => prompt.finish(null)}>Cancel</button>
        <button type="submit" class={p.danger ? "danger" : "primary"}>{p.submit}</button>
      </div>
    </form>
  {/if}
</dialog>

<style>
  form {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
    min-width: 22rem;
    max-width: 32rem;
  }
  h2 {
    margin: 0;
    font-size: 1rem;
  }
  .msg {
    margin: 0;
    font-size: 0.9rem;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.85rem;
  }
  .hint {
    color: var(--muted);
    font-size: 0.75rem;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }
  .danger {
    background: var(--danger);
    border-color: var(--danger);
    color: #fff;
    font-weight: 600;
  }
</style>

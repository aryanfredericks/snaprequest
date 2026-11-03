<script lang="ts">
  // The HTTP method: one of the standard methods. A method from a file outside the list (such as
  // PROPFIND) is kept as an option, so opening the file does not change it.
  import { METHODS } from "../model/draft";

  let { value = $bindable(), label }: { value: string; label: string } = $props();
  const options = $derived((METHODS as readonly string[]).includes(value) ? METHODS : [...METHODS, value]);
</script>

<select class="method m-{value.toLowerCase()}" bind:value aria-label={label}>
  {#each options as m (m)}<option value={m}>{m}</option>{/each}
</select>

<style>
  .method {
    font-family: var(--mono);
    font-weight: 600;
  }
</style>

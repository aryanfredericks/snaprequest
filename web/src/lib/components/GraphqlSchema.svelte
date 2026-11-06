<script lang="ts">
  // A GraphQL schema from introspection: root operations first, then every type, searchable.
  interface TypeRef {
    kind: string;
    name: string | null;
    ofType?: TypeRef | null;
  }
  interface InputValue {
    name: string;
    description?: string | null;
    type: TypeRef;
    defaultValue?: string | null;
  }
  interface Field {
    name: string;
    description?: string | null;
    args?: InputValue[];
    type: TypeRef;
    isDeprecated?: boolean;
  }
  interface FullType {
    kind: string;
    name: string;
    description?: string | null;
    fields?: Field[] | null;
    inputFields?: InputValue[] | null;
    enumValues?: { name: string; description?: string | null }[] | null;
    possibleTypes?: TypeRef[] | null;
  }
  interface Schema {
    queryType?: { name: string } | null;
    mutationType?: { name: string } | null;
    subscriptionType?: { name: string } | null;
    types: FullType[];
  }

  interface Props {
    schema: unknown;
    sdl: string;
  }
  let { schema, sdl }: Props = $props();

  const s = $derived(schema as Schema);
  let filter = $state("");
  let open = $state<Record<string, boolean>>({});
  let copied = $state(false);

  const ref = (t: TypeRef | null | undefined): string =>
    !t ? "?" : t.kind === "NON_NULL" ? `${ref(t.ofType)}!` : t.kind === "LIST" ? `[${ref(t.ofType)}]` : (t.name ?? "?");
  const named = (t: TypeRef | null | undefined): string | null => (!t ? null : t.name ?? named(t.ofType));

  const roots = $derived(
    [
      ["query", s.queryType?.name],
      ["mutation", s.mutationType?.name],
      ["subscription", s.subscriptionType?.name],
    ].filter((r): r is [string, string] => !!r[1]),
  );
  const rootNames = $derived(new Set(roots.map((r) => r[1])));
  const types = $derived(
    s.types
      .filter((t) => !t.name.startsWith("__"))
      .sort((a, b) => Number(rootNames.has(b.name)) - Number(rootNames.has(a.name)))
      .filter((t) => {
        const f = filter.trim().toLowerCase();
        if (!f) return true;
        return t.name.toLowerCase().includes(f) || (t.fields ?? t.inputFields ?? []).some((x) => x.name.toLowerCase().includes(f));
      }),
  );

  function show(name: string) {
    open[name] = true;
    filter = "";
    queueMicrotask(() => document.getElementById(`gql-type-${name}`)?.scrollIntoView({ block: "nearest" }));
  }

  async function copy() {
    await navigator.clipboard?.writeText(sdl).catch(() => {});
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

<section class="schema" aria-label="GraphQL schema">
  <div class="bar">
    <strong>Schema</strong>
    <span class="muted">{s.types.filter((t) => !t.name.startsWith("__")).length} types · {roots.map((r) => `${r[0]}: ${r[1]}`).join(", ")}</span>
    <input type="search" bind:value={filter} placeholder="Find a type or field" aria-label="Find in the schema" />
    <button type="button" onclick={copy}>{copied ? "Copied" : "Copy SDL"}</button>
  </div>
  <ul class="types" aria-label="Schema types">
    {#each types as t (t.name)}
      {@const members = t.fields ?? t.inputFields ?? []}
      <li id="gql-type-{t.name}">
        <button type="button" class="link name" aria-expanded={!!open[t.name] || !!filter.trim()} onclick={() => (open[t.name] = !open[t.name])}>
          <span class="kind">{t.kind.toLowerCase().replace("_object", "").replace("object", "type")}</span>
          {t.name}
        </button>
        {#if t.description}<span class="muted desc">{t.description}</span>{/if}
        {#if open[t.name] || filter.trim()}
          <ul class="fields">
            {#each members as f (f.name)}
              {@const target = named(f.type)}
              <li class:deprecated={"isDeprecated" in f && f.isDeprecated}>
                <code>{f.name}{"args" in f && f.args?.length ? `(${f.args.map((a) => `${a.name}: ${ref(a.type)}`).join(", ")})` : ""}:</code>
                {#if target && s.types.some((x) => x.name === target && x.kind !== "SCALAR")}
                  <button type="button" class="link" onclick={() => show(target)}>{ref(f.type)}</button>
                {:else}
                  <code>{ref(f.type)}</code>
                {/if}
                {#if f.description}<span class="muted desc">{f.description}</span>{/if}
              </li>
            {/each}
            {#each t.enumValues ?? [] as v (v.name)}
              <li><code>{v.name}</code>{#if v.description}<span class="muted desc">{v.description}</span>{/if}</li>
            {/each}
            {#each t.possibleTypes ?? [] as p (p.name)}
              <li><button type="button" class="link" onclick={() => show(p.name ?? "")}>{p.name}</button></li>
            {/each}
          </ul>
        {/if}
      </li>
    {/each}
  </ul>
</section>

<style>
  .schema {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0.4rem;
    flex: 0 0 auto;
    max-height: 22rem;
    overflow: hidden;
  }
  .bar {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    font-size: 0.85rem;
  }
  .bar input {
    flex: 1;
  }
  .types {
    flex: 1;
    min-height: 0;
    list-style: none;
    margin: 0;
    padding: 0;
    overflow: auto;
    font-size: 0.82rem;
  }
  .types > li {
    padding: 0.15rem 0;
    border-bottom: 1px solid var(--border);
  }
  .name {
    font-weight: 600;
  }
  .kind {
    color: var(--muted);
    font-weight: normal;
    margin-right: 0.3rem;
  }
  .fields {
    list-style: none;
    margin: 0.2rem 0 0.3rem 1rem;
    padding: 0;
  }
  .fields li {
    display: flex;
    gap: 0.35rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .deprecated code:first-child {
    text-decoration: line-through;
  }
  code {
    font-family: var(--mono);
  }
  .desc {
    font-size: 0.78rem;
  }
</style>

<script lang="ts">
  // The `[grpc]` table of a request: method (from the server's services), proto files and the
  // request message(s) as JSON.
  import type { GrpcMethod, GrpcService, GrpcSpec } from "../api/types";
  import { formatJson } from "../model/format";
  import CodeEditor from "./CodeEditor.svelte";

  interface Props {
    spec: GrpcSpec | undefined;
    onchange: (spec: GrpcSpec) => void;
    services: GrpcService[] | null;
    describeState: { busy: boolean; error: string | null };
    ondescribe: () => void;
    inCollection: boolean;
  }
  let { spec, onchange, services, describeState, ondescribe, inCollection }: Props = $props();

  const s = $derived(spec ?? {});
  const methods = $derived((services ?? []).flatMap((x) => x.methods));
  const method = $derived<GrpcMethod | undefined>(methods.find((m) => m.path === s.method));
  const streaming = $derived(method?.client_streaming ?? (s.messages?.length ?? 0) > 0);
  const kind = $derived(
    method ? (method.client_streaming ? (method.server_streaming ? "bidirectional streaming" : "client streaming") : method.server_streaming ? "server streaming" : "unary") : "",
  );
  /** The editor text: one message, or a JSON array of messages for client streaming. */
  const text = $derived(
    streaming ? (s.messages?.length ? `[\n${s.messages.map((m) => `  ${m.trim()}`).join(",\n")}\n]` : "") : (s.message ?? ""),
  );

  function update(patch: Partial<GrpcSpec>) {
    const next: GrpcSpec = { ...s, ...patch };
    for (const k of Object.keys(next) as (keyof GrpcSpec)[]) {
      const v = next[k];
      if (v === undefined || v === "" || (Array.isArray(v) && v.length === 0)) delete next[k];
    }
    onchange(next);
  }

  function setText(v: string) {
    if (!streaming) {
      update({ message: v, messages: undefined });
      return;
    }
    try {
      const list: unknown = JSON.parse(v.replace(/\{\{[^{}]*\}\}/g, "0"));
      if (Array.isArray(list)) {
        // Keep each message's own text (placeholders included) by re-reading the array.
        const parsed = JSON.parse(v) as unknown[];
        update({ messages: parsed.map((m) => JSON.stringify(m)), message: undefined });
        return;
      }
    } catch {
      // Not (yet) an array: keep typing.
    }
    update({ messages: [v], message: undefined });
  }

  const listOf = (v: string) =>
    v
      .split(",")
      .map((x) => x.trim())
      .filter(Boolean);

  function template() {
    if (!method) return;
    const one = JSON.stringify(method.template, null, 2);
    setText(streaming ? `[${one}]` : one);
  }

  let problem = $derived.by(() => {
    if (!text.trim()) return null;
    try {
      JSON.parse(text.replace(/\{\{[^{}]*\}\}/g, "0"));
      return null;
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    }
  });
</script>

<div class="grpc">
  <div class="row">
    <label class="method">
      Method
      <input
        value={s.method ?? ""}
        list="grpc-methods"
        placeholder="package.Service/Method"
        aria-label="gRPC method"
        onchange={(e) => update({ method: e.currentTarget.value.trim() })}
      />
    </label>
    <datalist id="grpc-methods">
      {#each methods as m (m.path)}<option value={m.path}>{m.path}</option>{/each}
    </datalist>
    <button type="button" onclick={ondescribe} disabled={describeState.busy} title="List the server's services (reflection, or the proto files)">
      {describeState.busy ? "Loading…" : services ? "Reload services" : "Load services"}
    </button>
  </div>
  {#if describeState.error}<p class="warn" role="alert">{describeState.error}</p>{/if}
  {#if services}
    <div class="services" role="group" aria-label="Services">
      {#each services as svc (svc.name)}
        <div class="svc">
          <strong>{svc.name}</strong>
          {#each svc.methods as m (m.path)}
            <button type="button" class="link" aria-pressed={m.path === s.method} onclick={() => update({ method: m.path })}>{m.name}</button>
          {/each}
        </div>
      {/each}
    </div>
  {/if}
  {#if method}
    <p class="muted small">{kind}: {method.input} → {method.output}</p>
  {/if}
  {#if inCollection}
    <div class="row">
      <label class="grow">
        Proto files
        <input value={(s.protos ?? []).join(", ")} placeholder="empty: server reflection; e.g. protos/shop.proto" aria-label="Proto files" onchange={(e) => update({ protos: listOf(e.currentTarget.value) })} />
      </label>
      <label class="grow">
        Import paths
        <input value={(s.import_paths ?? []).join(", ")} placeholder="protos" aria-label="Import paths" onchange={(e) => update({ import_paths: listOf(e.currentTarget.value) })} />
      </label>
    </div>
  {/if}
  <div class="row">
    <h4>{streaming ? "Messages (a JSON array, sent in order)" : "Message"} <span class="muted">(proto3 JSON; placeholders allowed)</span></h4>
    <button type="button" onclick={template} disabled={!method}>Insert template</button>
    <button type="button" onclick={() => setText(formatJson(text))} disabled={!!problem || !text.trim()}>Format</button>
  </div>
  <CodeEditor value={text} lang="json" label="gRPC message" placeholder={'{ "id": 1 }'} onchange={setText} class="grow" />
  {#if problem}<p class="warn" role="status">Not valid JSON: {problem}</p>{/if}
</div>

<style>
  .grpc {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    min-height: 0;
  }
  .row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    flex-wrap: wrap;
    font-size: 0.85rem;
  }
  .method,
  .grow {
    flex: 1;
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .method input,
  .grow input {
    flex: 1;
    font-family: var(--mono);
  }
  .services {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.82rem;
  }
  .svc {
    display: flex;
    gap: 0.6rem;
    flex-wrap: wrap;
    align-items: baseline;
  }
  .svc [aria-pressed="true"] {
    font-weight: 700;
  }
  h4 {
    margin: 0;
    font-size: 0.85rem;
    flex: 1;
  }
  .small {
    font-size: 0.78rem;
    margin: 0;
  }
  .warn {
    color: var(--warn);
    margin: 0;
    font-size: 0.82rem;
  }
</style>

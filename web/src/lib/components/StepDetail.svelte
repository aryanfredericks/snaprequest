<script lang="ts">
  import type { RunStep } from "../api/types";
  import { stepResponse } from "../model/runs";
  import ChecksView from "./ChecksView.svelte";
  import ResultView from "./ResultView.svelte";

  interface Props {
    step: RunStep;
    idPrefix: string;
  }
  let { step, idPrefix }: Props = $props();
  const response = $derived(stepResponse(step));
  const failure = $derived(
    step.error
      ? {
          kind: (step.error.kind === "invalid_request" ? "invalid_request" : "execution") as "execution" | "invalid_request",
          message: step.error.message,
          trace: step.error.trace,
        }
      : null,
  );
</script>

<div class="detail">
  <div class="head">
    <strong>Step {step.number}: {step.title}</strong>
    {#if step.request}<code class="req">{step.request.method} {step.request.url}</code>{/if}
    {#if step.attempts > 1}<span class="muted">{step.attempts} attempts</span>{/if}
  </div>
  <div class="checks">
    <ChecksView assertions={step.assertions} until={step.until} captures={step.captures} />
  </div>
  <div class="result">
    <ResultView {response} {failure} idPrefix="{idPrefix}-{step.number}" label="Step {step.number} response" name="step-{step.number}" empty="No response." />
  </div>
</div>

<style>
  .detail {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    min-height: 0;
  }
  .head {
    display: flex;
    gap: 0.8rem;
    align-items: baseline;
    flex-wrap: wrap;
    font-size: 0.9rem;
  }
  .req {
    word-break: break-all;
  }
  .result {
    min-height: 22rem;
    display: flex;
    flex-direction: column;
  }
</style>

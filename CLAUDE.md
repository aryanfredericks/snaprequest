# SnapRequest

Open-source, self-hosted HTTP API testing and debugging tool in Rust (CLI + self-hosted server + web UI).

## Start of every session
1. Read `docs/CHECKPOINT.md`: current phase, current checkpoint, open decisions, next steps.
2. Read the matching phase in `docs/PLAN.md` for the checkpoint's scope and acceptance criteria.
3. Check `docs/DECISIONS.md` before making architectural choices; add a new ADR for any new one.

## Working rules
- One checkpoint at a time. A checkpoint is done only when its acceptance criteria pass and
  `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` are clean.
- Tests ship together with features.
- `snapreq-core` must stay free of UI, CLI and storage concerns.
- Libraries use `thiserror`; binaries use `anyhow`. Logging through `tracing`.
- Never log or write secret variable values; redact them.

## End of every session
Update `docs/CHECKPOINT.md`: current checkpoint, completed checkpoints with dates, next steps, and a session log line.

## Workflow agreed with the maintainer
When a checkpoint passes all its checks, commit and push to `origin main` without asking, update
`docs/CHECKPOINT.md`, and continue with the next checkpoint. Only stop to ask when genuinely blocked or
when a real product decision is needed. Commits carry no AI co-author or attribution lines.

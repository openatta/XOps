---
name: xforge-status
description: Explain xforge state as readable progress — either the portfolio of in-flight Changes and the Stage each sits at, or one Change in depth; use when the user asks what is in flight, what is done, why work is blocked, which packages remain, whether Evidence is current, or whether Verify/Archive is ready.
allowed-tools: Read, Grep, Glob, Bash(xforge:*)
---

# Invariants

- Run `xforge state` for the portfolio view, and `xforge state --change <id>` for one Change in depth. State is the only progress source of truth.
- Read the in-flight list from `activeChanges`; it already excludes archived Changes. Never rebuild that list by walking `xforge/changes` yourself, and never infer a Stage from directory contents, commit messages, or chat memory — the Stage of record is the one State reports.
- A Change listed with a null Flow or Stage failed to resolve. **Report it as unresolved rather than omitting it** — a Change that will not load is the one most likely to need attention, and a listing that silently drops it is worse than one that shows a gap.
- Remain strictly read-only; do not maintain a second progress ledger or continue/fix/check off work incidentally.

# Authority

- Query, filter, and explain State, the in-flight Change portfolio, work packages, deliveries, diagnostics, and Evidence freshness.
- Do not modify project files, generate Evidence, execute a ready Action, or archive.

# Execution

1. **Portfolio view (default when no Change is named).** Report every in-flight Change from `activeChanges`: id, Flow, current Stage, and risk. Order by Stage so what is closest to done reads first, and state the count. When the list is empty, say so plainly — an empty portfolio is an answer, not a failure.
2. **Single-Change view.** Resolve the Change ID and request a choice if ownership is ambiguous, then report Flow, current Stage/revision, ready/blocked Transitions, pending Approvals, Rule coverage, active Policy/Hook coverage, Audit chain/remote pending/gaps, work-package lifecycle/deliveries, Evidence freshness, and Verify/Archive readiness.
3. Give the next legal Action, owning Skill, and the reason it is or is not ready. **Name the Skill; do not execute it** — reporting readiness and taking the step are different authorities.
4. Mark Requirement progress as heuristic when deterministic ID indexing is unavailable; do not over-infer from Markdown search.

# Evidence

- Bind every progress conclusion to one State revision and concrete diagnostics/Evidence paths.
- When the user is asking whether a Change should pass its current approval rather than where it stands, run `xforge brief --change <id> --text` and hand back its output verbatim. It answers a different question than this Skill does, and it answers that one better.

# Stop and rework

- Stop and report missing information when IDs are ambiguous, State errors, or Evidence cannot be verified. Never fill gaps from chat memory.

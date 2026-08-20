---
name: xforge-check
description: Perform a pre-implementation semantic review across Major Change Artifacts for completeness, consistency, testability, risk, and feasibility; use for a ready Check Action or a formal Major planning quality gate.
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# Invariants

- Run `xforge state --change <id>`, consume only the current-revision ready Check Action, and reread Proposal, Specs, Clarifications, Design, Constitution, Rules, and code facts.
- `xforge-check` performs semantic review; `xforge check` supplies deterministic schema, path, Gate, and Evidence input. Neither replaces the other.
- Governing Artifacts are read-only by default. Report rework instead of silently rewriting upstream content.
- A Check report is LLM Review Evidence, not Gate Evidence; `PASS` cannot satisfy a Machine Gate, Transition, or Approval.
- Gate Evidence binds to the content revision at the moment the Gate runs. Run Gates **after your last write**, in one invocation. Running a Gate, editing an Artifact, then running the next Gate leaves the first stale: every Gate reports `passed` and the Stage still will not close.

# Authority

- Write exactly the three Artifacts the Check Stage `produces`: `check-report.md`, `evidence/check-findings.yaml`, and `evidence/constitution-check.yaml`. Both ledgers are Agent-authored — no CLI command writes them, and the Stage cannot exit without them.
- Do not write product code, Proposal/Specs/Clarifications/Design, work packages, or Archive.
- "Gate Evidence" means the `evidence/*.json` files that only `xforge check` writes (`structure.json`, `check-findings.json`, `constitution-check.json`, …). Never hand-write or edit those. The two YAML ledgers above are Artifacts the Gates read, not Gate Evidence.

# Execution

1. Check that Proposal and Specs are complete, unambiguous, testable, and have no unresolved material questions.
2. Check Design coverage of Requirements, constraints, trust boundaries, failure cases, compatibility, migration, and rollback.
3. Verify that tests, rollout, monitoring, stop signals, owners, path scope, dependencies, and parallel boundaries match the critical impact.
4. Run `xforge check --change <id>` and use deterministic diagnostics as evidence input.
5. Write `evidence/check-findings.yaml`: blocker, warning, and suggestion findings, each with Artifact/Requirement location, reason, `refs`, and a `reworkTo` Stage while a blocker is open. Record an explicit empty list when the review found nothing. A blocker marked `resolved` needs a `resolvedBy` naming an approver on one of this Change's receipts or one of its Git authors — the same bar as `approvedBy` below, because an unattributed resolution is not one. Note what that check can and cannot do: while the Change has no commits and no receipts there is nothing to compare a name against, so every name passes and the Gate reports a warning saying so. That pass is provisional. The Change's first commit creates the set, and a name that does not match it then fails a Gate that was green a moment earlier, with this report already written — so put a real identity in from the start rather than one you intend to correct later.
6. Write `evidence/constitution-check.yaml`: one entry per `## ` heading of `xforge/constitution.md`, in document order, each with a `status` of `compliant`, `violation`, or `not-applicable` and at least one machine-locatable `references` entry — a Requirement id from this Change's delta Specs, a path that exists, or `gate:<name>` for a Gate this Change has passing Evidence for. "A path that exists" means any path in the repository, resolved Change-relative first and then project-relative — `xforge/constitution.md` and `xforge/architecture.md` are valid citations and are often the right ones for an architecture or governance principle. Do not confine yourself to Change-local paths. A `violation` also needs a `justification` and a named `approvedBy` (a real approver or Git author, and once this Change holds approval receipts, someone named on one); `not-applicable` needs a `justification`. A bare `compliant` with nothing cited is the blanket claim the Gate exists to reject, and an approval receipt is not a substitute: a receipt records that someone approved a transition, not why this Change satisfies the principle, so citing receipts alone is refused. This bites hardest on the governance principle, where a receipt is the nearest evidence to hand — cite what the Change actually did (the material-questions ledger, the Clarifications, a Requirement id) and add the receipt beside it if you want. Write every `justification` as a block scalar (`justification: >-`, prose indented beneath it): a plain scalar breaks on a colon-and-space or a leading `[`/`{`.
7. With `check-report.md` and both ledgers written, run `xforge check --change <id>` once more; it re-runs and refreshes the whole current-Stage Gate set against the final content. `--all-gates` also runs Gates belonging to Stages the Change has not reached yet, which cannot pass and is rarely what you want mid-Stage.
8. Refresh State. Request the State-specified rework transition for blockers; without blockers, let CLI Gates and Approval determine whether `xforge transition --change <id> --to apply` is ready.

# Evidence

- Report cross-Artifact mappings, CLI results, uncovered Requirements/risks, and feasibility.
- Claim Check satisfied only when blockers are zero and Action `doneWhen` is met.
- Before the approval that lets implementation start, run `xforge brief --change <id> --text` and give the user its output **verbatim**. Do not summarize, reorder, or paraphrase it — the brief separates what the CLI computed from what it quoted, and restating it in your own words destroys the only signal the reader has for telling those apart. Its reconciliation entries state differences between this Stage's own ledgers and the files; answer them, do not argue with them.

# Stop and rework

- Stop on material omissions, contradictions, scope drift, untestable Requirements, missing rollback, or path/owner conflicts.
- Return to the earliest affected Propose, Clarify, or Design Stage **through `xforge-revise`**, which is the sanctioned way to change an upstream Artifact: it revises the affected Artifacts consistently and lets the digest chain invalidate the downstream Evidence that relied on them. Editing an upstream Artifact directly leaves the rest of the Change silently disagreeing with it.
- Do not inspect a nonexistent persistent task plan.

# Judgment calls

- "Passes review" and "the CLI Gate is green" are different claims. A Design can be internally consistent and well-written and still fail Check because a Requirement has no test strategy at all — consistency inside one Artifact does not imply coverage across all of them.
- A missing negative case (the failure path, a boundary, a compatibility break) is easy to miss because nothing in a clean-looking Design points at its own absence. Check what should exist and is not there, not only what exists and is wrong.

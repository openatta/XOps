---
name: xforge-clarify
description: Resolve material ambiguity in a Major Change that affects scope, design, compatibility, risk, or acceptance, and atomically update authorized upstream specifications; use for a ready Clarify Action or formal planning rework.
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# Invariants

- Run `xforge state --change <id>` and consume only the current-revision ready Clarify Action.
- Resolve facts from code, Specs, Rules, and Proposal first; ask only a small set of project-unanswerable questions that materially affect the result.
- Keep Clarifications and authorized Proposal/delta Spec updates in one consistent revision. Any unresolved material question remains blocking.

# Authority

- Write only the clarifications path and existing Proposal/delta Spec paths explicitly listed by the Action's `revises` field.
- Do not write Design, Check reports, code, canonical Specs, Evidence, tasks, or Archive, and do not make material decisions for the user.

# Execution

1. Reread all Action inputs and list unknowns that affect scope, compatibility, risk, implementation boundaries, or acceptance.
2. Investigate project-answerable questions; ask the minimum decision set for the rest.
3. Record each question, impact, decision, source, and status; synchronize confirmed decisions into Proposal and delta Specs while keeping Requirements and Scenarios testable.
4. Refresh State, confirm `materialQuestions: resolved`, run `xforge check --change <id>` to check structure and policy, then request only the `xforge transition --change <id> --to <stage>` the typed nextAction names.

# Evidence

- Cite a user decision or project fact for each decision and identify the Requirements/Scenarios updated.
- Claim Clarify satisfied only when State reports its exit conditions satisfied.

# Stop and rework

- Stop with `request-decision` when the user has not decided, inputs conflict, scope expands, revision changes, or more authority is required.
- If later work reveals a new material ambiguity, invalidate downstream work and return through `xforge-revise` to Clarify.

# Judgment calls

- Not every open question is material. A question that would change the Design's approach or an acceptance boundary is material; a question whose answer only changes an implementation detail belongs in Apply, not here — escalating the latter delays planning without improving it.
- The absence of a stated question is not evidence the project already agreed on the answer. Silence usually means nobody decided, not that the obvious choice was chosen — treat an unstated but consequential default the same as an open question.

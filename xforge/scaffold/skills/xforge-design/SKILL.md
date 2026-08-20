---
name: xforge-design
description: Produce a governed technical design for a Solid or Major Change, including alternatives, failure boundaries, and verification; use for a ready Design Action after Proposal, Specs, and required Clarifications are satisfied.
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# Invariants

- Run `xforge state --change <id>`, consume only the current-revision ready Design Action, and reread every Action input.
- Design explains HOW, decisions, and boundaries. It does not repeat Proposal or become a file-by-file task list or persistent plan.
- Constitution, Rules, current architecture, and Specs constrain the design; summarize their implications instead of copying them mechanically.

# Authority

- Write only the Design Artifact path returned by the Action.
- Do not modify Proposal, Specs, Clarifications, product code, Check reports, Evidence, tasks, or Archive. Return upstream changes as rework.

# Execution

1. Model the current system, target behavior, integration points, data, and interface boundaries.
2. Record major decisions, viable alternatives and rejection reasons, failure modes, compatibility, migration, and rollback.
3. Follow the current Action's Design artifact `instruction` and outline exactly — Solid vs Major depth (e.g. Major's trust boundaries, risks and mitigations, test strategy, rollout, monitoring, stop signals, owner, and parallel boundaries) is already expressed there. Do not add or omit sections the Action does not define.
4. Refresh State and run `xforge check --change <id>`; fix only Design-authorized structural issues. Stop for human Approval and invoke only a typed ready Transition after the receipt is satisfied.

# Evidence

- Read `xforge/architecture.md` when it exists, and say how this Change stands against each decision it touches — within it, or departing from it with a stated reason. When the design needs a decision *changed*, write the proposal into the Design Artifact you own and stop for a human. Do not write `evidence/conditions/architectureDeltas.yaml` yourself: that entry names a `decidedBy`, and an Agent filling in a human's name records an authorisation nobody gave — the exact thing the ledger exists to catch. A human authorises and invokes `xforge-architect`, which is the only writer of the architecture file and its ledger. When the file does not exist, say so once and proceed: it is a project that has not written its architecture down, not a project in violation.
- Map each major decision to a Requirement, project constraint, or code fact and state the verifiable result.
- Report coverage, residual risk, and the next legal Action against Action `doneWhen`.
- When this Stage exits on a human approval, run `xforge brief --change <id> --text` and give the user its output **verbatim**. Do not summarize, reorder, or paraphrase it — the brief separates what the CLI computed from what it quoted, and restating it in your own words destroys the only signal the reader has for telling those apart.

# Stop and rework

- Stop on material ambiguity, Spec conflict, unknown trust boundary, irreversible impact, or an upstream change requirement.
- Hand upstream issues to Clarify/Revise and never silently expand Scope in Design.

# Judgment calls

- The cheapest-looking alternative is not automatically the right one to reject last. Write down why a simpler approach was rejected even when it seems obviously insufficient — "obviously insufficient" is exactly the kind of claim a reviewer six months later cannot verify without the reasoning that produced it.
- Compatibility and rollback are two different questions. A design that is backward-compatible in its data format can still be irreversible in practice if the migration is one-directional — check both independently instead of treating "compatible" as implying "reversible."

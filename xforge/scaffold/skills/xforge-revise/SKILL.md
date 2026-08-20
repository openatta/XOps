---
name: xforge-revise
description: Revise existing Change planning Artifacts consistently and invalidate affected downstream state and evidence; use when requirements, scope, or decisions change, or Check/Apply discovers an invalid upstream assumption.
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# Invariants

- Run `xforge state --change <id>` and use the dependency graph to find the earliest affected governing Artifact; never guess paths or create a missing Artifact.
- Reread existing files and Action inputs before every edit; keep Requirements, Scenarios, decisions, and scope consistent across Artifacts.
- Let digest/revision changes invalidate stale Check, Apply, or Verify results. Never tamper with Evidence manually.

# Authority

- Modify only existing Proposal, delta Spec, Clarifications, or Design paths explicitly returned by State and within user-authorized scope.
- Do not write product code, Check reports, work-package delivery, Gate Evidence, verification receipts, canonical Specs, or Archive.

# Execution

1. Resolve the change reason, earliest affected Artifact, and downstream planning material requiring synchronization.
2. Make the minimum consistent revision to concrete existing paths while preserving machine headings and stable IDs.
3. Request a user decision before materially expanding Scope, compatibility impact, or permissions.
4. Refresh State and run `xforge check --change <id>`; confirm stale downstream Gate/Approval revisions and list Stages that must rerun. Change Stages only through CLI Transition.

# Evidence

- Report changed Artifacts/Requirements, reason, new State revision, invalidated scope, and next legal Action.

# Stop and rework

- Stop if a target path is missing, inputs conflict, authority does not cover the edit, or code/Evidence changes are needed; hand off to the owning Skill.

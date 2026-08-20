---
name: xforge-upgrade-scaffold
description: Merge a staged newer XForge Scaffold into this project's own, preserving the project's adaptations and reporting what needs a human decision; use after `xforge upgrade-scaffold` has staged a version and written MERGE.md.
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# Invariants

- Read `xforge/scaffold-<version>/MERGE.md` and `plan.json` first. They name the whole job; nothing here requires reading the Scaffold to discover it.
- The `identical` files are already settled. Do not open them — the plan exists so the work is the few files that differ, not the seventy-eight that do not.
- `xforge/scaffold/**` is the project's; `xforge/scaffold-<version>/**` is the release's. Neither is authoritative over the other by default.
- `xforge/.rollback/**` is the restore point. Never write to it.
- Read what this project currently selects with `xforge state --kind skills` (and `--kind rules`), not by parsing `xforge/manifest.yaml`. What is selected is a resolved fact the CLI reports; the file is one input to it.
- `manifest.scaffold.version` tracks the Scaffold's *content* and only `upgrade-scaffold --complete` advances it, so a project whose CLI is newer than its Scaffold is in a normal state, not a broken one. If `xforge upgrade-scaffold` refuses because the declared CLI does not match the running one, run `xforge update` first: it moves the CLI pin alone and leaves the Scaffold pin where the files are.
- `XFORGE_UPGRADE_VERSION_PIN_UNRELIABLE` means the pin says this Scaffold is already the incoming version while files disagree — written by an older `update` that advanced the pin without merging anything. The starting version is unrecoverable, so the reported span is meaningless; the merge itself is computed from file content and is unaffected. Say so once and continue.

# Authority

- Write `xforge/scaffold/**`, and `xforge/manifest.yaml` only to record selections a person explicitly approved.
- Do not touch `xforge/changes/**`, `xforge/specs/**`, the audit chain, approvals, `xforge/constitution.md`, or `xforge/architecture.md`. The Scaffold can be regenerated; the governance record cannot, and an audit chain that could be rebuilt would not be worth keeping.
- Never delete a `project-only` file. Nothing distinguishes an asset upstream dropped from one this project wrote, so deleting on that reading destroys somebody's work on the strength of a guess.

# Execution

1. For each `added` file: copy it in verbatim. Do not add it to Manifest selection — a file arriving in a release is not a decision to run it.
2. For each `changed` file: read both versions. Adopt what the new one **rules**; keep what this project **knows**. A Gate carrying a real test command, a Skill carrying wording this project chose, a threshold somebody tuned — those are facts about this project and they survive the upgrade.
3. Keep English and `_cn` Skill variants equivalent. Merging one language and not the other leaves the project with two Skills that disagree, and whichever an Agent reads is then a matter of the Manifest's language setting rather than of what the project decided.
4. Run `xforge upgrade-scaffold --complete`, then `xforge install`, then `xforge doctor`.

# Evidence

- Report, per `changed` file, which side you took and why in one line. "Adopted upstream" and "kept ours" are both answers; an unreported merge is not.
- List every asset the plan marked shipped-but-not-selected, and say plainly that selecting it is the user's decision, not yours.
- Quote `xforge upgrade-scaffold --complete`'s adoption count verbatim. It reports how many planned files now match the release; it does not grade the merge, and restating it as a score would invent a judgement the CLI did not make.

# Stop and rework

- Stop when a `changed` file's two versions cannot both hold — when the release removes a rule the project depends on, or renames something the project references. That is a decision about the project, not about the merge.
- Stop rather than resolve a conflict by taking the newer file wholesale. Preferring upstream is the one resolution that is always available and almost never right; it is how a project silently loses the adaptation the Scaffold existed to invite.
- Stop when the staged directory is missing or its `plan.json` does not parse: run `xforge upgrade-scaffold` rather than reconstructing the plan by reading directories.

# Judgment calls

- A file that differs only in wording still deserves the question. Upstream rewrites a Skill's prose because the old wording misled an Agent, so "it means the same thing" is exactly the claim the rewrite disputes.
- Selection is a separate decision from content, and it is the one that changes behaviour. Copying `xforge-architect` in changes nothing; adding it to `scaffold.skills` changes what every Agent on the project is told to do. Bring the file, report the choice.
- A merge with no conflicts is a normal outcome, not a suspicious one. Most releases change files no project has touched, and inventing a difficulty to look thorough wastes the reader's attention on the one that matters.

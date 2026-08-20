---
name: xforge-scaffold
description: Customize project-canonical XForge agents, skills, rules, permission policies, hooks, and gates and project them safely into target tools; use when the user asks to add, change, enable, disable, or install project Agent capabilities.
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# Invariants

- Run `xforge state --kind <skills|agents|rules|policies|hooks|gates|scripts|flows|approvals|mcp-servers>` and read Manifest selection, canonical assets, Adapter capabilities, and degradation. Plain `xforge state` reports every kind at once.
- `xforge/scaffold/**` is source. `.agents/`, `.codex/`, `.claude/`, `.cursor/`, `.opencode/`, `.github/`, and `opencode.json` are generated and must not be edited directly.
- Directory discovery never enables an unfinished or unselected resource.
- Agents and Skills keep English canonical entries plus `_cn` Chinese variants. Manifest `scaffold.language` selects projection; all other Scaffold assets remain English.

# Authority

- Modify `xforge/scaffold/**` and minimally update Manifest scaffold selection only when adding, removing, enabling, or disabling resources. `xforge/manifest.yaml` is covered by the `protected-manifest` PermissionPolicy, whose effect is `ask`, not `deny`: expect a confirmation prompt on that write and show the user the exact selection diff before answering it.
- Do not modify product code, Specs, Changes, Flow business state, or generated directories.
- Show and confirm Hooks, PermissionPolicy, network, secrets, tool-permission expansion, and destructive commands before install. Installed, platform-trusted, and runtime-active are distinct states.

# Execution

1. Query the target resource kind and Adapter capability, then reread existing resources and references.
2. Create the smallest canonical asset and close Agent→Skill, Rule→Gate/Policy/Approval, and Hook→dispatcher/event/failure-policy references. Rules express guidance/coverage; enforcement belongs in PermissionPolicy.
3. For every Agent/Skill text change, update English and `_cn` variants with equivalent invariants, commands, authority, evidence, and stop conditions.
4. Run `xforge check`, then `xforge sync --dry-run`; show cross-target diff, conflicts, capability level, and sensitive changes.
5. After confirmation, run `xforge sync`. If the CLI returns `XFORGE_FULL_UPDATE_REQUIRED` or `XFORGE_STATE_UPGRADE_REQUIRED`, run `xforge update --dry-run` instead, then `xforge update` after confirmation. Never report a successful install as an unsupported capability being active.
6. Query State again and verify Manifest selection, language, lock digest, ownership, Adapter coverage, and installation; report any separate platform review/trust requirement.
7. On `XFORGE_VERIFICATION_NOT_DECLARED`, `XFORGE_VERIFICATION_TOOLCHAIN_UNCOVERED`, or `XFORGE_VERIFICATION_GATE_MIGRATED`, **ask the user how this project runs that check and record their answer** under `manifest.verification.<gate>` with their name in `declaredBy`. The `nextAction` carries any command this CLI can suggest for the build-system markers it found — treat every one as a question to put to the user, never as an answer to adopt. **Do not guess, and do not adopt a suggestion silently:** XForge cannot tell whether a command verifies anything, which is exactly why a person has to say so and be named. A toolchain the Gate deliberately does not cover is recorded as `notApplicable` with a `justification` rather than left unanswered.

# Evidence

- Report canonical paths, Manifest changes, dry-run/sync changes, Adapter degradation, and final lock/installation state.

# Stop and rework

- Stop on broken references, target conflicts, modified user files, unconfirmed sensitive permissions, missing locale parity, or unsupported Adapter behavior. Never bypass ownership/conflict policy.

---
name: xforge-kanban
description: Generate a Git-history activity dashboard for this repository — per-contributor activity and code volume, a commit-time heatmap, and a feature/fix/other breakdown — derived only from Git commit metadata; use when the user asks for a project dashboard, contribution report, activity heatmap, or commit history summary.
allowed-tools: Read, Grep, Glob, Write, Bash(git:*), Bash(node:*)
---

# Invariants

- This Skill is read-only project reporting, independent of Change/Flow/Gate lifecycle state; never query `xforge state --change <id>` or touch `xforge/changes`, `xforge/specs`, Evidence, or Approvals for it. The bundled script may call plain `xforge state` (no `--change`) only to read `project.modules` for grouping, and degrades to a single implicit module when that is unavailable — this is project-structure lookup, not Change/Flow/Gate governance.
- Treat `git log` (and, for module grouping, `project.modules` from `xforge state`) as the only sources of truth. Never invent commits, authors, dates, counts, or module boundaries not produced by the bundled script.
- Run `scripts/git-activity.mjs` to extract data; never hand-count from a partial `git log` read or from memory.
- Group contributors by email, not display name — the same person may commit under multiple names (the script does this; do not re-group by name yourself).
- Classify a commit as `feat`/`fix`/other only from a literal Conventional Commits type prefix in its subject line (e.g. `feat:`, `fix(scope):`). A commit without a recognized prefix is `unclassified`; never guess its intent from the diff or message body.
- A shallow clone or a filtered `--since`/`--until`/`--author` range under-covers history; state this plainly instead of presenting partial data as complete.
- Projects may need more than Git alone (e.g. linking commits to issues via an MCP server). That belongs in a project-local extension of this Skill's script, not in this Skill's invariants — see Stop and rework.

# Authority

- Only permitted actions: run the bundled script (read-only; it never writes) and, only if the user explicitly asks for a saved copy, write the rendered report to a project-local path outside version control.
- Never commit, push, amend history, or write into any tracked file as a side effect of this Skill.
- Never rewrite, filter, or "clean up" Git history to change what the report shows.

# Execution

1. Confirm the working directory is inside a Git repository (`git rev-parse --is-inside-work-tree`). If not, or if `git` is unavailable, stop and say so.
2. Run `node scripts/git-activity.mjs [--root <path>] [--since <date>] [--until <date>] [--author <pattern>]` from this Skill's directory and parse its JSON stdout. Pass through any date/author scoping the user asked for; do not filter results yourself after the fact.
3. If the script exits non-zero or reports `shallow: true`, surface that verbatim before anything else — a shallow clone undercounts history and the user needs to know before trusting the numbers.
4. Render the JSON into a Markdown dashboard:
   - a contributor table: commits, lines added/deleted, active days, first/last commit date (one row per email; list alternate display names inline if more than one);
   - a compact weekday × hour activity heatmap from the `activity` histogram (text/emoji grid or a Markdown table — whichever renders best for the current output surface);
   - a `feat`/`fix`/other breakdown from `typeBreakdown`, listing the commit subjects under each type so the user can see what each feature/fix actually was, not just a count;
   - if `modules` has more than one entry, a per-module section (same contributor/activity/type shape as above, scoped to that module) so activity in a monorepo is not flattened into one misleading global ranking; if `modules` has exactly one entry, skip this section — the global numbers above already cover it and repeating them would be noise;
   - when there is more than one module, also report `unscoped` (activity outside every declared module path — e.g. root-level docs or CI config) and `crossModuleCommits` (commits whose changes span more than one module) as their own short sections; never fold their line counts silently into a single module's total.
5. Default to presenting the dashboard in the reply. Only write it to a file if the user explicitly asks for a saved copy; then write under a project-local, untracked path (e.g. `.xforge-kanban/<name>.md`) and tell the user to confirm it is `.gitignore`d before it risks being committed.

# Evidence

- Report the exact commit range covered (`range.from`–`range.to` from the script output), total commit count, and the `shallow` flag.
- Report `moduleResolution`: state plainly whether module grouping came from the project's own `project.modules` (`xforge-state`) or is a single implicit fallback module (`implicit-root`) because XForge was unavailable or this is not an XForge-managed project.
- State every number exactly as the script reported it; do not round, estimate, or extrapolate.

# Stop and rework

- Stop if the directory is not a Git repository, `git` is unavailable, or the script errors — do not fall back to guessing from file listings or memory.
- Stop and ask before writing any file to disk.
- If the user wants data the script cannot produce (issue links, PR metadata, a different classification scheme, extra MCP-sourced signals), do not fabricate it. Point them at `xforge-scaffold` to extend this Skill's private `scripts/git-activity.mjs` in their own project copy — this Skill is intentionally scaffold-local so every project can adapt it.

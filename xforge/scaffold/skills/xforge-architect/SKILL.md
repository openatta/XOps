---
name: xforge-architect
description: Write and maintain xforge/architecture.md, the project's durable architecture — its module map, invariants, and the few decisions whose reversal would touch several modules; use when starting a project, when asked about the architecture, or when an approved architecture change needs merging back.
allowed-tools: Read, Grep, Glob, Write, Edit, Bash(xforge:*)
---

# Invariants

- `xforge/architecture.md` is a durable project asset, not a Change Artifact. It does not archive, it has no delta semantics, and it is the only file this Skill writes.
- **This Skill is the sole writer.** A Change may propose an architecture change; only this Skill merges one in, and only when a person asks it to.
- Requirements are merged automatically at archive because they have delta semantics. Architecture does not: it is one map that has to stay internally consistent, so merging it is a deliberate act, not a side effect of closing a Change.
- Absence is a legitimate state. A project without this file is not broken — nothing blocks, and Changes proceed normally. Offer to create it; never require it.
- Keep it under 50 lines. The budget is spent on **fewer decisions**, never on shorter ones: a decision recorded without its reason is remembered backwards as often as forwards, and then reversed by somebody who thinks they are fixing it.

# Authority

- Write only `xforge/architecture.md`.
- Do not modify Specs, Changes, product code, the Constitution, Flows, or any Scaffold asset. An architecture decision that belongs in the Constitution is a Constitution amendment, and that is a governed Change, not this.
- Never invent an architecture. Every decision here is the user's; this Skill proposes candidates and records what the user chose.

# Execution

1. Establish the current state: read `xforge/architecture.md` if it exists, and run `xforge state` for the project's declared modules and paths.
2. Choose the way in from what the user has:
   - **From code** — read the tree and report what the architecture *appears* to be, then confirm every line with the user before writing. Reading tells you what is true, never why it was chosen, and the why is the part worth keeping.
   - **By questioning** — converge with a few questions the user can answer without design work: how many protocol entry points, what persists and where, one process or several, what is deliberately out of scope. Stop as soon as the answers pin the module map; a longer interview produces a longer file, which is the wrong direction.
   - **From a description** — the user states the architecture and this Skill writes it in the shape below, asking only where the description is ambiguous.
3. Write the file in this shape. Sections are fixed; content is the project's.

   ```markdown
   # Architecture — <project>

   <one sentence: what this system is>
   Non-goals: <2-3 lines of what it deliberately does not do>

   ## Structure

   | Module | Responsibility | Path |
   |---|---|---|

   Invariants:
   - <3-5 properties that must hold across modules, each one checkable>

   ## Decisions

   ### ARC-001 <title>
   <the decision, one line>
   **Why**: <what breaks without it>
   **Rejected**: <the alternative this project weighed and turned down, and why — especially when
   it was the better engineering choice and lost to a constraint; omit the line entirely when
   nobody weighed one>
   **Located in**: <paths>
   **Supersedes**: <ARC-id, when replacing one>
   ```

4. Recording an architecture change a Change proposed: Design writes the proposal into its own
   Artifact and stops, because the ledger entry names a `decidedBy` and only a human can be that.
   Read the proposal, put the decision to the user in the terms it states, and on a clear answer
   write the entry into the Change's `evidence/conditions/architectureDeltas.yaml` naming that user
   as `decidedBy`. You are the only writer of that ledger — an Agent that could write its own
   authorisation is not an authorisation.
5. Merging a decided architecture change: read the Change's `evidence/conditions/architectureDeltas.yaml`, apply each decided entry to the file — new `ARC-` entry, or an amendment carrying `Supersedes` — and tell the user which entries were merged. Do not delete the ledger; it is the Change's evidence and archives with it.
6. Report what changed, what the user decided, and anything you deliberately left out.

# Evidence

- State which way in was used, and for a code-derived file, which paths were read.
- Name every decision the user made, and quote back the reasons recorded for them.
- Report the line count against the 50-line budget and the decision count against 6 (hard maximum 8). Over budget, propose which decisions to drop rather than which lines to shorten.

# Stop and rework

- Stop when a decision the user is asking for contradicts the Constitution — that is a Constitution amendment and belongs in a governed Change.
- Stop rather than write a decision whose reason you had to infer. A `Why` that nobody stated is a guess wearing the same clothes as a decision.
- The same rule governs `Rejected`, which records an alternative this project actually weighed and turned down. A credible alternative nobody ever considered is an open question, not a rejection: put it in your report and leave the line out. A reader a year later cites `Rejected` as proof the option was evaluated, so an inferred one manufactures a deliberation that never happened — and unlike a missing line, it is invisible as a gap.
- Report, and do not merge, an architecture delta whose ledger entry has no `decidedBy`. An unauthorised architecture change is exactly the thing the ledger exists to catch.

# Judgment calls

- Reserve a decision for one whose reversal would touch several modules. A local technology choice — a date library, a logging format — belongs in the code that makes it. Six decisions that shape the system beat sixteen that describe it.
- Write invariants as things that can be checked (`api/** must not reach store/**`), not as prose about collaboration. A live project passed 207 tests at 93% coverage while its services were never wired to anything: every module was correct and nothing was connected. Prose about responsibilities cannot see that gap, because neither side of the comparison is wrong.
- When reading code to derive the file, the module boundaries are usually visible and the reasons usually are not. Say so, and ask, rather than reconstructing a plausible rationale — a wrong `Why` is worse than an absent one, because it will be defended.

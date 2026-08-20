---
paths:
  - "src/**"
---

# design-within-the-declared-architecture

Severity: should

When xforge/architecture.md exists, read it before designing or implementing, and state in the Stage's Artifact how this Change stands against each decision it touches: within it, deliberately departing from it (say why), or proposing to change it. A departure that is written down is a fact somebody can act on; the same departure recorded in a build-file comment is one nobody will find. Proposing a change is a legitimate third answer, not a failure — an architecture that can only be complied with is one every Change will quietly route around. When the file does not exist, proceed normally and say so once: it is a project that has not written its architecture down, not a project in violation. Judgement guidance only; XForge does not claim to enforce it.

Enforcement: gates=none; policies=none; approvals=none.

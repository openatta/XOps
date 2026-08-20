---
paths:
  - "xforge/constitution.md"
  - "xforge/specs/**"
  - "xforge/manifest.yaml"
  - "xforge/lock.yaml"
  - "xforge/flows/**"
  - "xforge/.audit/**"
---

# governance-assets-are-integrator-only

Severity: must

Shared governance assets (Constitution, canonical Specs, Manifest, Lock, Flow definitions, the audit chain) are written by the Integrator, an explicit XForge transaction, or the CLI itself, never by a Worker implementing a work package. Scaffold resources under xforge/scaffold/ and Change content under xforge/changes/ are written by the governing Skills and are intentionally outside this Rule's scope.

Enforcement: gates=none; policies=protected-files, protected-manifest; approvals=none.

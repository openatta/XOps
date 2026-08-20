---
paths:
  - "src/**"
  - "tests/**"
---

# observable-requirements-are-tested

Severity: must

Every externally observable requirement added or modified by a Change must have automated verification. A requirement whose only evidence is prose is not verified.

Enforcement: gates=unit-tests; policies=none; approvals=none.

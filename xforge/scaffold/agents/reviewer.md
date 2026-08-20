Review the final base-to-integrated-commit diff independently. Do not rely only
on Worker or Integrator summaries and do not participate in the original
implementation. Read the Constitution, Change Specs, optional Design/Check report,
`work-packages.yaml`, delivery records, and current Gate Evidence.
Also inspect dispatch bindings, effective Rule/PermissionPolicy coverage, and
reported runtime Audit gaps.

Check requirement coverage, contract coherence, compatibility, security, test
quality, work-package write boundaries, shared-file ownership, and whether each
`verify` and `done_when` claim has evidence. Use a separate review worktree for
commands that create caches, coverage, or build outputs. Do not modify product
code or hand-write Evidence.

Return `pass` or `changes-required`. Each finding must include severity, an
actionable file or requirement location, the reason, and a recommended fix.
State explicitly when no substantive issue exists. Never self-approve a Major
Change or an exception. A reviewer `pass` is assurance only: it is not Machine
Gate Evidence, an Approval receipt, or authority to transition/archive.

You cannot write any file, including your own evidence: this Agent is granted
read, search, and test tools only. Return your complete result as your reply,
in a form that can be stored unchanged — verdict, every finding with its
severity, location, reason, and recommended fix. The Main Agent transcribes it
verbatim into `<change>/evidence/agents/<package>/review-<execution>.yaml` and
then runs `xforge work-package acknowledge --change <id>
--package <package> --as reviewer --evidence <that path>`. Do not summarize
your own findings on the assumption someone will expand them; what you return
is what is recorded. Note the trade-off this creates and say so if it matters:
the party being reviewed is the party writing the record. What makes that
detectable is that the transcript is committed and covered by the audit chain,
not that it is impossible to alter. Whenever a Skill you follow calls for
running the XForge CLI directly, invoke it as `xforge
<command> ...` — a project-local install is not on this shell's `PATH`.

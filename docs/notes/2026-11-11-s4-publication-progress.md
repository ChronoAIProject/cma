# S4 writer publication progress — 2026-11-11

Part of #1350 and transactional #1349. Branch
`tl/cma-log-segments/writer-publication`. The published source checkpoint
`2777afa105da8cd516ffe7d6d9795b47c6013aeb` includes the actual keyed append
entry and S1 point/visitor forwarding. This is a source handoff, not a
review-ready PR or production activation. The merge freeze is lifted; review
verdict and the lead merge relay still apply.

## Consumed dependencies

Normally merged S1 scoped capture/compile fixes
`5b8a823fc947cdabf3018b8eee524fa27f5f96bc`, S2 output-index contract
`9327ec32f`, and bounded scope page method `9e5c47a72`. No mixed integration
tip was imported. Private transaction stamps and database binding remain
mandatory; snapshot capture and raw transaction capture cannot authorize writes.

## Executable source now present

- `writer_commit::{prepare,persist}` validates the captured source and exact
  publication, then writes one Agent CAS, original BSON, optional seal/head,
  projection/evidence and immutable receipt in the caller's transaction.
- The pure core planner validates the complete batch, immutable source lineage,
  source queue and final aggregate before returning changes.
- Invisible bootstrap uses the original admission and fenced permit, bounded
  writes and exact final recapture. Source-based renewal every64 rows also
  covers diagnostic-only histories that produce no evidence rows.
- Output publication/bootstrap uses the unchanged S2 model: mutable turn/tool
  facts and immutable numeric output-change rows. First-terminal evidence
  remains unchanged when later output updates current tool state.
- `AgentLogStore::append_agent_log_keyed` now performs real admission/staging,
  same-scope publication and exact receipt/event replay after later appends.
  Default-closed legacy adoption refuses without mutation. Fitting segmented
  appends need no new gate admission. Unknown outcome returns its retained
  identity without starting another publication.
- Point/range/predecessor visitor ports forward to the S1 format-aware adapters.
  The existing unkeyed append compatibility path remains legacy.

The keyed caller consumes the prepared publication and releases the exclusive
scope before existing suggestion-domain work and final commit. A later domain
error aborts the entire transaction. No raw-session accessor was widened. This
ordering does not by itself qualify the other transaction caller families.

## Evidence and limits

| Check | Actual result |
| --- | --- |
| Core projection units |10passed at the whole-publication source checkpoint |
| Core all-target clippy, warnings denied |Passed before output/caller additions |
| CMA all-target check |Reached new sources; found a missing test import, now fixed. Production wiring warnings remained. No corrected pass yet. |
| New S4 unit gate |First attempt timed out after600s waiting for heavy-0, without starting Cargo. Second acquisition pending at this checkpoint. |
| New S4 Mongo tests |Source present; not executed |
| Formatting / whitespace |Passed on current source |
| Repository size |5917files passed; all new S4 source/test files below500lines |
| Secret scans |Staged deltas passed. Full history throughfb1ad83c7:3648commits, no findings. Later deltas independently scanned. |

New tests cover production captured-source commit/abort; wrong/later transaction
and snapshot refusal; bootstrap recapture/replaced permit; output atomicity and
missing index; same-source competition and different Agents in one database;
gate-close continuation; legacy append rejection; known-abort retry; commit
failpoint with later-head reconciliation; actual keyed trait adoption/replay;
and sealed-source point integrity. Their presence is not a passing Mongo gate.

## Remaining work and handoffs

- S1: transaction certification and additive output numeric validator/index plus
  gate/admission schema. Audit/rebuild must preserve the output evidence of its
  promoted generation; it can reuse the shared S4 fold/encoder.
- S2: narrow canonical Mongo runner dependency fromd6368d023, plus the existing
  operator/migration/authority assembly. The mixed integration branch is not
  imported into this slice.
- S4: conversation/worker/lifecycle/admission/reuse roots and bounded command
  evidence are still incomplete. Preserve original command, lease, checkpoint,
  budget and authorization semantics; no blanket dead-code allowance or fake
  reference replaces this wiring. Exact shared-helper requests are posted in
  #1350 comment-6795669495.
- S3/S5: their owned title/trigger/recovery callers and scoped checkpoint/domain
  adapters. Full caller coverage and old-binary/release qualification remain
  integration acceptance gates.

No PR is open. Required strict CMA lint and real-Mongo gates are not green.
Project9 remains In Progress; milestone comments are on#1350/#1349/#1348.

## Resource discipline

Use only the assigned existing heavy-0 with`lockf -k -n`, two Cargo jobs and
one`api/target`. Never delete or recreate a shared lock. The previous omission
of-k is recorded in the gate note and was reported. The current inode remained
160171589 throughout the latest source work and queued gate attempts.

One1.2GiB target is retained for immediate gate reuse; no Mongo resources were
created. A checksum-verified local copy of CI's nextest0.9.151 is prepared for
the Mongo gate. Temporary tools/logs will be removed when no longer needed.
Previously reported own cleanup remains`CLEANED1.32GB`; other teammates' freed
disk space is not attributed to this work. Retain requested notes and the
dedicated worktree for the lead's eventual removal.

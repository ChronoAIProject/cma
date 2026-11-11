# S4 writer publication progress — 2026-11-11

Part of #1350 and transactional #1349. Branch
`tl/cma-log-segments/writer-publication`. The published source checkpoint
`0513eae62` includes the actual keyed append entry, S1 point/visitor
forwarding and the lifecycle checkpoint transaction root. This is a source handoff,
not a review-ready PR or production activation. The merge freeze is lifted; review
verdict and the lead merge relay still apply.

## Consumed dependencies

Normally merged S1 scoped capture/compile fixes
`5b8a823fc947cdabf3018b8eee524fa27f5f96bc`, S2 output-index contract
`9327ec32f`, bounded scope page method `9e5c47a72`, S1 executable
qualification/snapshot correction `d5d934731`, and the plan-authorized canonical
Mongo registration `9011a3796` (including S5 scoped checkpoint helpers).
No mixed integration tip was imported. Private transaction stamps and database binding remain
mandatory; snapshot capture and raw transaction capture cannot authorize writes.

## Executable source now present

- `writer_commit::{prepare,persist}` validates the captured source and exact
  publication, then writes one Agent CAS, original BSON, optional seal/head,
  projection/evidence and immutable receipt in the caller's transaction.
- The pure core planner validates the complete batch, immutable source lineage,
  source queue and final aggregate before returning changes.
- Invisible bootstrap uses the original admission and fenced permit, bounded
  writes and exact final recapture. Source-based renewal every 64 rows also
  covers diagnostic-only histories that produce no evidence rows.
- Output publication/bootstrap uses the unchanged S2 model: mutable turn/tool
  facts and immutable numeric output-change rows. First-terminal evidence
  remains unchanged when later output updates current tool state.
- `AgentLogStore::append_agent_log_keyed` now performs real admission/staging,
  same-scope publication and exact receipt/event replay after later appends.
  Default-closed legacy adoption refuses without mutation. Fitting segmented
  appends need no new gate admission. Unknown outcome returns its retained
  identity without starting another publication.
- Lifecycle checkpoint capture now starts the exclusive scope at its real
  transaction root, consumes S5 bounded capture/fencing for segmented children,
  then releases the scope before existing parent/event writes. A later domain
  failure aborts those earlier fences. Legacy checkpoint validation is retained.
  Agent-log appends from this lifecycle family are still pending conversion.
- Point/range/predecessor visitor ports forward to the S1 format-aware adapters.
  The existing unkeyed append compatibility path remains legacy.

The keyed caller consumes the prepared publication and releases the exclusive
scope before existing suggestion-domain work and final commit. A later domain
error aborts the entire transaction. No raw-session accessor was widened. This
ordering does not by itself qualify the other transaction caller families.

## Evidence and limits

| Check | Actual result |
| --- | --- |
| Core projection units | 10 passed at the whole-publication source checkpoint |
| Core all-target clippy, warnings denied |Passed before output/caller additions |
| CMA all-target check |Reached new sources; found a missing test import, now fixed. Production wiring warnings remained. No corrected pass yet. |
| New S4 unit gate |Two attempts each timed out after 600 seconds waiting for heavy-0, without starting Cargo. A subsequent nonblocking CMA-check acquisition also refused the occupied lock. |
| New S4 Mongo tests |Source present; not executed |
| Formatting / whitespace |Owned changes pass. Workspace fmt currently reports one inherited assertion-formatting diff in S1 capture_tests.rs:207; no S1 file was edited. |
| Repository size |5,948 files passed after the dependency merges; all new S4 source/test files below 500 lines |
| Secret scans |Staged deltas passed. Full history through fb1ad83c7: 3648 commits, no findings. Later deltas independently scanned. |

New tests cover production captured-source commit/abort; wrong/later transaction
and snapshot refusal; bootstrap recapture/replaced permit; output atomicity and
missing index; same-source competition and different Agents in one database;
gate-close continuation; legacy append rejection; known-abort retry; commit
failpoint with later-head reconciliation; actual keyed trait adoption/replay;
and sealed-source point integrity. Their presence is not a passing Mongo gate.

## Remaining work and handoffs

- S1: additive output numeric validator/index plus gate/admission schema and
  the inherited capture-test formatting correction. Transaction capture now has
  S1 executable certification (36 Mongo tests at bb6b4f801); this is not a
  combined S4 test result. Audit/rebuild must preserve the output evidence of its
  promoted generation; it can reuse the shared S4 fold/encoder.
- S2: remaining shared scoped-helper ownership and operator/migration/authority
  assembly. The canonical runner dependency is consumed at 9011a3796, as
  authorized by the updated strict-lint assignment. The mixed integration
  branch was not imported.
- S4: conversation/worker/admission/reuse roots, lifecycle log publication,
  and bounded command evidence are still incomplete. Preserve original command, lease, checkpoint,
  budget and authorization semantics; no blanket dead-code allowance or fake
  reference replaces this wiring. Exact shared-helper requests are posted in
  #1350 comment-6795669495.
- S3/S5: their owned title/trigger/recovery callers and scoped checkpoint/domain
  adapters. Full caller coverage and old-binary/release qualification remain
  integration acceptance gates.

No PR is open. Required strict CMA lint and real-Mongo gates are not green.
Project 9 remains In Progress; milestone comments are on #1350/#1349/#1348.

## Resource discipline

Use only the assigned existing heavy-0 with `lockf -k -n`, two Cargo jobs and
one `api/target`. Never delete or recreate a shared lock. The previous omission
of -k is recorded in the gate note and was reported. The current inode remained
160171589 throughout the latest source work and queued gate attempts.

No Mongo resources were created. Cleanup removed the owned target, checksum-verified
local nextest 0.9.151 copy and temporary gate logs: 1,343,286,598 bytes
(1.34 GB). Both shared lock files were preserved; disk readback was 61 GiB free.
Previously reported cleanup was `CLEANED 1.32 GB`; other teammates' freed disk
space is not attributed to this work. Retain requested notes and the
dedicated worktree for the lead's eventual removal.

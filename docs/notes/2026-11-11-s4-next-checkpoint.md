# S4 original caller and restart checkpoint — 2026-11-11

Part of #1350. Branch `tl/cma-log-segments/writer-publication`.
No PR: the integration owner opens segmentation PRs. The develop freeze was
lifted at 22:58 SGT; this slice still requires an exact-head review and merge relay.

## Implemented source

- Keyed append retains the exact original Publication, resulting Agent and log
  entry. Unknown commit reconciliation uses `writer_reconcile::{Outcome,reconcile}`
  at the original session operation time and compares the complete receipt.
- A bounded `keyed_publication_attempt_v1` field on the existing Agent now survives
  call/process loss. Its majority-durable CAS binds the original key, fingerprint,
  timestamp, creation identity, source revision/sequence, result digest and random
  claim token. Publication removes that exact field in the same transaction as
  its immutable receipt. Missing receipt evidence leaves the attempt unresolved;
  it cannot replan against a later head. Only a locally known abort releases the
  claim for retry. No new collection, TTL or unbounded request map is introduced.
  A crash before publication is conservatively unresolved as well; there is no
  automatic expiry or inference of abort from absence. This addresses lost attempt
  state, not a demonstrated duplicate committed-row bug.
- Adopted Agent worker, stop/delete control and lifecycle-child mutations call
  the shared writer through `TransactionScope`. Original domain validators,
  worktree/parent/lease fences and events still share the transaction; a later
  refusal aborts the proposed log/head/projection/publication as well.
- Original worker/control/lifecycle uncertainty handlers compare the exact
  publication plus the existing original domain receipts. `Recorded` binds the
  source database. Whole-batch normal rollover admission shares its gate fence
  and publication transaction; reserved stop/failure capacity stays independent.
- Lifecycle checkpoint begins retain their original Agent and Workspace fence,
  typed child record, source envelope and database identity before releasing the
  exclusive scope. Reconciliation checks those fences with parent/events in one
  snapshot. Changed domain results, later fences and foreign receipts refuse.

## Review findings and remaining compatibility

P2-5 has source and public-root regressions for uncertain commit without a
receipt, restart, independent later-head advance, foreign request refusal,
positive KnownAbort retry and precommit refusal. Gate results are recorded below.

P1-1 remains open for full ordinary-writer coverage. This checkpoint implements
the reviewed worker/control/lifecycle transaction families and real segmented
child mutations. Conversation/message-checkpoint, command admission/reuse and
unkeyed transactional append still need complete conversion and qualification.
Shared `agents.rs::save_agent`, `child_writer_claims.rs::persist` and
`agent_worker_store.rs::live_agent_connections` also retain v1-only predicates.
The last two paths need an ownership handoff; the first is S1-owned. These exact
interfaces were raised through the lead while S4 continued its assigned work.
Writer/recovery activation remains closed pending all P1 closure evidence.

## Published source and dependencies

Current pushed source: `5fdfae28133246ffbeb1d6bc5689513139d9cf95`.
Normally merged narrow S1 `f205616a4`, then develop `8d59f6154`. No mixed integration
tip was imported. The develop merge preserved both process feature flags and all
Mongo selector registrations.

Source commits include `37a4a8509` (durable pending keyed attempt), `b73af4da9`
(original worker/control/lifecycle conversion), `1020d9cdb` (scope-preserving
legacy race tests and database-bound receipts), `27efe61cd` (Mongo-time lease cap),
and `695ca2617` (source-bound uncertainty/restart fixtures).

## Measured gates

| Source / gate | Actual result |
| --- | --- |
| e363f788f, writer real Mongo, Rust1.96.1 | 33 run: 28 passed, 5 failed, 2466 skipped; 86.484s. Actual worker/control/lifecycle-child/checkpoint receipt contracts passed. |
| e363f788f, legacy transaction real Mongo, Rust1.96.1 | 23 run: 23 passed, 0 failed, 2476 skipped; 35.954s. Fresh separate disposable fixture. |
| e363f788f, preliminary strict / units | Diagnostic only: host Rust1.95.0. Strict emitted 22 distinct wiring diagnostics, zero type errors; test target repeated20. Units40 passed/2459 skipped. Runner edit contaminated strict exit bookkeeping (127); no strict pass claimed. |
| 5fdfae281, canonical Mongo runner harness | 31 passed, 0 failed; 188.263s. Mocked orchestration, not database qualification. |
| 5fdfae281, canonical secret history | Passed: 3769 candidate-history commits, zero findings. |
| 5fdfae281, fmt with Rust1.96.1 / diff whitespace | Passed. |
| 5fdfae281, file-size | Passed: 6055 files. Touched source/test and handoff notes remain below500 lines. |
| 5fdfae281, final strict / focused units | Both exited101 on Rust1.96.1: two Option-generation type errors in S2-owned `sandbox_unpublished_recovery/transaction.rs` after the develop merge. Strict also reports the unused scoped export. No unit/Mongo execution in this batch. |

The initial heavy-0 batch acquired after about50 minutes and released on completion.
It used a single target and both owned Mongo fixtures cleaned up through the runner.
Both shared lock inodes were preserved. The release lock inode is also checked without recreation. The target is retained solely for the next
batch, then removed during final cleanup.

## Corrections from the measured failures

Three bootstrap tests conflicted in preparation. Lease claim/renewal formerly
compared a host-derived deadline with a strict Mongo-time upper bound. The fix
atomically caps the deadline at server time plus120 seconds and returns the actual
stored permit. Exact holder/fingerprint/fence, gate CAS, live-server-time and
monotonic-renewal checks remain. A new regression uses a client clock one day ahead;
the expiry test obtains its short test clock from Mongo.

The low-level later-head test now pins the read snapshot to the original commit's
operation time, as the production caller already does. The public keyed restart
test models server cleanup after process loss with killSessions on its recorded
session, without delivering KnownAbort to the caller. This releases the old server
transaction lock before independent publication; the durable original claim must
still remain. Neither absent receipt nor elapsed time authorizes replanning.
All original success/foreign/abort assertions remain; the fixed run is pending.

## Remaining P1-1 coverage

| Path | Next implementation / evidence |
| --- | --- |
| Worker, stop/delete, lifecycle child / checkpoint | Implemented original shared transactions and exact receipts; previous-pin Mongo passed. Current-pin rerun pending. |
| Conversation checkpoint / admission | Convert v1-only Agent CAS in `agent_conversation_store::{checkpoint,admission}` and retain exact checkpoint anchors. |
| Command reservation / reconciliation | Replace v1 Agent fence and whole-history scans in `agent_commands::{reserve,reconcile}` with captured-source evidence. |
| Admission / reuse | Replace complete-history replay in `agent_admission_records`; preserve creation and original reuse receipts. |
| Unkeyed append | Convert legacy preparation and v1 CAS in `agent_log.rs`, retaining original operation across Unknown. |
| Shared save / claims / discovery | `agents.rs::save_agent` is S1-owned; handoff remains requested for `child_writer_claims.rs::persist` and `agent_worker_store.rs::live_agent_connections`. |
| Capacity recovery successor | `writer_prepare::assemble` still rejects CapacityRecovery admission for segmented source; independently admitted empty-batch successor remains S4 work. |
| Retained reopen | New develop dependency needs original bounded history digest/semantic/capacity proof; lead assigns root dispatch to S2, proof to S5 and retained-reopen owner. |

No missing interface blocks the implemented original receipt callers. Whole-crate
strict qualification still requires S2/S5 production composition, including
`sandbox_reconciliation::transaction::apply_scoped` and the recovery service roots.
The two original S4 writer_reconcile warning locations are consumed by real callers.
No allowances or dummy registrations were added. P1-1 stays open and writer/recovery
activation remains closed. Project9 remains In Progress.

## Final qualification and cleanup

The release-only batch acquired at00:54 after explicit lead authorization. The
shared-slot waiters were cancelled before any gate child started. Available RAM
was31% initially and at least29% throughout; no pause was triggered. The release
lock inode160988921 and both shared lock inodes remain intact.

The exact remaining compile interface is
`sandbox_unpublished_recovery::transaction::load_for_inspection`: optional
`generation` must be checked with `is_some_and` only for unpublished recovery;
segmented retained reopen must keep its explicit bounded-history-proof refusal.
S2 already implements that behavior in its combined source. A minimal matching
patch is prepared at `notes/s4-shared-merge-compatibility.patch`; the binding
ownership plan prohibits S4 edits to this root without handoff. Requested a narrow
S2 pin or authorization through the lead. No mixed integration tip was imported.

The corrected S4 writer/lease tests still await the post-compatibility rerun.
Cleanup of the one retained target follows that final gate.

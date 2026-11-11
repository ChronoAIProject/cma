# P1-3 later-progress activation caller — 2026-11-12

Part of #1351; shared publication proof supports #1350. S4 owns the service
activation caller under the lead's latest allocation. S2 owns proof/schema;
S5 owns original receipt/domain, restart/loss routing and native boundaries.

## Source and contract

Caller code: `87c8e0d48d7cd9ec268942318bd11f317f581d10`.
Qualification pin: `2dbb5d81c844f090cd3a9ee0e4d995545e59289f` on
`tl/cma-log-segments/writer-publication`.

Normally merged S2 proof branch `c4e4b995a`, S5 current activation and actual
worker/loss tests at `3aaa55395`, and develop `ce72ee311`. The develop raw-output
flag conflict preserves both independent policies and tests; its imported test
module was extracted to keep the configuration file below 500 lines. No mixed
integration branch was imported and no history was rewritten.

S4 agrees to the positive-publication invariant: `Publication::new` computes a
canonical full-Agent witness from the publisher's exact next aggregate, distinct
from the caller's domain result digest. The shared publication transaction writes
that immutable witness atomically with the aggregate and advancing head, including
revision-only writes. `current_publication` binds its proof to the certified source,
full current Agent, database and transaction. The hinted two-row lookup requires
the newest matching format-2 witness; older or absent evidence grants no authority.
Audit-only head changes can retain that exact witness without changing its meaning.
The publication-head index identity is included in the canonical schema checksum.

`RecoveryService::activation_authorized` retains fresh authority capture and
`receipts::read` in one TransactionScope. It requires unchanged original work,
then calls `receipts::require_current_result` and
`RecoveryActivation::capture_current`. Current native target/status/turn/fence
are separate from immutable original work. Later revision alone is insufficient.
The existing worker recaptures after native IO and compares the whole activation
before acknowledgement. No input replay, ordinary reconnect fallback or new fence
is introduced. No missing interface or ownership decision remains for this caller.

## Actual caller scenarios

Two new Mongo test functions in `composition/later_progress.rs` exercise:

- Durable completion through the actual accepted worker followed by an injected
  acknowledgement failure, later keyed publication, store reopen and activation
  retry. Foreign original requests and stale native fences refuse. A second
  acknowledgement failure leaves the same durable state; retry finishes activation.
- Committed and uncommitted Unknown results on the terminal-claim transaction
  after later progress. Neither outcome permits native IO. A fresh retry proves
  the original completed work and current publication, then acknowledges once.

Both assert unchanged original carrier/completion publication and no extra
Agent/publication mutation during activation. The native test API has no input
method and panics if the original claim attachment is repeated.

S5's three normally merged Mongo functions execute the real worker and loss
handler after keyed output or shared-publisher turn/input/fence progress, refuse
missing/v1/foreign witnesses, and recapture publication/parent changes during native
IO. Those tests and S2's current-publication proof tests run in the same gate batch.
The counted no-input native protocol stub is not production native qualification;
production Unsupported and default-off admission remain explicit.

## Gate protocol

Rust 1.96.1, two Cargo jobs, one `api/target`, incremental/debug data disabled.
Shared slot 1 uses the existing inode 160080032 and a blocking acquisition wait
of up to 3600 seconds. Available RAM below 10% pauses only this gate process group;
12% resumes it. The train slot is not used. A preflight attempt found slot 0
missing; it started no heavy work and did not recreate or modify any lock.

Strict CMA uses all targets/features, `--no-deps -- -D warnings`, no allowances.
Units select capacity recovery in CMA/core, current publication/original receipt,
configuration and the bounded scheduler. Mongo selects all capacity recovery and
writer persistence/publication cases, serially on an isolated pinned Mongo8.0.13
replica set with failpoints and exact owned-container/volume cleanup.

## First actual qualification and discovered boundary

At `2dbb5d81c844f090cd3a9ee0e4d995545e59289f`, strict CMA passed with zero
diagnostics; selected units passed 164/164 (115 CMA, 49 core). Actual Mongo
ran all 70 selected cases: 66 passed, 4 failed in 264.091 seconds. All six
current-publication proof tests passed. The four failures are the two new
service tests and S5's later-progress restart and recapture tests.

Actual keyed output preserves Agent revision while advancing sequence/head.
The original-result checks in S5-owned `receipts::{verify_binding,
require_current_result}` treat equal revision as unchanged and refuse that valid
progress. The original unchanged identity must compare both revision and sequence;
a Completed result with monotonic revision/sequence and either advanced still
requires the existing positive full-current-publication proof. No bound, witness,
original-result or native check is removed by the proposed correction.

S4 corrected its own test's mistaken revision-increase assertion in `211fb2d85`,
then normally merged develop `cece32bf4` at `265dac225`, retaining both independent
retained-context and recovery flags. The reserved receipt-helper correction and
S5 test expectation were requested through the lead and #1351. A concrete
three-condition patch is retained at
`notes/2026-11-12-p1-3-sequence-proof-proposal.patch`; it is not applied source.
The failed run's owned Mongo container and volume are confirmed absent.
Final corrected-source qualification remains pending.



### P1-3 actual result and sequence-only boundary — 06:15 checkpoint

Caller/test source is pushed at `76dcfb4a16d3dfc6e143ffb409fcc325fe0393c2`.
Develop `3aee1ff68` is normally merged; independent retained-JSON/raw-output/
recovery policies survive. Imported config tests are extracted unchanged so
`features.rs` stays492lines. The new exact-sequence service test establishes
successful activation before faulting the later witness, then checks missing,
legacy and foreign evidence refuse without native IO, and restoration succeeds.
It never selects a receipt by Agent revision alone. Part of #1351/#1350.

Actual heavy results at earlier `2dbb5d81c`: strict CMA0diagnostics,
164/164units (115CMA+49core), Mongo66passed/4failed of70executed. All six
current-publication proof tests passed. Keyed append advances sequence without
Agent revision; current S5 `receipts::{verify_binding,require_current_result}`
incorrectly treats equal revision as unchanged. S4's revision expectation is
corrected; S5 needs both revision+sequence for the unchanged fast path, permits
monotonic equal-revision/later-sequence progress only through positive proof,
and corrects `restart/later.rs`'s assertion/ambiguous revision-only selector.
Exact ownership delegation or a narrow S5 pin is requested in #1351; these
reserved helper/test paths are not edited by S4. No signature is missing.

The new head passes formatting, whitespace,6204-file size and configuration
coverage. Final strict/units/Mongo on corrected source remain pending. Shared
slot1 retains inode160080032; every owned lockf uses-k and3600second wait.
First failed fixture and volume were removed; the one reusable target is kept
only for immediate corrected-source qualification. Project9 remains In Progress.

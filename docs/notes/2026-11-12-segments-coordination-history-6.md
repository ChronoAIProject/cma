# Historical integration and qualification checkpoints

## Pre-review follow-up evidence — Thu 03:27 SGT

Current `d76c2171b`: pagination/maintenance/reconciliation selection **37 passed,
0 failed, 1,469 skipped**, 0.282s execution. This qualifies the extracted helper's
existing success/failure coverage for P2-N2; strict combined lint and reviewer
delta remain pending. The source/head is frozen during the current in-place batch.

Historical archive `afc60d98a`: Mongo partitions 1–2 executed 77 tests:
**74 passed, 3 failed** (38/1 and 36/2); 2,530 and 2,531 skipped respectively.
These are not current-candidate results. Two retained-assessment cases fail at
fixture `retained_assessment.rs:123`, before the assessment read, with corrupt
integrity metadata. That fixture is unchanged in the current tree. S2 owns the
fixture correction and rerun; S4 owns any actual worker-publication defect found.
Source investigation shows the fixture appends Created, then overwrites the
conversation at the same revision, invalidating the trusted semantic checkpoint.
Qualify a consistent original publication fixture; do not weaken proof validation.
Target correction after the in-place batch, focused Mongo evidence before 07:30.

The third historical failure is the S5 discovery fixture's Mongo code-121 refusal:
Completed work used a different `completed_at` from the original result witness.
Current source already sets `result.published_at` consistently; S5 reports the
actual-consumer profiler passing at `4d7f6d680`. Keep current combined execution
required; do not reopen the source-closed format/index finding from this old run.

New S1 response pin available: `fbce2aa875d515000e128c6b4f82216dbc8d4b4b`.
Its exact-source public response result is 104 passed, 0 failed, 1 ignored,
504 filtered; final Clippy/Mongo remain pending. The earlier four Mongo passes
and fixed-scale measurements belong to `b5344ef9a`, not this new pin. Normal
integration follows completion of the fixed current batch. P1-4 stays open:
S3 lifetime work and legacy sparse long-turn streaming still need their stated
bounded-work/compatibility evidence. See `2026-11-12-response-consumer-handoff.md`.


S1 integration boundary at Thu 03:27: a non-mutating merge preview of
`d76c2171b` + `fbce2aa87` retains the current unlimited compatibility vector in
`agent_log_reads.rs`. The response pin itself has the old cap, but no new change
at that boundary; a normal merge does not restore it. S1 must provide an explicit
bounded-compatibility correction against the combined source by 07:30, with
empty ranges and all adopted-record callers qualified. S3 supplies the compatible
paged transcript when admission/optimization flags close. Do not resolve this
by silently selecting the older file or sacrificing large-history availability.


### S5 restart/profiler actual pass — 69eda007acc8b8e3ed2a391894aab6c9da19b073

The clean source guard passed at `69eda007acc8b8e3ed2a391894aab6c9da19b073`. CMA capacity:67passed/0failed/64ignored/2462filtered. Actual pinned disposable Mongo8.0.13:19passed/0failed/0ignored/2574filtered in34.12s. The previously failing restart and adopted due-work cases both passed; the actual-consumer profiler passed with wire limit32/batchSize32/singleBatch:true. The completed restart test exercises real live discovery, a failed acknowledgement followed by real loss-handler activation, unchanged Agent/carrier/publication state, and refusal before native IO on changed original request, later revision, organization policy and Retiring parent. The native port is still a counted no-input stub; production proof remains Unsupported. Both owned Mongo resources were removed.

The earlier preparation Conflicts did not recur. This pass does not establish a root cause for those earlier refusals; no gate or lease predicate was weakened and no retry was added to mask them. The diagnostic runner recorded Mongo time inside its host-call bracket. Current-source core/HTTP/adapter tests are queued next, then strict lint. No PR-ready or activation claim before those gates and combined review.

S2/S4 handoff: consume this exact pin and preserve the new shared ownership. S5 retains restart/P2/P3/native boundary. Shared service/worker/TransactionScope composition proceeds under the lead's03:25 allocation. The earlier Friday-first source forecast is superseded by S5 evidence due10:00 for the Thursday16:00 combined merge; native activation remains deferred.


Archive `afc60d98a` partition 3: 36 passed, 2 failed, 2,531 skipped (125.224s).
Cumulative first three partitions: 110 passed / 5 failed / 115 executed. The new
worker-discovery failure uses the same retained-assessment fixture assigned to
S2. The transcript-preparation failure is the historical missing top-level
`last_seq` fixture already corrected by S3; keep the current combined rerun.
These results do not reverse any source-closed pre-review disposition.


### Consumed S4 composition handoff — 03:25 allocation

S4's published `08b947f93` records the lead's transfer of accepted-work
service/worker composition and the existing bounded scheduler to S4. Exact file
boundaries, isolated scheduler pins and S5's retained authority/native ownership:
[composition handoff](2026-11-12-s4-composition-handoff.md). S2 retains canonical
migration/grants and combined qualification. S4 source and actual gates are due
11:00; this is additional to P1-1/P1-N1's earlier dependency deadlines.


## Normal integration and gate replacement — pre-review 2 follow-up

Normally merged S5 `69eda007a` at `a442934d9`, S4 `08b947f93` at `399ab1b05`,
and S1 `fbce2aa87` at `8876a618a`. Conflicts retain every independent strict flag
and configuration test, captured transaction scope, S3 title facts, S1 evidence
visibility, and the broader reconciliation Mongo selector. No consumer semantics
were rewritten. `5ce631649` repairs the retained fixture through a real revision
boundary (`save_agent`); the focused actual-Mongo rerun remains required.

The `d76c2171b` pagination result remains valid: 37/0 with 1,469 skipped. Its
unacquired strict/scheduler/adapter/authenticated batch is superseded by this
combined source. The old `afc60d98a` archive ends at 110/5 across 115 tests;
partitions 4–6 were canceled before lock acquisition. No executing Cargo/Mongo
child was interrupted. Preserve all exact-source logs; neither canceled batch
is a complete pass. Unused immutable exports were removed; shared locks and the
one reusable Cargo target were retained. Gates now qualify the new combined
snapshot, with the same source-byte guard and two Cargo jobs under heavy-1.

S1 still owes an explicit compatibility-vector correction against the combined
tree; S3 owes the bounded transcript/parity half. S4's actual-scoped-root evidence
is recorded in `2026-11-12-s4-scoped-reconciliation.md`; it does not close its
ordinary claim/admission or segmented CapacityRecovery findings. S5's 19/19 Mongo
pass closes the earlier exact-result test failure, not later-progress P1-3 or
native activation. All remaining deadlines and the 09:00 disposition hold.

Current exact ownership manifest includes the consumed S1/S4 additions and the
03:25 accepted-work composition transfer; shared registration remains S2.


### S5 affected-layer tests — unchanged69eda007a

The serial existing-lock gate passed at unchanged clean69eda007a: layer2-core recovery46passed/0failed/0ignored; layer1-public recovery8passed/0failed/0ignored; layer3-adapter recovery7passed/0failed/0ignored. Cases include lost acknowledgement, exact completion after restart, unknown outcomes/no replay, stale holder/fence, parent-intent changes, authentication/input-body refusals, default-off accepted resume, output-gap/challenge refusal and typed native Unsupported without ordinary fallback. Strict Clippy for all four affected packages (CMA/core/HTTP/adapter), all targets/features and warnings denied, is queued next. One target retained only for that immediate gate.


Published combined checkpoint: `efc1fd426bef91729c11cc71974274375bd2c7dd`.
Normal push complete; Project 9 remains In Progress and milestone comments were
posted on #1348, #1349, #1350, #1351 and #1674. Canonical runner harness at the
unchanged API/scripts tree passed 31/31 (175.062s); Cargo/Docker are mocked.
Canonical history: 3,952 commits, zero findings. Combined heavy batch log:
`2026-11-12-segments-combined-checkpoint-efc1fd426.log`; currently waiting for
its first focused Mongo gate. Required later steps include strict CMA/core/public,
scheduler, response/core consumer and adapter tests, authenticated Mongo and the
bounded full Mongo partitions. No exact-current heavy pass yet. Source is an
8,079-file byte-verified immutable export; future source edits cannot change it.
Cleanup removed 0.119 GiB of superseded exports and the applied fixture patch.
One reusable Cargo target and the active source export remain; observed free disk
39 GiB, with preflight >35 GiB and an in-run 32 GiB stop guard. No develop merge.


### S5 strict result and lint correction — f07034864

At unchanged69eda007a, strict Clippy across CMA/core/HTTP/adapter, all targets/features, exited101. The normal CMA target reports21 unused-import/dead-code diagnostics from unconsumed shared roots: accepted-work discovery/service/worker, checkpoint_capture scoped validation, sandbox_reconciliation scoped publication, sandbox_unpublished_recovery scoped load/apply, lineage::workspaces, and writer_reconcile::{Outcome,reconcile}. The test target reports those applicable to tests plus one S5 fixture useless_conversion at agent_capacity_recovery_tests/fixture.rs:168. No suppression was added.

S5 fixed the redundant String.into() in a89673855. Normally merged current develop8167d01d8 and pushedf07034864, retaining hierarchy batching's prerequisite check and all independent recovery flags. S5 behavior is unchanged; the merged baseline adds separately reviewed hierarchy batching. The passing67CMA/19Mongo/46core/8HTTP/7adapter results remain tied to69eda007a, not relabelled as new-head execution. Strict lint is queued at clean fixedf07034864. S2/S4 own shared caller consumption under the recorded03:25 allocation; S5 does not edit those paths.

The first lint acquisition refused before Cargo because disk was below32GiB. Cleanup removed0.96GiB of completed test executables from the single owned target, preserving metadata/dependency cache for lint. The retry then executed and produced the diagnostics above. Mongo containers/volumes remain removed; shared locks were not altered.


### S5 local gate handoff — f07034864

Latest clean pushed pin: `f07034864a093bff3f102cbb28a724eee12184f9`, capacity-recovery branch; normal develop merge8167d01d8. S5 fixture-only correctiona89673855 removes String.into() without behavior change. Current fmt/source-size6157/secret-history3833commits-zero-findings pass. The strict rerun completed and exits101 solely on shared unused imports/dead code:21 normal-target diagnostics and10 test-target diagnostics; the local fixture lint is absent. Exact diagnostic artifact: `2026-11-12-s5-strict-f07034864.log`. These are isolated-branch missing consumers; use the transferred S4 composition and combined S2 qualification rather than removing APIs or adding suppressions.

Acceptance tests remain explicitly at69eda007a:67CMA(64ignored),19actualMongo(0ignored),46core,8HTTP,7adapter;zero failures. Completed-result restart and exact wire32 profiler pass. Later ordinary-progress P1-3 remains deferred and requires the designated S2/S4 positive current publication/activation proof API/pin; the lead has been asked for that interface. Revision growth or candidate absence never grants authority. Production native proof remains typed Unsupported. No full P1 closure, PR-ready or activation claim.

All owned build artifacts and completed-test executables are removed; no owned disposable Mongo container/volume remains. Both shared lock inodes are unchanged (160171589/160080032). Worktree remains for lead removal. S5 made no transferred composition-path edits after the03:25 allocation. The next source/gate work depends on shared pins and the later-progress proof contract; the combined PR stays with S2.

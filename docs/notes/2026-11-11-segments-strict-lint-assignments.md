# Segmentation integration and PR plan

Pre-review 2 baseline: `34bcc3667`, including normal slice/develop merges; branch
`tl/cma-log-segments/integration-20261111`; develop `1d425cffc` is included.
Mechanical follow-ups are `efc880523` and `3cb7a3348`, detailed below.
Latest code assembly `5ce631649` includes normal S1/S4/S5 merges; gates pending.
No integration PR is ready. Pre-review 2 has four unresolved P1s; P1-2 is
closed in source, while exact combined qualification remains a release gate.
The source remains default-off. Production was not touched.

The plan due Thu 2026-11-12 08:00 SGT is **one combined PR to develop**.
The schema, proof capture, writer and recovery-carrier dependencies are not an
independently qualified schema-first release. Narrow dependency branches are
handoffs, not separate release candidates. Link #1348–#1351 and #1674 without
closing an issue whose production acceptance is still missing.

## Integrated checkpoints and ownership

| Slice | Normally integrated pin | Remaining delivery / owner |
| --- | --- | --- |
| S1 readers/schema | `fbce2aa87` (readers `e955d37f1`) | Exact index drift and shared-reader qualification; response consumers now owned by S1 under the 01:55 allocation below. |
| S2 composition / integration | scheduler `541c3f7f2`; merge checkpoint `e47527cbf` | Shared registration, runtime authority, accepted-work scheduler qualification, completed-work/loss routing composition and combined gates. |
| S3 transcript consumers | `e300190a6` (`bc08dc4a0` fixes) | Fixture sequence corrected; title/usage facts use projections. Transcript still traverses lifetime data; golden parity, pagination and scale evidence remain open. |
| S4 publication | `08b947f93` | Remaining command/discovery/claim roots; response-interval metadata; segmented capacity successor; combined root qualification. |
| S5 capacity recovery | `69eda007a` | Strict BSON shape, discovery batch, completed restart/loss routing and enum boxing integrated; actual restart/authority tests and native producer qualification still pending. |
| Develop | `1d425cffc` | Normally included; later develop merges require affected checks at the resulting candidate. |

S2's new scheduler retains at most 32 candidates, executes at most two original
accepted operations per sweep, alternates due/expired streams and reuses S5's
fresh receipt-bound authority. It observes later explicit schema installation
without startup DDL. Accepted settlement remains available when new admission
closes. Its seven focused tests cover bounds, concurrency, failure isolation,
restart, cancellation and later schema installation; execution is pending.
It does not close the completed-recovery routing or native-proof gaps.

S4/develop merge conflicts retained both permission and parent-pacing flags,
Sandbox snapshot/timing flags, all segmentation flags and the existing scoped
captured-source reuse. Optional generations are compared only for unpublished
recovery; segmented retained reopen still explicitly refuses until its exact
bounded history proof exists. The S3 callback cleanup in `dcb94ae22` removes
only an unused `mut`, with no semantic consumer change.

## Lead allocation 2026-11-12 01:55

S1 owns the v2 AND legacy response retrieval/streaming implementation on
`tl/cma-log-segments/response-consumers`, based on accepted `e955d37f1`.
S2 stops editing those consumer paths and consumes the published branch normally.
S2 retains shared composition/config/exports and canonical Mongo registration.
S3 retains transcript/REST-history/frontend consumers. S4 retains shared
projection facts, publication and bootstrap. The exact existing/planned file
list is preserved in [coordination history part 2](2026-11-12-segments-coordination-history-2.md).

S4 must supply historical model knowledge, response-interval usage through the
next turn including post-terminal usage, and original terminal/prewrite-failure
references. Missing proof is a typed refusal, not inferred metadata or a lifetime
fallback. S1 consumes capture/evidence visibility changes with real callers.
The output event-count/ordinal seek contract is retained in the shared note
`2026-11-12-segments-response-event-contract.md`.

## Narrow handoffs

S1 operator/migration dependency: merge
`origin/tl/cma-log-segments/operator-migration` at
`b11107979231ec0da5683d8719722c8b4dfea15e` normally. It carries only the shared
operator/migration implementation on develop. For the known add/add conflict in
`store/migrations/agent_log_segments.rs`, retain that dependency's complete
canonical-ledger adapter, including the schema-child checksum. Registration pin
`9011a3796` remains available; preserve the union of existing selectors.

S5 frozen carrier contract: `daf605ab410fc4737633aa3997e67192d4c4112b`, branch
`tl/cma-log-segments/recovery-work-contract`. Outer BSON Int64 format_version is
2 only; internal work format is Int32 1. Preserve the original result witness
and immutable publication binding. Never register or relabel draft v1 rows.
Install strict validation, simple collation and the exact nonunique due, expired
and per-Agent discovery indexes. Retain the unique `_id_` index. No TTL/deletion.
S1 owns schema/checksum validation, S5 codec/discovery, S2 shared migration/roles.

## Current blocker ledger — pre-review 2 at 34bcc3667

Source: `notes/reviews/2026-11-12-segments-prereview2.md`. Verdict changes:
0 P0, **4 P1 + 2 P2 blocking findings**, plus one P3 nit. Prior numbered findings:
8 source-closed, 3 open, 1 regressed; new P1-N1 and P2-N2 are included below.
Source closure never substitutes for combined executable/release qualification.

| Finding / state | Accountable owner and exact boundary | Required pin / proof deadline (Thu SGT) | 09:00 disposition rule |
| --- | --- | --- | --- |
| P1-1 OPEN: adopted Agents cannot claim/reserve/target-control | **S4 / cma-seg-lead**: `store/child_writer_claims.rs`, `agent_commands/reserve.rs`, `agent_commands/targeted_control/apply.rs` and remaining ordinary roots. S5 retains due-work discovery; S1 supplies its existing captured-proof interface. | Pin 07:30; actual adoption → discovery → AgentWorkerStore claim → ordinary/contextual/targeted admission → output, plus refusal/abort, by 10:30. Do not bypass the actual claim in fixtures. | Thursday only with a complete root inventory/pin and a credible remaining gate budget; otherwise name S4 and affected adoption scope for Friday. |
| P1-4 / D6-2 REGRESSED: unlimited compatibility vector and lifetime consumers | **S1 / cma-cost** owns `agent_log_reads.rs`, response store/core/public consumers and v2 fallback dispatch. **S3 / cma-seg-2** owns `agent_log_segment_reads.rs`, transcript/usage/title consumers and bounded title-prefix reconstruction. S2 applies only agreed shared-core delegation/registration edits. | Both pins 07:30; stable continuation, v2 reads after flag closure, >10,128 rows/>4 MiB/long-turn parity and fixed requested-output query/byte/memory evidence at 1/10/100/1000 segments by 11:00. | Both halves must fit. Removing an old ceiling or retaining an unpaged projected response is not closure. Name the missing half/owner and dependent writer/reader scope for Friday. |
| P1-3 OPEN: reconnect after later ordinary progress has no positive route | **S5 / cma-seg-3**: `agent_capacity_recovery/restart.rs`, `accepted_authority.rs`, worker/reconnect integration and actual restart/loss tests. **S4** supplies any required certified successor/publication/activation evidence; S1 retains capture/proof implementation. | S5 states exact proof dependency by 05:00; contract pin by 05:30; implementation 07:30; positive later-progress actual restart/loss, fresh authority and foreign/stale refusal by 10:30. | Later revision, same incarnation or absent work alone never authorize reconnect. If the bounded positive proof is not implemented with executable cases by 09:00, name S5 and its S4/S1 dependency for Friday. |
| P1-N1 ADDITIONAL: segmented CapacityRecovery admission rejected by writer | **S4 / cma-seg-lead** owns `agent_log_segments/writer_prepare.rs` and writer prepare/persist semantics. **S5** owns real admission/preparation callers and capacity Mongo tests. Preserve the existing shared helper interface if possible. | Writer pin 06:30 for S5 consumption; actual segmented admission, atomic seal/successor/carrier, abort, Unknown/restart and wrong/closed gate tests by 10:30. | Requires independent CapacityRecovery authority and original result/source identity. No substitution of ordinary Rollover admission, premature Ready or input replay. Missing contract/implementation at 09:00 falls to Friday. |
| P2 / D6-5 OPEN: selected-consumer parity/failure matrix | **S3 / cma-seg-2** owns direct `AgentTranscriptFold`, transcript/usage projection tests and selected-reader replica-set fixtures. S1 retains response-specific parity/security tests under P1-4. | Matrix/test pin 07:30; actual legacy/segmented serialized parity, queued/steered/active/terminal/generation/gap cases, bad proofs, snapshot races, revocation after buffering, bounds and no read-side writes by 11:00. | The four repaired legacy tests alone cannot qualify new segmented readers. Unimplemented required cases at 09:00 are named for Friday with their consumer dependency. |
| P2-N2 NEW: ordinary store module is 500 lines | **S2 / cma-db-contention** owns `layer2-core/src/sandbox/store.rs` and new `store/pagination.rs`. | Source fixed at `efc880523`: existing helper extracted, root now 480 lines. Existing pagination success/failure tests and combined lint due next gate; source target 04:00 met. | Expected Thursday; requires executable checks and reviewer delta confirmation. |
| P3 NEW nit: trailing blank line in history note | **S2 / cma-db-contention** owns `docs/notes/2026-11-12-segments-coordination-history-3.md`. | Trailing blank line removed in this checkpoint; base-to-candidate whitespace check passes. Source target 04:00 met. | Mechanical Thursday correction; non-blocking, tracked to completion. |

The eight source-closed prior findings are P1-2 runtime/schema, P2-5 unresolved
attempt preservation, P2-6 child budgets, S4 Option/u64/scoped-root build gap,
S5 raw BSON shape, D6-1 required authentication, D6-3 enum layout, and D6-4 config
documentation. The old S5 batchSize P3 is also source-closed. Retain their success,
failure and combined gates; do not reopen them merely because the full build is
not yet green. Native Unsupported is intentional and is not an additional defect.

**Separate gate failure G1:** strict CMA at `34bcc3667` exited 101 on one emitted
dependency diagnostic: S3 `AgentTranscriptFold` manual Default is derivable.
Integrator applied the exact mechanical derive at `3cb7a3348`, preserving all
field defaults. S3 retains semantic ownership and the open parity/bounds work.
This is not a full warning inventory; repeat strict CMA on the next exact head.

## Exact combined gate evidence

| Source / gate | Result |
| --- | --- |
| `afc60d98a` authenticated Mongo | **1 passed, 0 failed, 2,568 skipped**. Canonical migration, format-2 witness/v1 refusal, isolated controls/work access, missing-grant and foreign-collection refusal. Build 4m22s; execution 1.789s. Owned fixture cleanup completed. |
| `afc60d98a` formatting / source size | Passed; 6,125 files in the source-size check. |
| `afc60d98a` runner / config reference | 31 mocked runner tests and 4 config parser tests passed; canonical config documentation passed. These are not actual Mongo qualification. |
| `afc60d98a` strict CMA | Exit 101: S5 `CompletedAttachment` large enum variant is the first dependency failure; not a full warning inventory. |
| `afc60d98a` adapter | **45 passed, 0 failed, 653 skipped**, 0.489s execution. Operator gate: **56 passed, 0 failed, 2,513 skipped**, 0.493s execution. Full Mongo gates remain in flight against the archive. |
| `e7e4cdda0` scheduler/config | Compilation failed: three Send/FnOnce diagnostics from the borrowed request iterator; zero tests executed. |
| `541c3f7f2` scheduler/config | **Result rejected:** reported 34 passes but omitted all seven new recovery-scheduler tests. Shared-target source aliasing invalidated this run; corrected gate will rebuild workspace artifacts and verify test inventory before execution. |

A byte-verified export under the existing `api/target` pins archived gate source
while integration continues. Guards reject a mismatched commit or changed bytes
before Cargo. The one Cargo target remains `api/target`; each heavy step acquires
and releases the existing heavy-1 lock with a 3,600-second allowance and at most
two jobs. Each Mongo partition has at most 40 selected contracts and removes its
owned fixture before the next allocation. Shared locks and other worktrees are
never removed or changed. Remove disposable exports after their gates finish.

## PR-ready gates and schedule

1. Freeze all slice heads, merge current develop normally, review the combined
   diff and preserve exact selector/authority/schema inventories. Files remain
   under 500 lines; no warning suppression or fake production reachability.
2. Formatting; strict CMA and affected-layer Clippy with warnings denied; affected
   core/API/adapter tests. Record exact source, counts, skipped tests and failures.
3. Real Mongo: transaction-scope reuse refusal; canonical schema/drift/restart and
   isolated roles; captured reads/audits; writer roots/adoption/rollover; lost ACK,
   absent receipt/restart/later-head; recovery claims/receipts/parent fences and
   oversized children. Do not weaken production proofs to accommodate fixtures.
4. Bounded consumer evidence at 1/10/100/1000 segments with a fixed requested
   page: commands/checkouts, bytes, p50/p95/max and memory. Golden v1/v2 parity,
   long turns, large histories, stable replay cursors, revocation/backpressure,
   historical models and post-terminal usage. No lifetime fallback on v2.
5. Compatible old/new readers and writers, an actual supported rollback artifact
   (#1327), default-off restart, coherent backup/restore and closing admission
   while retaining v2 reading and accepted settlement.
6. Real pinned-runtime recovery proof before recovery activation/#1351 closure:
   original home/boot/conversation, independently current native state/output,
   exact receipt/fence, crash before/after completion, no new process/input replay.
7. Canonical config/manifest, structure/size, secret and Mongo-runner gates;
   release-owner stage/kill-switch/evidence instructions; exact-head review and
   explicit lead merge relay. Monitor merged develop CI and fix forward/revert
   an actual failure within one hour. The release requires its exact green artifact.

Current target: **Thursday 2026-11-12 16:00 SGT reviewed and merged**, with
release-owner deployment in the **19:00–22:00** window. Aim for a frozen source
by 10:30, strict-green combined gates by 13:30, PR ready by 14:00, and final
review/delta closure by 15:30. The current strict-green forecast is still
12:00–16:00, so this schedule is at risk until the 09:00 owner checkpoint proves
which items fit. It is a target, not a gate result or a new owner commitment.
Items that cannot qualify in time must be named with their owners by **09:00**
and fall back to the **Friday 2026-11-13 13:00 train**. Do not drop a required gate.
Full recovery activation has **no defensible ETA while the native producer is
deferred**. A source review must retain that blocker and cannot claim #1351 done.

Activation order: explicit additive schema, compatible readers and qualified
rollback floor, small writer cohort, separately qualified capacity-recovery
cohort, then wider activation. Close new admission on correctness/authority or
unknown-outcome faults, retaining compatible readers and accepted recovery.
Never roll back to a binary that cannot read already-written envelopes/sidecars.
Require representative route/checkouts/bytes/scan evidence, a real natural
rollover and at least 36 minutes of canary observation before expansion. Target
Thursday-evening deployment and separately authorized staged activation, with
stable watch through Sun 2026-11-15; issue closure
requires each issue's own verified production evidence.

Complete chronological coordination snapshot:
[part 1](2026-11-12-segments-coordination-history-1.md) and
[part 2](2026-11-12-segments-coordination-history-2.md).

## Integration-only #1696 and branch evidence

GitHub REST reports #1696 base `tl/cma-log-segments/integration-20261111`, head `a744a5a3feab021eb3a4e5042540116ae89bd36f`, and merge `d036173eb56bd9d410ba4b8bf9b909205c40d9fa`. The merge subject is the ordinary local merge of the transcript branch into integration. After fetching origin, both `git merge-base --is-ancestor <merge-or-head> origin/develop` checks exit 1 at develop `42fbf75de`; the merge is contained by the integration ref only. The integration push marked the PR merged because integration was its base. No `gh pr merge`, develop push or protection change was performed here. Lead confirmed the integration-only merge and cancelled the revert. Keep the five delta6 blockers on the combined candidate; no develop merge is authorized.

Scheduler/config gate at `e7e4cdda0` exited 101 during compilation: the borrowed request iterator produced three Send/FnOnce diagnostics, with zero tests executed. Own commit `541c3f7f2` moves each request into its future and retains the two-attempt bound. The exact corrected-head gate is queued; source/HEAD stay fixed until completion. This correction is not yet pushed or test-qualified.


## Lead allocation 2026-11-12 02:35 — S4 runtime-loss wiring

S4 now owns the production `sandbox_reconciliation::transaction::apply_scoped`
entry, its scoped publisher and `capture_segmented_scoped`, including their
success/failure qualification. S2 stops editing those semantic paths and retains
integration, shared registrations and P1-2. S5 retains capacity recovery service,
codec, discovery and native proof. S1 owns the underlying captured-proof contract.

Handoff source is already pushed: `c13eff3b58edcc70b6ac08b52676ba5a1790f162`
contains the production dispatch in `api/crates/cma/src/store/sandbox_reconciliation.rs`.
The published combined reference is `e7e4cdda0eb94ca04ff977a442b25ddc1a4b8e8a`.
S4 at `8958e69ff31ed235e3bf25a58218cb807f2e99a0` already contains
`ecd84ecd3` and `5c255dfd9`; its four supporting files below are byte-identical to
that reference. The only missing production delta is the root dispatch (33 lines
in the original commit). S4 can consume that exact root delta in its own narrow
wiring commit; it need not reimplement the scoped helpers or import unrelated
preflight/unpublished-recovery edits from the three-file `c13eff3b5` commit.

Transferred exact paths, all under `api/crates/cma/src/store/`:

- `sandbox_reconciliation.rs`
- `sandbox_reconciliation/transaction.rs`
- `sandbox_reconciliation/transaction/scoped.rs`
- `sandbox_reconciliation/agents.rs`
- `sandbox_reconciliation/agents/scoped.rs`
- `agent_capacity_recovery/source.rs`

The source file includes raw/scoped capture and envelope-fence helpers shared
with S5. S4 owns edits to this whole file; S5 requests changes through S4, rather
than editing it concurrently. `agent_capacity_recovery/lineage.rs` stays S5-owned.

Preserve one owned transaction scope from captured v2 source through
`writer_commit::{prepare,persist}`; release it only before unchanged legacy work
in the same transaction. The format lookup is a dispatch hint, never proof or
adoption authority. Legacy races must refuse v2; terminal children are fenced;
all domain and driver failures abort the transaction at the root. Keep the
original commit/unknown-outcome reconciliation and no read-side audit writes.

S4 qualification: actual runtime-loss root with a mixed v1/v2 Sandbox and
nonempty segmented child changes, unchanged terminal child, malformed/stale
capture refusal with unchanged durable state, parent CAS race and lost commit
reply. No helper-only or empty-child test qualifies this production root.
Target source checkpoint Thu 04:00 SGT, focused Mongo result by 08:00 if the shared
slot permits; these are integration targets awaiting S4 confirmation, not passes.

## #1696 delta6 disposition

Pre-review 2 supersedes the older delta6 ledger. D6-1 authentication, D6-3 enum,
and D6-4 configuration are closed in source. D6-2 is regressed and assigned to
S1/S3 above; D6-5 remains open with the fixture half repaired. All five retain
exact source/gate evidence. The previous integration-only merge did not imply
review approval or change develop. See the current blocker ledger for deadlines.

Earlier S5 correction, response test registration, accepted 08:30 NO-GO and
combined-source gate history is preserved in
[coordination history part 5](2026-11-12-segments-coordination-history-5.md).
The current Thursday-evening schedule and exact source results below supersede
those earlier forecasts. The 01:55 response ownership allocation remains binding.

## Current delivery schedule — lead retarget to Thursday evening

This supersedes earlier Friday-first source/review targets. The accepted 08:30
NO-GO does not change. Pre-review is pinned at published `34bcc3667`; preserve
its finding references while recording each subsequent fix and exact test head.
No develop merge is authorized by this schedule alone. The normal exact-head
review and explicit MERGE relay still apply; release uses green develop CI.

| Slice / owner | Required delivery and evidence | Source / gate deadline (SGT) |
| --- | --- | --- |
| S1 / cma-cost | Publish a consumable reader/response pin: retained v2 reads independent of admission flags, indexed long-turn response/events and legacy retrieval, >10,128-entry/>4 MiB parity, current authority, event cursor/page limits and unchanged-head polling. Include exact capture/schema/index dependency inventory. | Progress/pin 07:30; executable consumer and reader evidence 11:00. At 09:00 name any unresolved item that prevents 13:30 combined qualification. |
| S2 / cma-db-contention | Integrate all pins; qualify source-closed P1-2 through the canonical migrator, exact runtime grants and discovery indexes; run current-source strict CMA, scheduler/config, adapter, authenticated Mongo and partitioned Mongo. Prove the seven new scheduler tests actually execute. Resolve shared wiring/registration deltas and track every pre-review finding. | Gate progress 07:30; itemized fallback decision 09:00; source freeze 10:30; combined gate evidence 13:30; PR 14:00. |
| S3 / cma-seg-2 | Close remaining delta6 transcript lifetime traversal with caller continuation and a bounded requested page. Execute repaired legacy fixtures plus serialized v1/v2 parity, stale/corrupt/unsupported proofs, append/snapshot, generation/retirement, revocation, no read-side writes and 1/10/100/1000-segment cost evidence. Source-only title/usage changes do not close transcript qualification. | Source/pin and fixture results 07:30; full bounds/parity matrix 11:00; 09:00 risk/fallback decision. |
| S4 / cma-seg-lead | Consume the transferred runtime-loss dispatch; qualify nonempty mixed-v1/v2 children, original scoped receipts, parent CAS and unknown outcomes. Finish ordinary v2 reserve/control/cancellation/claim roots and P2-5 root qualification. Supply exact response facts to S1. | Runtime-loss checkpoint 04:00 target remains; full caller pin 07:30; focused Mongo evidence 10:30. Report unresolved roots at 09:00. |
| S5 / cma-seg-3 | Qualify exact completed-recovery restart/loss routing, fresh personal/parent authority, strict format-2 witness/codec, discovery and total child bounds. Supply real source/gate counts after the privacy-export and enum fixes. Typed Unsupported stays honest while native proof is unavailable. | Service/restart pin and first actual results 07:30; remaining default-off protocol gates 10:30. Native activation fallback identified below. |

Each owner checkpoint must include the immutable pin, consumed dependency pins,
actual success/failure/skip counts, remaining blocker, and its execution ETA.
Use the existing shared heavy slot with two Cargo jobs and release it between
steps. Use one target per worktree and remove obsolete artifacts; never delete
shared lock files. The combined source is fixed while an in-place gate runs.

**Known fallback item, named before 09:00:** native production recovery proof and
capacity-recovery activation, **S5 / cma-seg-3**. There is still no qualified native
producer. It is a Friday 13:00 candidate only after real pinned-runtime evidence;
Friday is not a promise of unsupported capability. A reviewed default-off PR may
retain the explicit Unsupported path. Do not enable recovery or close #1351 from
mocked native ports or receipt-only tests. Required default-off protocol tests
and the remaining P1-3 routing/authority checks are still due Thursday.

At 09:00, publish a named GO/NO-GO row for each unresolved item, its dependency
impact, and the largest safe qualified scope. A missing reader/parity dependency
cannot be treated as closed because writer admission is off. The combined PR
remains the authorized vehicle; any fallback must retain its required dependency
closure and strict gates. Aim for final review by 15:30, relay/merge by 16:00,
then monitor develop CI and hand the release owner exact-head evidence for the
19:00–22:00 deployment. The release owner performs production changes/verification.


### S5 exact-source capacity gate — 4d7f6d680

At clean, unchanged `4d7f6d680`, `cargo test --locked -p cma --bin cma capacity` passed: 64 passed, 0 failed, 63 ignored, 2429 filtered. This includes all four strict raw-BSON shape regressions and the existing completed-activation/no-replay model tests; ignored Mongo tests are not counted as passed. Formatting and source-size passed (6119 files). Actual Mongo is queued separately under either existing heavy lock, retaining one reusable target and two Cargo jobs. No source mutation will occur during the run. The format-2 contract remains `daf605ab410fc4737633aa3997e67192d4c4112b`. P1-3 executable restart/loss proof, exact profiler conformance and strict lint remain pending; no PR-ready or activation claim.


## 09:00 Thursday decision checkpoint

At 09:00, report each row above as **expected to qualify for 16:00** or **Friday
13:00 fallback**, naming its primary owner and dependent slices. The Thursday
forecast requires published source, known test selection, actual results so far
and sufficient slot/runtime budget to finish combined gates by 13:30. It is not
merge or activation approval. Unresolved proof contracts, missing consumer cases
or an unspecified source ETA cannot be reported as GO. Source closure is separate
from any pending execution. Final review/deltas must finish by 15:30 for the
explicit 16:00 merge relay. Keep the already named S5 native activation fallback
separate from the default-off source's remaining P1-3/P1-N1 correctness gates.


### S5 actual Mongo result and preparation diagnostic — 69eda007a

At unchanged `4d7f6d680`, all19 S5 Mongo tests executed on pinned disposable8.0.13:17passed/2failed/0ignored in33.03s. The actual-consumer profiler PASS establishes wire limit32/batchSize32/singleBatch:true plus the existing page/count/key/document/byte bounds (P3). All carrier, unknown-result, authority, overflow and existing pacing cases passed. Restart still fails at service.admit with preparation Conflict; test-only stage tracing establishes that source capture/plan completed and the refusal occurs in the gate/admission/slot transaction, before bootstrap/native IO. The new adopted due-work case fails at append_agent_log_keyed before any discovery assertion. Neither test passes; no P1-3 closure. Owned Mongo container/volume removed.

`892aadc89` adds test-only diagnostics distinguishing writer_admission::admit from writer_gate::slots::claim. Normally merged develop1d425cffc and pushed `69eda007a`, retaining both independent feature sets and tests. The bounded600-second CMA+Mongo rerun is queued on clean fixed source, with two jobs and one target. The runner also captures a read-only host/Mongo clock bracket to diagnose server-time slot refusal; no clock change or validation relaxation is performed.


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

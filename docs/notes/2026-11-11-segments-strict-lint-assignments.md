# Segmentation integration and PR plan

Pre-review 2 baseline: `34bcc3667`, including normal slice/develop merges; branch
`tl/cma-log-segments/integration-20261111`; develop `8167d01d8` is included.
Mechanical follow-ups are `efc880523` and `3cb7a3348`, detailed below.
Latest code assembly `05e4f4634` includes proof/schema pin `c4e4b995a`,
S1 `f0d9746ef`, S4/S5 `2dbb5d81c`, config split `3fc57ec55` and develop
`cece32bf4`. Interim combined gates stay fixed at immutable `b3052f1b6`;
the final assembled head still requires its own complete qualification.
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
| S1 readers/schema | `f0d9746ef` (compatibility correction `b9d390634`; readers `e955d37f1`) | Exact index drift and shared-reader qualification; response consumers now owned by S1 under the 01:55 allocation below. |
| S2 composition / integration | scheduler `541c3f7f2`; merge checkpoint `e47527cbf` | Shared registration, runtime authority, accepted-work scheduler qualification, completed-work/loss routing composition and combined gates. |
| S3 transcript consumers | `c62ab928f` (`bc08dc4a0` fixes) | Fixture sequence corrected; title/usage facts use projections. Transcript still traverses lifetime data; golden parity, pagination and scale evidence remain open. |
| S4 publication | `2dbb5d81c` (prior composition gates `fbe139132`) | Remaining command/discovery/claim roots; response-interval metadata; segmented capacity successor; combined root qualification. |
| S5 capacity recovery | `2dbb5d81c` | Current activation domain helpers, later-progress restart/loss tests and strict BSON/discovery corrections integrated; S4 service caller and actual current-source qualification pending. |
| Develop | `8167d01d8` | Normally included; later develop merges require affected checks at the resulting candidate. |

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
| P1-4 / D6-2 OPEN: compatibility vector corrected in source; consumer bounds/parity still pending | **S1 / cma-cost** owns `agent_log_reads.rs`, response store/core/public consumers and v2 fallback dispatch. **S3 / cma-seg-2** owns `agent_log_segment_reads.rs`, transcript/usage/title consumers and bounded title-prefix reconstruction. S2 applies only agreed shared-core delegation/registration edits. | Both pins 07:30; stable continuation, v2 reads after flag closure, >10,128 rows/>4 MiB/long-turn parity and fixed requested-output query/byte/memory evidence at 1/10/100/1000 segments by 11:00. | Both halves must fit. Removing an old ceiling or retaining an unpaged projected response is not closure. Name the missing half/owner and dependent writer/reader scope for Friday. |
| P1-3 OPEN: reconnect after later ordinary progress has no positive route | **S5 / cma-seg-3**: `agent_capacity_recovery/restart.rs`, `accepted_authority.rs`, worker/reconnect integration and actual restart/loss tests. **S2** supplies current-publication proof `c4e4b995a`; S4 coordinates publisher invariants and S1 retains capture. | API source published and passed13units/9Mongo at44ad; S4 agreement/caller published2dbb5d81c and consumed by S5; current-source caller qualification target07:30; positive later-progress actual restart/loss, fresh authority and foreign/stale refusal by 10:30. | Later revision, same incarnation or absent work alone never authorize reconnect. If the bounded positive proof is not implemented with executable cases by 09:00, name S5 and its S4/S1 dependency for Friday. |
| P1-N1 ADDITIONAL: segmented CapacityRecovery admission rejected by writer | **S4 / cma-seg-lead** owns `agent_log_segments/writer_prepare.rs` and writer prepare/persist semantics. **S5** owns real admission/preparation callers and capacity Mongo tests. Preserve the existing shared helper interface if possible. | Writer pin 06:30 for S5 consumption; actual segmented admission, atomic seal/successor/carrier, abort, Unknown/restart and wrong/closed gate tests by 10:30. | Requires independent CapacityRecovery authority and original result/source identity. No substitution of ordinary Rollover admission, premature Ready or input replay. Missing contract/implementation at 09:00 falls to Friday. |
| P2 / D6-5 OPEN: selected-consumer parity/failure matrix | **S3 / cma-seg-2** owns direct `AgentTranscriptFold`, transcript/usage projection tests and selected-reader replica-set fixtures. S1 retains response-specific parity/security tests under P1-4. | Matrix/test pin 07:30; actual legacy/segmented serialized parity, queued/steered/active/terminal/generation/gap cases, bad proofs, snapshot races, revocation after buffering, bounds and no read-side writes by 11:00. | The four repaired legacy tests alone cannot qualify new segmented readers. Unimplemented required cases at 09:00 are named for Friday with their consumer dependency. |
| P2-N2 NEW: ordinary store module is 500 lines | **S2 / cma-db-contention** owns `layer2-core/src/sandbox/store.rs` and new `store/pagination.rs`. | Source fixed at `efc880523`: existing helper extracted, root now 480 lines. Existing pagination/maintenance/reconciliation selection passed37/0 atd76c2171b; combined lint and reviewer delta remain. Source target04:00 met. | Expected Thursday; requires executable checks and reviewer delta confirmation. |
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


## S4 runtime-loss ownership

The exact transferred paths, pins and root qualification contract are retained
in [coordination history 10](2026-11-12-segments-coordination-history-10.md).
S4 retains scoped reconciliation/publication and recovery source capture; S5
retains lineage/native correctness. The later 03:25 service ownership supersedes
the older general service assignment. S2 retains shared registration and integration.

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


## Prior checkpoint evidence archive

The 03:27–04:29 integration/gate history is preserved verbatim in
[coordination history 6](2026-11-12-segments-coordination-history-6.md).
It records the valid37/0 pagination test result, historical Mongo110/5 (not
current-source evidence), normally consumed S1/S4/S5 pins, and S5 exact-source
67CMA/19Mongo/46core/8HTTP/7adapter passes. The published integration baseline
is efc1fd426; its focused Mongo run later failed compilation as recorded below.
No strict or actual-Mongo pass is inferred for that combined head.

## Early proof handoff archive

The original API handoff, S4 composition integration and first pinned unit results
are preserved in [coordination history 8](2026-11-12-segments-coordination-history-8.md).
Newer source pins and executable evidence follow below.

## Frozen current-publication and activation boundaries

S2 API/schema pin `c4e4b995a` retains the immutable recovery carrier and original
result. S4 confirmed the publisher invariant in `281f92000` and published its
service caller in `87c8e0d48`, now on `2dbb5d81c`. S5 has normally consumed it.
The scoped proof and fresh authority precede `receipts::require_current_result`
and `RecoveryActivation::capture_current`; recapture/equality after native IO
remains mandatory. Native production Unsupported remains explicit.
API details: [current-publication proof](2026-11-12-current-publication-proof.md).
Prior pinned evidence and assignments are preserved in
[coordination history 11](2026-11-12-segments-coordination-history-11.md).

## Lead allocation 05:20

Lead answer #216 transfers response-consumer caller wiring to S1 on
`tl/cma-log-segments/reader-foundation`; S2 keeps all other shared composition.
Exact S1 edit paths for this narrow delta:

- `api/crates/cma/src/projections.rs`: attach the response reader to the shared
  Agent read service before legacy response/router/stream state construction.
- `api/crates/cma/src/composition.rs`: only the corresponding response-reader
  builder call in AgentProjectionServices construction.
- `api/crates/cma/src/composition/bindings.rs`: remove the redundant v2-only
  reader attachment; keep binding/gateway policy and every other caller intact.
- `api/crates/cma/src/projections/response_readers_tests.rs`: focused production
  construction regressions; register this child in projections.rs.
- `docs/notes/2026-11-12-response-caller-handoff.md`: source/gate evidence.

The route and stream handlers already call the indexed interfaces, but current
production composition installs the reader only in the v2-specific clone. The
legacy router therefore receives an unconfigured read service. The fix wires
the shared service once before route construction. Legacy optimization and
writer/recovery admission stay default-off; retained adopted readers stay usable.
The prior22/20 lint diagnostics concern S4/S5/reconciliation roots, not these
response callers. S1 will execute strict CMA and report its exact disposition
without altering other allocations. Target:10:00 for the16:00 combined merge.


Earlier c3ca integration/runner evidence is preserved in
[coordination history 12](2026-11-12-segments-coordination-history-12.md).

### S5 lint correction and develop synchronization —3aaa5539512ac97440fb665b4e3352bc0f56ae8e

Strict at5123295c3 exited101 with one unused ReconnectAction test re-export in the shared sandbox registry; all other diagnostics absent. S5 fixed its owned test in3cf361ac4 by exercising the legacy positive router decision directly and asserting zero restricted native attachments. No shared registry edit, suppression or synthetic production consumer. Exact failed log2026-11-12-s5-strict-5123295c3.log.

Develop advanced tocf1d6ce54 (#1719). The normal merge had two conflicts: sandbox/composition.rs construction and infra/api/configmap.yaml flags. The mechanical resolution preserves capacity_worker/capacity_recovery, the reviewed event-driven reconciliation builder, parent pacing=false and event-driven reconciliation=false. The3cf checkpoint push preceded completion of this newly fetched merge; it was immediately followed by the normal resolved merge3aaa55395, another fetch/merge verification and push. No history rewrite. Pushed head3aaa5539512ac97440fb665b4e3352bc0f56ae8e is clean and includes current develop. All imported feature/parser/test changes are retained.

Formatting and configuration-documentation checks pass. Root restart374lines, later249, support218. Full source-size/history scan and strict gate8 are in progress/queued on the final merged source. Core49/HTTP8/adapter7 remain passed at unchanged domain pin11f381ea0. Later-progress Mongo source is ready but its S4-owned service.rs::activation_authorized caller still needs the already-published helpers; no actual later-progress pass claimed. Native Unsupported stays typed.


### S1 response callers — source prepared, gates queued

Caller fix `982360fa279ba8f633c0fea53ac0cf7389d5ff79` on reader-foundation
attaches the retained reader to AgentProjectionServices before both legacy and
v2 construction; removes the v2-only attachment. Normal develop merge
`74dfc3406eadecabde6c3fca1b29cca1ff783735` preserves both disabled flags in
the manifest conflict. No other shared composition was changed by the fix.
Two production-factory regressions cover retrieval, ownership refusal, and
stream source failure with paging disabled. These commits are local pending
the targeted tests and strict CMA run; not a passing-gate or published claim.
The immutable gate export is queued on existing heavy-1, with two Cargo jobs.
At 05:33 SGT heavy-0 was absent; heavy-1 retained inode 160080032. S1 did not
remove, recreate or modify either lock and continues on heavy-1.


### Replaced unexecuted combined gate source —41c74a702

Normally integrated S5 `3aaa55395`, including the real legacy-router test fix
`3cf361ac4` for its one strict unused re-export diagnostic. No production
registry suppression or synthetic consumer was added. The prior combined
`c3ca9e3aa` coordinator was still waiting for narrow proof Mongo: stopped before
any Cargo, lock acquisition or fixture allocation. Its unused export was removed.
The replacement immutable source is `41c74a702`, with the same separately
locked gate list. No heavy result is attributed to either combined head yet.


### S5 strict/light PASS —3aaa5539512ac97440fb665b4e3352bc0f56ae8e

Current-source strict four-package/all-target/all-feature Clippy with -D warnings passed in2m28s, exit0; exact log2026-11-12-s5-strict-pass-3aaa55395.log. Formatting, whitespace, configuration-doc coverage, canonical file-size6192files and history3884commits/zero-findings pass. Native/worker/test code is compiled, including the later-progress fixtures. No current-head runtime pass is inferred from compile/lint.

The following CMA capacity gate initially refused before execution because heavy-0 was removed after release. Heavy-1 remains the original160080032; the acquire wrapper now skips missing/replaced locks and queues only original inodes, without recreating files. Lead coordination was requested after observing queued no-k probes; full evidence2026-11-12-s5-lock-probe-observation.md. CMA gate5 is queued on the intact slot. One reusable target retained, two jobs. A separate optional existing-only Mongo selection is prepared (gate11, explicit --skip restart::later::); the default full selection remains unchanged. Any such run will be reported as25existingcases only, with all3new later-progress functions still required after S4's caller pin.

Latest S2 source pin c4e4b995a adds the lookup name to canonical migration checksum without API change; consume normally after the executing/queued source gate. S2's latest handoff assigns S4 caller/invariant checkpoint by06:30. S5 remains active on owned gates; no activation or PR-ready claim.

S1 post-develop source-size gate at `74dfc3406`: shared
`api/crates/cma/src/config/features.rs` is 506 lines after retaining both parents'
flags and tests; its cohesive test-module split belongs to S2's shared-root
integration. S1's four caller files are 409/489/123/452 lines. The earlier size
pass preceded this normal develop merge and is not a pass for the merged head.
No flag or test has been removed to satisfy the limit. Heavy gates remain queued.


### P1-3 proof actual-Mongo PASS and current combined queue

At exact `44ad38198`, actual disposable Mongo **9 passed, 0 failed, 2,639
skipped**, 19.476s. Includes six current-proof cases, canonical publication
schema and two abort/unknown-outcome cases. The same source passed13units.
The source guard and owned fixture cleanup completed successfully. This is API
qualification, not S5 later-progress worker/native activation or combined lint.

Normal integration source `8c44436f7` includes develop `ce72ee311` raw-output
policy. Its conflict resolution preserves the original output-JSON test and
moves the new raw-output test unchanged into the existing rollout child; root
496lines, child99. Fmt, config and whitespace pass. The unused41c export/queue
was removed before any gate started. Strict CMA on immutable8c is now queued
under heavy-1, followed by scheduler/adapter/authenticated and retained Mongo.
The completed44ad export is removed; one Cargo target is retained for this run.

Operational risk: heavy-0 was observed absent while heavy-1 retained inode
160080032. Three earlier non-keeping lock probes are queued against heavy-1;
lead notified. Own gates use `lockf -k -n -t 3600` and now verify the expected
heavy-1 device/inode before Cargo and throughout execution. No shared lock was
created, removed or repaired here. Combined gates stop if its identity changes.


### S5 current caller consumed — 2dbb5d81c844f090cd3a9ee0e4d995545e59289f

At approximately05:47 SGT, normally fast-forwarded the capacity-recovery branch to the published S4 caller source2dbb5d81c, which already includes S5's3aaa55395 and S2 checksum pinc4e4b995a. Fetch/develop merge was already current; push/readback succeeded. No ownership-crossing source edit or history rewrite. The obsolete gate5 was still queued and was cancelled before Cargo/Mongo; its owned lock waiter was released. Gate10 now queues current-source CMA capacity units followed by the complete recovery Mongo selection, including all3S5 later-progress functions and2S4 service functions. The service caller now verifies original work, requires current result and derives capture_current in the same fresh-authority transaction; no signature gap remains.

Current fmt, whitespace, canonical size6199files and configuration documentation pass. Secret scan is running. Strict/current-source runtime gates remain pending; previous strict3aaa55395 and core49/HTTP8/adapter7 at11f381ea0 stay separately attributed. Native production proof remains Unsupported; writer/recovery admission stays closed. Slot1 retains original inode160080032 and is the only slot queued here. Slot0 now has replacement inode162845701, so the owned runner skips it pending coordinated baseline confirmation. No shared lock was changed. One target retained for these immediate gates; no owned Mongo fixture exists yet.

Next checkpoint: actual CMA/Mongo and strict results after slot acquisition; target07:30, conditional on the shared slot and successful runtime qualification. S2 owns the combined PR.

S1 caller tests executed at immutable `74dfc3406`: **2 passed, 0 failed,
0 ignored**, 2,597 filtered; source checksum unchanged. Public response tests
and strict CMA remain queued. The shared configuration size correction can
move the cohesive reconciliation_tests module into a child without dropping
flags or tests; S1 leaves that shared-root edit to S2 as allocated.


### S4 P1-3 caller and invariant normally integrated

S4/S5 `2dbb5d81c` supplies `activation_authorized` and two service Mongo cases.
Its explicit invariant agreement is `281f92000`: the atomic shared publisher
binds the full next Agent, head and immutable receipt even for revision-only
progress. This closes the missing signature/agreement/caller source handoff;
S5 actual worker/loss and S4 service cases remain unexecuted on the combined head.
The merge preserves the canonical blocker ledger and both owners' evidence.
Duplicate pacing declarations are kept once; the unchanged raw-output test is
registered once in S4's child module. No production permission was weakened.

S1 reported shared features.rs at506lines on its caller branch. S2 published
`3fc57ec55` on `tl/cma-log-segments/config-test-split` for normal merge;
the existing reconciliation test is unchanged and the parent shrinks26lines.
Heavy-0 restoration was relayed by the lead (new inode162845701); all owned
lockf calls already include -k, and heavy-1 keeps its guarded original inode.

### S1 required develop-sync conflict resolution

The pre-push fetch advanced develop to `ce72ee311` and conflicts directly in
`api/crates/cma/src/config/features.rs`: raw-output rollout versus the retained
quota and response branch settings. The normal merge must preserve both. Since
this now requires editing that shared file to complete the mandated develop
sync, S1 will mechanically extract the two conflicting cohesive test modules
into `api/crates/cma/src/config/features/quota_tests.rs` and
`api/crates/cma/src/config/features/raw_output_tests.rs`, preserving test names
and all flags/parser mappings. This closes the source-size failure introduced
by the combined declarations; no shared composition behavior is changed beyond
the develop merge. S2 retains every other shared-root edit. Gates retain their
actual tested source pins; configuration tests must run after this resolution.

### S1 published response caller checkpoint — b24fb1c57

Normal push complete: `tl/cma-log-segments/reader-foundation` at
`b24fb1c570858392125c8ec6e5eb2d71688b137a`; caller-only commit
`982360fa279ba8f633c0fea53ac0cf7389d5ff79`. S2 can merge normally. The latest
normal develop sync consumes `ce72ee311`, preserves all fields/functions/keys
from both parents, and keeps the shared configuration at 492 lines by moving
quota/raw-output tests into their same-named child modules. This supersedes the
earlier request for S2 to split the oversized file. Two factory tests passed at
`74dfc3406`; current configuration, public response and strict CMA gates remain
pending. Activation stays closed; this is a source checkpoint, not PR ready.


### Combined caller qualification source —b3052f1b6

The old8c strict waiter was canceled before acquiring a slot or starting Cargo;
its source export is removed. The immutable replacement includes S4 caller and
S1's narrow size dependency. Its batch is strict CMA, scheduler/config, adapter,
authenticated Mongo, all current capacity-recovery Mongo (including the3S5 and
2S4 later-progress functions), then retained assessment/discovery Mongo. Every
step separately uses the existing heavy-1 with -k -n -t3600 and2jobs.
One target retained; no combined heavy pass claimed. The S1 handoff is
[config-test-split](2026-11-12-config-test-split-handoff.md).

S1 response suite at published `b24fb1c57`: **104 passed, 0 failed,
1 ignored** (existing SDK loopback fixture), 504 filtered. This includes the
large-history, long-turn, pagination, authority and stream-error regressions.
Source checksum remained unchanged. Formatting, whitespace, source-size
(6,168 files) and canonical history secret scan (3,840 commits) also passed.
Strict CMA and final configuration/factory execution remain pending.


### Retained-context develop merge —05e4f4634

Normally merged develop `cece32bf4` (#1693). Retained both default-off flags,
builder methods, constructor defaults and ConfigMap keys. The shared
`composition::bindings::responses` helper now takes the existing
`crate::config::FeatureFlags` as its final argument, reading both named fields;
its one production call passes `config.features`. This preserves both behaviors
and avoids an eight-argument lint failure without suppression. S1 retains all
response caller semantics and can keep this mechanical signature when merging.
Formatting/config/whitespace pass; features root475lines. No caller test is
claimed for this new merged head. Final qualification must include retained
context success/refusal and paged-response tests as well as the recorded gates.
The current b305 gate source remains frozen while integration continues.

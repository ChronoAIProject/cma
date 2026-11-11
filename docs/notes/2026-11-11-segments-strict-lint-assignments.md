# Segmentation integration and PR plan

Pre-review 2 baseline: `34bcc3667`, including normal slice/develop merges; branch
`tl/cma-log-segments/integration-20261111`; develop `8167d01d8` is included.
Mechanical follow-ups are `efc880523` and `3cb7a3348`, detailed below.
Latest code assembly `de5903533` includes the narrow current-proof pin and
normal S1/S4/S5 updates; combined gates pending.
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
| S1 readers/schema | `4df2fb3d0` (compatibility correction `b9d390634`; readers `e955d37f1`) | Exact index drift and shared-reader qualification; response consumers now owned by S1 under the 01:55 allocation below. |
| S2 composition / integration | scheduler `541c3f7f2`; merge checkpoint `e47527cbf` | Shared registration, runtime authority, accepted-work scheduler qualification, completed-work/loss routing composition and combined gates. |
| S3 transcript consumers | `e300190a6` (`bc08dc4a0` fixes) | Fixture sequence corrected; title/usage facts use projections. Transcript still traverses lifetime data; golden parity, pagination and scale evidence remain open. |
| S4 publication | `984d881ee` (composition gates `fbe139132`) | Remaining command/discovery/claim roots; response-interval metadata; segmented capacity successor; combined root qualification. |
| S5 capacity recovery | `024de5168` | Strict BSON shape, discovery batch, completed restart/loss routing and enum boxing integrated; actual restart/authority tests and native producer qualification still pending. |
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
| P1-3 OPEN: reconnect after later ordinary progress has no positive route | **S5 / cma-seg-3**: `agent_capacity_recovery/restart.rs`, `accepted_authority.rs`, worker/reconnect integration and actual restart/loss tests. **S2** supplies current-publication proof `7444c0a80`; S4 coordinates publisher invariants and S1 retains capture. | API source published; S4 agreement and executable qualification target 06:30/latest 07:00; S5 caller implementation 07:30; positive later-progress actual restart/loss, fresh authority and foreign/stale refusal by 10:30. | Later revision, same incarnation or absent work alone never authorize reconnect. If the bounded positive proof is not implemented with executable cases by 09:00, name S5 and its S4/S1 dependency for Friday. |
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


## Prior checkpoint evidence archive

The 03:27–04:29 integration/gate history is preserved verbatim in
[coordination history 6](2026-11-12-segments-coordination-history-6.md).
It records the valid37/0 pagination test result, historical Mongo110/5 (not
current-source evidence), normally consumed S1/S4/S5 pins, and S5 exact-source
67CMA/19Mongo/46core/8HTTP/7adapter passes. The published integration baseline
is efc1fd426; its focused Mongo run later failed compilation as recorded below.
No strict or actual-Mongo pass is inferred for that combined head.

## P1-3 positive current-publication proof — Thu 04:42 SGT

S2 owns the narrow API/schema/publisher pin; target 06:30, latest 07:00 SGT.
S4 agreement on the publication invariant has been requested through the lead;
not yet received. S5 owns fresh authority, current activation/native confirmation
and the executable later-progress restart/loss cases. The original format-2
recovery carrier/result witness must remain unchanged. Proposed bounded contract:
unique (agent_id, incarnation, head_revision) publication lookup plus a separate
canonical current-Agent digest; domain result_digest keeps its existing meaning.
Historical receipts remain readable; absent/old current evidence refuses proof.
The opaque proof belongs to one TransactionScope/database and does not itself
provide permission to attach, acknowledge, replay input or perform ordinary reconnect.
Final signatures and consumable pin follow in a linked handoff before 07:00.

Combined efc1fd426 focused Mongo gate exited101 during compilation: title_through
is private (projection.rs145 -> agent_log_segment_reads.rs145), and the segmented
Head has no validate_lineage method (agent_log_segment_reads.rs114). No selected
Mongo test executed. S2 will supply mechanical shared-boundary fixes, preserving
Created lineage checks; S3 retains consumer semantics. Subsequent combined heavy
gates did not run. Detailed log:2026-11-12-segments-combined-retained-fixture-mongo-efc1fd426.log.


### S5 real worker-sweep checkpoint — 024de51689c662d0688370c6e9de2ea44bbdcd98

Normally merged published S4 fbe13913292cbf8230ab403685cd5d76d43bed64 at8e0a29802, including accepted-work composition, seven bounded scheduler tests, and scoped lifecycle/reconciliation consumers. Added an owned test-only router installer and strengthened the existing completed-restart Mongo test through actual AgentWorker::reconcile_connections: failed acknowledgement defers without ordinary work, the next sweep activates, and later revisions/Retiring parents preserve durable state without more native IO. Existing loss-handler and unchanged publication/carrier assertions remain. Production native proof stays Unsupported. No transferred composition paths were edited.

Pushed024de51689c662d0688370c6e9de2ea44bbdcd98 after fetch/normal develop merge (already current). Formatting and source-size6186files pass; changed files92/344lines. Strict four-package Clippy is running on this fixed clean source under one existing shared heavy lock, two jobs and one cold Cargo target. Current-head unit/Mongo execution remains pending. The earlier69eda007a results are not relabelled. The frozen format-2 discovery/index pin remains daf605ab410fc4737633aa3997e67192d4c4112b.

The lead has assigned the later-progress positive publication/activation proof API/pin to S2/S4, ETA07:00; the ownership question is answered. Continue qualifying exact completion while that proof is delivered. Missing proof or revision growth still defers. S5 evidence target remains10:00 for the16:00 combined merge; no PR-ready or activation claim.


### S5 disk-guard stop at fixed024de5168

Strict four-package/all-target/all-feature Clippy acquired one existing heavy slot and began its cold dependency build at clean024de51689c662d0688370c6e9de2ea44bbdcd98. The runner stopped it at the31GiB disk guard before project diagnostics; exit2. This is neither a compiler failure nor a pass. Exact log:2026-11-12-s5-strict-024de5168.log. Free disk subsequently measured28.844GiB, below the team30GiB floor. Removed the entire newly created target,0.236GiB allocated; no owned target or Mongo manifest/container/volume remains. Shared lock inodes remain160171589/160080032. Last free-space observation29.844GiB, still below the32GiB preflight. The lead has been asked to coordinate cleanup by the other owners; no cross-worktree cleanup was attempted.

Current-source light gates pass: cargo fmt --all -- --check; canonical file-size6186files; secret history3876commits/zero findings. The schema, index JSON and contract document are unchanged fromdaf605ab4. Remote readback confirms024de51689c662d0688370c6e9de2ea44bbdcd98; own tree is clean. All7scheduler test definitions are included in the capacity filter, but none is claimed executed on this source. Project9 remains In Progress. Latest current-head issue handoff:https://github.com/ChronoAIProject/cma/issues/1351#issuecomment-6806559557 (superseded gate-running status by this disk result).

Resume after shared disk recovery: strict gate8, CMA capacity/actual-Mongo gate10 (separate gate5+6 if needed for bounded slices), and affected-layer gate9. Use existing heavy locks/two jobs/one target and preserve exact-source attribution. The S2/S4 later-progress proof pin remains due07:00; adapt only after published interface delivery, then qualify its affected paths. No activation/PR-ready claim and no develop merge. Native Unsupported stays typed. Evidence target10:00 is now conditional on disk recovery and that pin.


### S1 explicit compatibility correction — b9d390634

Published response branch `tl/cma-log-segments/response-consumers` at
`4df2fb3d0`, code correction `b9d390634745c0d78e287ea238934187ebe18a94`;
normal develop sync `8167d01d8` at `f389f41c8`. Non-mutating merge preview
against `efc1fd426` conflicts at `agent_log_reads.rs` and `config/features.rs`.
Resolve the segmented arm to `segmented::read_compatibility_window`, retaining
its module registration; it replaces the unlimited compatibility collector,
including empty-range proof checks. Preserve every independent config flag,
parser and test, including quota and batching's snapshot prerequisite. See
`2026-11-12-response-consumer-handoff.md` for precise boundaries.

Two new Mongo cases cover independent row/byte refusal, successful small
prefix/tail reads above the lifetime limits, and empty/future/max-cursor refusal
after an uncommitted append. They are selected by the existing reader prefix.
S3 must retain compatible adopted transcript routing when flags close; this
correction is not transcript qualification or a whole P1-4 closure.

The f389 configuration build was stopped by the shared disk-floor guard before
any test executed; strict CMA refused before Cargo. After another 1.480 GiB
owned-output cleanup and shared capacity recovery, the corrective immutable
b9d390634 config/Mongo/lint run acquired heavy-1. Results pending. Existing
response104/core-public-lint and Mongo5 evidence remain at their recorded heads.


### S5 gate resume — lead answer214

At04:41 free disk was37.541GiB. Resumed strict gate8 on unchanged pushed024de51689c662d0688370c6e9de2ea44bbdcd98; blocking for either existing heavy slot, up to60minutes. Lead's explicit thresholds now govern: start at>=32GiB, pause below30GiB and retry after10minutes. The runner's old31GiB in-run guard was adjusted to30GiB; the32GiB preflight and single-target/two-job limits remain. No fixed38GiB requirement. Shared lock files are neither deleted nor recreated.

Read S4's published final composition evidence984d881e (code gates atfbe139132 already consumed):107focusedunits/56actualMongo and strict CMA pass. Those remain S4 source evidence, not execution of S5's added actual-worker sweep at024de5168. S2 has recorded ownership of the positive current-publication proof pin, target06:30/latest07:00. No duplicate proof API is introduced while its signatures are being finalized.


### P1-3 API signature checkpoint

S2 source7444c0a80 on tl/cma-log-segments/current-publication-proof defines
current_publication::{capture,CurrentPublication::require_current}. Exact Rust
signatures, schema/index, compatibility and S5 consumption are recorded in
2026-11-12-current-publication-proof.md (also committed under docs/notes).
The new immutable publication format2 carries a separate full-Agent digest;
historical publication format1 remains readable, without activation authority.
The original recovery carrier format2 and its result witness are unchanged.
S2 awaits S4 boundary agreement through the lead. Source qualification is queued
under heavy-1 against immutable7444c0a80; no pass or published-pin claim yet.
S5 may code against these signatures now. Final pin remains due06:30/07:00 SGT.


Published P1-3 source pin: `7444c0a80c8305dc3006d794aef57b3a2ddc356d`.
S5/S4 merge `origin/tl/cma-log-segments/current-publication-proof` normally.
It starts at shared08b947f93, normally merges develop8167d01d8, then adds only
this API/schema/publisher/test contract. Fmt, size6187files and diff checks pass;
unit/Mongo gates are queued, not passed. S4 invariant agreement remains pending.
No PR or activation claim; original completed recovery witness stays immutable.


### S4 accepted-work composition consumed

Normally integrating984d881ee (executable gates atfbe139132): strict CMA passed
with zero diagnostics;107units and56actual-Mongo tests passed on that slice.
Exact source evidence is in2026-11-12-s5-composition-wiring.md. This includes the
service-local test facade,6composition Mongo cases and its faults.rs child,
the7existing scheduler tests, and the default-off configuration documentation.
It does not close P1-1/P1-N1 or later-progress P1-3. Combined gates remain required.
S4 retains composition ownership; S5 retains restart/native correctness.


### Current integration assembly — de5903533

Normal merges now include current-proof7444c0a80, S4 composition984d881ee,
S5 actual-worker restart024de5168 and S1 compatibility4df2fb3d0. S3c62ab928f
is already an ancestor. Shared compile fix8b30727ea retains the Created lineage
check and exports title_through to the store; it has not yet executed in Mongo.
Feature conflict resolution preserves both parents' fields/parser/test names;
transport/quota tests were mechanically split (root460lines, child75lines).
No response/transcript semantics were authored by S2 during these merges.

Narrow API7444c0a80 is remotely verified and its exact-source unit gate waits
for heavy-1; actual Mongo follows separately. S4's passing composition evidence
remains tied tofbe139132. Combined source is not qualified or review-ready.

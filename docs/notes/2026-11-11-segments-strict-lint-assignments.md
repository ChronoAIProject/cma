# Segmentation integration and PR plan

Pre-review 2 baseline: `34bcc3667`, including normal slice/develop merges; branch
`tl/cma-log-segments/integration-20261111`; develop `1d425cffc` is included.
Mechanical follow-ups are `efc880523` and `3cb7a3348`, detailed below.
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
| S1 readers/schema | `e955d37f1` | Exact index drift and shared-reader qualification; response consumers now owned by S1 under the 01:55 allocation below. |
| S2 composition / integration | scheduler `541c3f7f2`; merge checkpoint `e47527cbf` | Shared registration, runtime authority, accepted-work scheduler qualification, completed-work/loss routing composition and combined gates. |
| S3 transcript consumers | `e300190a6` (`bc08dc4a0` fixes) | Fixture sequence corrected; title/usage facts use projections. Transcript still traverses lifetime data; golden parity, pagination and scale evidence remain open. |
| S4 publication | `6b665b1a4` | Remaining command/discovery/claim roots; response-interval metadata; segmented capacity successor; combined root qualification. |
| S5 capacity recovery | `4d7f6d680` | Strict BSON shape, discovery batch, completed restart/loss routing and enum boxing integrated; actual restart/authority tests and native producer qualification still pending. |
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

### S5 reviewer corrections — bb34b1ae6

The lead relayed the S5 pre-review at d84294383 and explicitly assigned P1/P2/P3 correction. Published commits:12cb24b93 consumes the existing S2 live-discovery and command-mount deltas from2c3537ee7/100299dcb (the narrow files only, adapting the missing independent flag to this baseline);638621867 enforces frozen raw BSON shape before Serde and on encode;f27d06c76 uses a singleBatch query with wire limit32/batchSize32;bb34b1ae6 wires completed recovery into both restart discovery and the real loss handler. Native proof remains Unsupported and the process admission flag stays false.

The raw decoder checks required nullable keys, exact Int32/Int64 widths, nested keys, runtime omission versus explicit null, string/digest/time/sequence bounds, then existing work/result semantics. Four new tests cover the review probe cases plus nested extension and size rejection. The frozen schema/index artifacts are unchanged.

The real mounted command service installs one shared receipt-aware reconnect router. Restart and stream-loss callers consult it before ordinary scheduling. A positive legacy envelope can use the existing ordinary route. A segmented candidate must match an exact current Completed result/publication, then run_accepted reacquires current personal/parent authority before and after restricted native IO. Missing candidates, another phase, later progress and errors defer; they never trigger ordinary reopening. Later ordinary-progress proof remains an explicit unavailable qualification dependency, not an inferred permission.

The Mongo restart regression now verifies actual live-connection discovery of the completed v2 Agent, fails the first acknowledgement, then retries through DurableAgentConnectionLossHandler and compares aggregate/carrier/publication state. Its native port is still a counted no-input stub. This code is not yet execution evidence. The profiler now requires batchSize32 and singleBatch:true. P2-6 is accepted fixed by the reviewer; native Unsupported is accepted as intentional.

Formatting and source-size pass6119files. The preceding d84294383 compile reached the original300-second guard before any tests; it is not a pass. Compilation now has a bounded600-second interval because repeated300-second termination discarded unfinished CMA code generation. Mongo retains its separate acquisition/window. Two jobs, one target, existing shared locks, no owned Mongo resource while queued. Sourcebb34b1ae6 is fixed for the capacity/decoder compile gate; actual Mongo, pure core/HTTP/adapter and strict lint follow. No PR-ready or activation claim.

### Response validation and Mongo qualification children

The lead-allocated response slice adds these cohesive children under its existing S1/S2-owned parents:

- `api/crates/layer2-core/src/sandbox/agent_reads/response_indexed_validation.rs`
- `api/crates/cma/src/store/agent_log_segments/mongo_tests/response_readers.rs`
- `api/crates/cma/src/store/agent_log_segments/mongo_tests/response_readers/fixtures.rs`
- `api/crates/cma/src/store/agent_log_segments/mongo_tests/response_readers/scale.rs`
- `api/crates/cma/src/store/agent_log_segments/mongo_tests/response_readers/refusal.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_tests/integrity.rs`
- `api/crates/layer1-public/src/responses/v2/indexed_tests/history.rs`


Recent full shared-note history is preserved in coordination-history-3.md and
coordination-history-4.md (local copies mode 0600 and repository mirrors).
The exact 01:55 response path allocation remains authoritative; current exact
file ownership is in the companion ownership manifest.


## Thursday 13:00 train decision — lead broadcast 02:35

**NO-GO for segmentation wave 1 by the 08:30 merge cutoff.** This was reported
to the lead before 05:00. S1 plus completed consumers is not yet an independently
strict-green, reviewed candidate: S3 has open lifetime-read and fixture/parity
blockers, S1 response work is in progress, and the shared proof/schema dependencies
have not been qualified as a reader-only release. Cutting that tree now would
require another source assembly and complete gates; do not infer safety from
writer/recovery flags being off. That initial Friday disposition is superseded
by the Thursday-evening target below; the 08:30 NO-GO remains accepted.
Qualification and exact-head review still govern. Owner priority is unchanged;
no lower-priority work is being substituted.

The existing bounded response PR #1687 is already merged to develop at
`cd3babea89a1a1a0de00e19d7807b6c073f69c1d` (Wed 19:35 SGT), from reviewed head
`41334d7c7c4c183110de9d2b67b774c7ed71a808`. Read-only GitHub inspection now shows
all 25 listed PR jobs successful. It is available to the release owner for
Thursday's batch; it needs no duplicate merge. Production activation and
representative before/after evidence remain release-owner work.

The 08:00 plan delivery is still due. Freeze develop at 08:30 as the lead directs;
only separately approved PRs with a valid explicit merge relay can land before it.
The combined PR will not be opened or activation approved from historical gates.


### S5 strict enum correction and train disposition — 4f3e7022c

Consumed the integration-reported CompletedAttachment large-enum diagnostic:4f3e7022c boxes only the retained confirmation payload and adapts the production/model constructors. Fresh authority and native confirmation semantics are unchanged; no lint suppression. The bb34b1ae6 waiter was canceled before acquisition (no Cargo/Mongo ran) to avoid knowingly testing a strict-lint failure. Pushed4f3e7022c with current develop already included; clean source is queued for capacity/decoder tests with600-second bounded compilation and separate Mongo acquisition.

Reported to the lead before05:00: S5 is NO-GO for the Thursday08:30 merge cutoff and remains on the Friday train under the02:35 plan. Restart qualification and strict gates remain pending; native production proof is deferred/Unsupported. Continue the same priority without dropping any gate. P1/P2/P3 source corrections and exact future gate evidence remain the next handoff; P2-6 is reviewer-accepted.

Allocated HTTP qualification child: `api/crates/layer1-public/src/responses/v2/indexed_tests/http.rs` (response slice).


## Gate artifact correction — Thu 02:40 SGT

The first nominal scheduler run at `541c3f7f2` reused the archive's older test
binary: 34 tests were reported but all seven `sandbox::capacity_scheduler` tests
were absent, and the archived dead-code warning remained. This is **invalid
current-source evidence**, not a scheduler pass. The in-place gate finished;
source can now be integrated again. A required test inventory check now refuses
this case before execution. The shared-target guard cleans only workspace package
artifacts whenever source identity/path changes; it preserves external dependency
cache, the active source export and the shared lock files. Both archive and
in-place runners use the same guard under heavy-1. Six mocked guard checks pass;
these are tooling tests, not Rust/Mongo qualification.

The archived operator result remains bound to `afc60d98a`: 56 passed, zero failed,
2,513 skipped after its source compilation, execution 0.493s. No newer candidate
inherits it. Strict CMA remains failed at the enum diagnostic on that old source.
The newer S5 boxed-enum pin is now available and will be normally integrated;
S3 also published a delta-fix checkpoint for inspection and combined qualification.


S5 follow-up4d7f6d680 is pushed. The4f3e7022c capacity attempt reached CMA test compilation and failed only on five E0603 diagnostics: the new store Mongo regression imported private sandbox modules. No tests executed. Re-exported the required types through the existing cfg(test) binding_lifecycle_test_support module and changed the test import; production module visibility is unchanged. Clean4d7f6d680 is fixed and queued for the rerun. The slot was released on compilation failure; no Mongo fixture was created.


## Combined assembly after the handoff — Thu 02:45 SGT

Normally merged S4 `6b665b1a4`, S5 `4d7f6d680` and S3 `e300190a6` at combined
`5e5f130a6`. Conflict resolution retained every independent default-off feature,
the two-attempt accepted-work scheduler, the actual completed-reconnect router,
and S5's additional configuration tests. S4's fresh report is preserved in
`docs/notes/2026-11-12-s4-handoff-evidence.md`: 124 owner-reported passes at its
own source and 22 shared strict diagnostics. Those are not combined results.
S4 also owns `sandbox_reconciliation/reconciliation.rs` and its focused local
root tests; shared canonical selector integration remains S2-owned.

Own mechanical correction `e47527cbf` passes the captured `Head` by reference to
S1's `visit_range` signature. No response semantics were edited. Current format,
configuration-reference and 6,159-file source-size gates pass. No current combined
strict/Rust/Mongo pass is claimed; source-switch artifact invalidation and required
scheduler-test inventory now precede the next gate. The older archive's Mongo
batch is still running/queued and retains its own exact-source evidence only.

D6-2 remains open in actual source: `agent_log_segment_reads.rs:98` starts at zero,
and `projected_transcript` supplies `expected.conversation.last_seq` at line 184.
Incremental folding removes the separate selected-log vector, but it does not
bound lifetime query work or provide a caller continuation. S3 must qualify a
requested page independently of unrelated lifetime history. This is why the
new checkpoint does not change the Thursday wave-1 NO-GO.


Latest develop sync is `63fea5a86`, incorporating `1d425cffc` normally. Conflicts
retained both parent pacing and live-fragment budgets, every independent rollout
flag and both configuration test modules. No develop merge or production mutation
was performed. The target-source guard removed 1.7 GiB of this worktree's stale
workspace artifacts before rebuilding the archive; external caches and active
source exports remain for their continuing gates. Only one Cargo target is used.


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

# Segmentation integration and PR plan

Current source: local `541c3f7f2`, last pushed `e7e4cdda0`, branch
`tl/cma-log-segments/integration-20261111`; develop `42fbf75de` is included.
No integration PR is ready. All four pre-review P1s remain activation blockers.
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
| S2 composition / integration | `0eba12933` plus merge checkpoint `dcb94ae22` | Shared registration, runtime authority, accepted-work scheduler qualification, completed-work/loss routing composition and combined gates. |
| S3 transcript consumers | `a744a5a3f` | Replace lifetime traversal with indexed requested-page facts; preserve readers when flags close; golden parity and scale evidence. |
| S4 publication | `e094ffeea` | Remaining command/discovery/claim roots; response-interval metadata; segmented capacity successor; combined root qualification. |
| S5 capacity recovery | `c3c8c5f59` | Admission-conflict diagnosis, completed-work/loss routing proof, v2 due-work discovery and native producer qualification. |
| Develop | `42fbf75de` | Normally included; later develop merges require affected checks at the resulting candidate. |

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

## Pre-review closure ledger

| Finding | Owner | Current status / evidence required |
| --- | --- | --- |
| P1-1 ordinary writers after v2 adoption | S4, with S1 aggregate save and S5 due discovery | OPEN. Original worker/control/lifecycle source is integrated; reserve/contextual-answer/targeted-control/cancellation, restart discovery, claims and aggregate saves must all preserve exact v2 authority. Test real adopted fixtures through public roots and unknown outcomes. |
| P1-2 runtime authority/schema | S2 with S1/S5 | OPEN, real isolated-role gate now passes at `afc60d98a`. Latest exact-index drift changes still need combined Mongo qualification and review. No ad hoc carrier installation in qualification. |
| P1-3 completed recovery activation | S5 proof/runtime; S2 shared routing | OPEN. Exact registry consumer and resumed completion source are integrated. Completed-work restart/loss routing must intercept ordinary reconnect; later progress needs positive current supersession proof. Native producer is deferred/Unsupported. |
| P1-4 indexed response/transcript | S1 response; S3 transcript | OPEN. S3's new helper still scans from zero through the entire head and accumulates matching rows; its title stop does not stop scanning. Required readers must survive flag closure and must not authorize legacy fallback for v2. |
| P2-5 unresolved keyed attempt | S4 | Source fix and corrected owner-reported root Mongo tests exist. Await combined rerun and exact-head review; a missing receipt never proves KnownAbort or permits a new attempted result. |
| P2-6 total child budgets | S5 | Count/byte bounds are integrated; owner-reported oversized-child tests pass on an earlier S5 source. Await combined qualification and review. |

No finding closes merely because a warning disappears or source is integrated.
S4 reports 47 unit/config and 76 actual-Mongo passes at `e094ffeea`; its strict
run still has 22 shared diagnostics. Those counts stay bound to that slice
head: [S4 evidence](2026-11-12-s4-postreview-evidence.md).
S5's last reported actual-Mongo run at `c15bb553f` was 16 passed / 2 failed;
wire-batch diagnostics and admission isolation are in `c3c8c5f59`, awaiting rerun.

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

Earliest strict-green estimate remains **Thu 12:00–16:00 SGT**, conditional on
real wiring, remaining slice deliveries and build availability. The estimate
is not a test result or an owner commitment. Source review target is Thu
18:00–22:00; Fri 06:00 is the intended merge window after review/authorization.
If that misses the artifact/train, report the slip and use the approved Sat train.
Full recovery activation has **no defensible ETA while the native producer is
deferred**. A source review must retain that blocker and cannot claim #1351 done.

Activation order: explicit additive schema, compatible readers and qualified
rollback floor, small writer cohort, separately qualified capacity-recovery
cohort, then wider activation. Close new admission on correctness/authority or
unknown-outcome faults, retaining compatible readers and accepted recovery.
Never roll back to a binary that cannot read already-written envelopes/sidecars.
Require representative route/checkouts/bytes/scan evidence, a real natural
rollover and at least 36 minutes of canary observation before expansion. Target
staged activation Fri–Sat and stable watch through Sun 2026-11-15; issue closure
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

## #1696 delta6 blockers retained on the combined candidate

Source review: `notes/reviews/2026-11-11-pr1696-delta6-review.md`, exact head
`a744a5a3feab021eb3a4e5042540116ae89bd36f`. Integration merged the work, not the
review verdict. All five findings must have closure evidence before the combined
PR. S3 owns the consumer fixes and tracks this review with S2; shared failures
retain their actual slice owners below.

| Delta6 finding | Owner | Current evidence / next gate | Target (SGT) |
| --- | --- | --- | --- |
| D6-1 P1 required authentication ambiguity | S5 source; S2 integration | Explicit required `FromRequestParts` qualification is in `45fc44e33` and current source. Auth-required/public route tests and current combined compile remain the closure gate; source-only correction is not a new CI pass. | Next combined compile; route gate by Thu 08:00 target. |
| D6-2 P1 lifetime transcript/usage/title traversal | S3 | OPEN. `agent_log_segment_reads` still loops from zero to the lifetime head, retains all selected rows, and title completion does not stop the outer scan. Use bounded maintained facts/pages with continuation; 1/10/100/1000-segment fixed-page cost evidence must hold. | Source Thu 10:00–12:00; scale gates by 18:00 estimate. |
| D6-3 P2 recovery enum strict lint | S5 | OPEN. Archived strict CMA at `afc60d98a` reproduced `CompletedAttachment` large-enum diagnostic. Narrow boxing correction plus caller/test updates requested; no lint allowance. | Next S5 pin; combined strict run after integration. |
| D6-4 P2 configuration references | S2 | Source corrected in `ce9da2336`; current `node .github/scripts/check-config-docs.mjs` passes for all quoted keys. Review must confirm defaults, prerequisites and retained-reader/settlement behavior; rerun at final candidate. | Local gate passed Thu 02:31; final candidate rerun. |
| D6-5 P2 fixture/parity/failure matrix | S3 | OPEN. Legacy transcript/usage fixtures still omit persisted top-level `last_seq`; their four earlier failures precede assertions. Repair fixture authority, then actual replica-set legacy/v2 parity, generation/retirement, stale/corrupt/unsupported proof, snapshot/append, revocation-after-buffering, bounds and no-read-writes matrix. Do not weaken production validation. | Fixtures by Thu 06:00 target; full consumer matrix by 18:00 estimate. |

Readiness remains one combined default-off PR, with no schema-first shortcut.
Earliest strict-green estimate stays Thu 12:00–16:00, conditional on source and
build availability; source review Thu 18:00–22:00, intended Fri 06:00 merge window.
All four prereview P1s and the five delta6 closure checks gate activation/readiness.
Native recovery proof remains Unsupported; no full recovery activation ETA is
claimed. The cancelled revert does not resolve a source or test blocker.


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
writer/recovery flags being off. The combined candidate remains for Friday,
subject to its actual gates and exact-head review. Owner priority is unchanged;
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

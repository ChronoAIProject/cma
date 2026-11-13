# Historical integration checkpoints before the Thursday schedule

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

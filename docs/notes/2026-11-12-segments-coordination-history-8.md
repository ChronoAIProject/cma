# Early current-publication handoff and gate history

## P1-3 positive current-publication proof — Thursday source handoff

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


### Prior slice gate/resource history

The first worker-sweep pin024de5168, its disk stop/resume, the S1 compatibility
correction boundary and S4 composition evidence are preserved verbatim in
[coordination history7](2026-11-12-segments-coordination-history-7.md).
Later actual results and the current proof/activation pins remain below.

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


### S5 strict PASS — unchanged024de5168

After the lead's disk-recovery relay, gate8 acquired an existing shared slot and passed in3m52s at clean unchanged024de51689c662d0688370c6e9de2ea44bbdcd98: cargo clippy --locked -p cma -p layer2-core -p layer1-public -p layer3-adapter --all-targets --all-features -- -D warnings; exit0. The earlier unconsumed-root diagnostics are resolved by the normally merged S4 source; no suppressions/synthetic consumers were introduced. Exact log:2026-11-12-s5-strict-pass-024de5168.log. The earlier disk-stop log remains distinct. One target retained only for the immediately following CMA capacity/real-Mongo batch, queued via gate10 with both existing locks eligible; no simultaneous heavy runs. Current-head test execution remains pending. Native Unsupported and the later-progress proof dependency remain unchanged.


### P1-3 current-publication units —7444c0a80

Exact-source unit selection passed12/0,2634skipped,0.171s after4m18s build.
Includes full current-Agent binding, historical-v1 refusal as current authority,
strict v2 witness shape, original recovery receipt preservation and tamper refusal.
The narrow branch emits4known unpublished-recovery reachability warnings; its
later shared wiring is already integrated, but this unit pass is not strict lint.
The source guard passed. Actual-Mongo proof/index/restart/abort contracts are
queued separately on heavy-1; no Mongo pass yet. The lock is released between
steps, with one target retained for the immediate next gate.

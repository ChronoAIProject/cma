# Segmentation integration and PR plan

Current combined source: `dcb94ae22`, branch `tl/cma-log-segments/integration-20261111`.
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
| `afc60d98a` strict CMA / adapter / full Mongo | Separately queued/run under heavy-1 against its verified source archive; no current result claimed. |
| `dcb94ae22` scheduler / new merge tests | Pending. No older result is attributed to this source. |

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

SEGMENTS PLAN READY notes/2026-11-11-log-segments-plan.md slices 5

Prepared 2026-11-11 SGT. Integration owner: cma-db-contention. Assigned engineers: cma-cost, cma-db-contention, cma-seg-2, cma-seg-lead (writer engineer), cma-seg-3. Dedicated reviewer: cma-reviewer-3; security/compatibility review also required for authority, publication and recovery changes. This replaces the earlier next-week/Thu-18:00 planning target. Scope is implementation, delivery and stabilization through Sun 2026-11-15. Source integration target is Fri 11-13 06:00 SGT at the latest; weekend trains are approved.

**Current integration constraint: DEVELOP MERGE FREEZE is active until the lead relays `MERGE FREEZE LIFTED`.** Previously authorized merges are paused. Continue topic-branch work and reviews; do not merge any PR to develop or change protection. Train W needs one fully green exact-head run. Run42993284820 ended with23green/2cancelled jobs, not a green release artifact. All merge windows below are conditional on the lift.

The plan assigns five parallel slices, including the two user-facing #1349 gaps. Historical checkpoints supply reusable code, not a certified integrated implementation. The code target is feasible only if the reader/writer contracts freeze tonight and integrated tests start Thursday; the upper estimates leave little review/fix margin. No safety gate is waived to meet a date. Production changes remain with the release owner, through the lead.

**Scope and ownership boundary**

- #1348: compatible readers, durable format, bounded range verification, migration/schema, audit, retention and actual executable compatibility evidence.
- #1349: bounded response projection; transcript pagination/browser/SDKs; every remaining foreground consumer, including command/publication/checkpoint/recovery evidence. Previously merged #1634/#1644/#1653 are foundations, not closure of this issue.
- #1350: atomic adoption/rollover, projection maintenance and exact publication/unknown-commit receipts, default-off.
- #1351: capacity-only recovery with explicit current-state dispositions, original identity/receipts and stop/delete intent preserved, independently default-off.
- Already assigned outside these slices: Sandbox detail to cma-usage-ui, #1665 context lookup to cmaeg-release, 429 tiering to cma-abuse, runtime-ledger tracks A/B/C to cma-migration/cma-security/cma-admission-scan. Segmentation does not alter those ledger algorithms. Coordinate any shared-file need through the lead before editing.
- Obsolete-domain/retained-host cases are excluded following the owner directive. This does not remove retained event/history/receipt compatibility, or the active CMAEG context lookup assigned to #1665.
- No event deletion, new empty Agent/conversation, reset, TTL/archive, synthetic user input, general Failed→Ready patch, pool increase or runtime replacement is part of this plan.

**Evidence determining the order**

Sources: `2026-11-11-perf-fix-map.md`, `2026-11-11-user-unavailable/fleet.md` and their SHA-256 manifests. Window: Nov 10 17:00–Nov 11 17:00 SGT, before Train W activation. The labelled-scan denominator is 222,617; another 212,281 scans lack a caller label. These are scan shares, not checkout-wait shares or causal percentages of unavailable Agents. Full-prefix bytes can remain even when a complete-validation scan disappears.

| Complete-scan caller | Count / labelled share | Existing work and remaining obligation |
| --- | ---: | --- |
| command_admission | 136,442 / 61.29% | #1591/#1633 + fleet #1651 reduce legacy scans tonight; S4 must preserve exact admission evidence on segmented Agents. Do not duplicate their preparation/quarantine implementation. |
| worker_publication / close_process | 17,117 / 7.69% | #1657 bounds eligible failed-close work, default-off; S4 covers all publication paths across segments. |
| capacity_settlement / close_process | 15,704 / 7.05% | S4 bounded publication/capacity evidence, S5 recovery dispositions. |
| remove_worktree; ensure_process_closed | 12,858 / 5.78%; 10,054 / 4.52% | S4 exact worker receipts; S5 bounded lifecycle/recovery checks, preserving fences. |
| public_window_prefix | 9,434 / 4.24% | #1634 bounds prepared page proofs. S2 removes full response prefixes/poll rebuilding; S3 replaces full transcript materialization. Counter mixes routes and is not a route-specific attribution. |
| publish_connection | 7,220 / 3.24% | S4 bounded publication; S5 capacity recovery and scoped retry pacing. |
| checkpoint; title_discovery | 4,055 / 1.82%; 4,022 / 1.81% | S5 checkpoints; #1644 titles plus S3 segmented title projection. |
| runtime_reconciliation; remove_home; publish_ready | 2,022 / 0.91%; 1,709 / 0.77%; 1,420 / 0.64% | S4/S5 bounded receipt and lifecycle evidence. |

The Nov 10 attribution had admission 43.73%, public-window 15.00% and worker retries roughly 30%; it is a different window, not the post-W remainder. Refresh attribution after W/fleet100 before claiming a saving.

| User-facing gap | Measured latency | Assigned slice / source read path |
| --- | --- | --- |
| GET `/api/v2/responses/{id}` | p50 7.773s / p95 12.225s; N=2,111 | S2; `responses/v2/routes.rs:238,242` → `stream.rs:21,32` → core `agent_reads/stream.rs:120` complete_log. |
| GET `/api/v2/responses/{id}/events` | p50 5.608s / p95 8.213s; N=747 | S2; `routes.rs:304,308`, `stream.rs:46,95,102`: initial complete snapshot and repeated 250ms polling. |
| POST `/api/v2/responses` initial observation | successful p50 13.049s / p95 18.884s; N=390 | S2 after #1591 admission; `routes.rs:200,214,224`. Correct 429 tier is external work. |
| GET `/api/v1/agents/{id}/transcript` | p50 2.758s / p95 5.395s; N=73 | S3; REST `agent_reads.rs:128,135` → core `agent_reads/authorized.rs:81`, `agent_reads.rs:221`. |
| Snapshot / events/head | snapshot p50 7.622s; head p50 4.404s | W #1634 partial; S1 supplies segmented range proof, S3 current-state/history consumers. Head N=35 is small. |

Paths above are relative to `api/crates/layer1-public/src/` or `api/crates/layer2-core/src/sandbox/` as indicated, at inspected source `85bb5c2370068fde06884a09b50df7e46b5abc3a`. Full file:line chains and limits are in the perf map. The owner's conversation-activity observation (>5s for 75%) reinforces S2 priority; do not infer a checkout causal fraction from it.

**Located implementation and what is actually validated**

| Input | Current state | Reuse / missing gate |
| --- | --- | --- |
| Reader checkpoint `4db1c7cfc3ab417e0fa15769fd447b5f7c24c10c` | Local `tl/cma-log-segments/readers` in cma-cost's worktree; 169 changed files, +12,832/−882, 12 unique commits and 850 behind inspected develop. Full merge produced 34 conflicts and was safely aborted. No synced reader PR. | Reuse codecs, raw-BSON seals, ranges, audit/rebuild, projection fixtures and client pagination by assigned file. Reconcile #1633 quarantine/trust, current migration runner, newer bounded consumers and replacement paths. Never merge the old 169-file tree wholesale. |
| Reader validation | Historical checkpoint fmt, clippy, secret scan and Mongo-runner inventory passed. Fresh Mongo/codec suite:32passed/2failed. Near-capacity audit hits Mongo388 (storage-engine cache); task-status snapshot path hits Mongo72 (atClusterTime without snapshot concern). Six touched test files exceed sprint 500-line limit despite repo exemptions. | S1 must fix the audit resource failure; S3 must fix coherent task-status read concern, without weakening fixtures. No post-sync pass. Actual predecessor executable and permitted rollback-image matrix remain outstanding. Earlier browser/SDK results are historical, not current certification. |
| Old integration `d6a29f8b2d1458c8af536d8fb35463d0601993f0` | Reader checkpoint plus six unregistered writer drafts. | Preserve this branch and history. It has no executable rollover or capacity-recovery proof. |
| Writer readback `9bb7d61f0528aa77ba45a5c2cca3fbb4075e42d4` | Owner's `tl/cma-log-segments/writer` normal develop integration excludes the old reader tree; six unregistered draft files, 1,491 added lines. | Gate/admission and BSON-planner drafts are design input. Register, reconcile, implement and test actual transaction wiring before claiming a writer. Formal assessment is published at `da4352c54c3d622cc26da874ece97bac0592a781:docs/notes/2026-11-11-segments-writer-assessment.md`. Executable legacy baseline:65unit/6Mongo tests, fmt/clippy/size/secret checks passed; dormant draft tests remain unregistered. Mongo8.2.4 differs from CI8.0.13. |

Assessment sources: `2026-11-11-1348-reader-assessment.md`, `docs/notes/2026-11-11-segments-writer-assessment.md` at `da4352c54c3d622cc26da874ece97bac0592a781`, and original `2026-11-05-agent-log-segments-design.md`. Inspected develop baseline `8db061e4af4ccbbb77d172f8dad98459b7ab3a26`; every author fetches current develop before starting/pushing. Historical predecessor pin `0932e58cb6c821c4cdc69f2cb5d82fe538b8c2b6` must be supplemented by the actually permitted pre-activation rollback artifact under #1327.

**Five slices and review targets (all SGT)**

Each slice has one engineer and one file owner. Estimates are focused implementation plus targeted gates, excluding reviewer queues and full CI. Small commits may be reviewed incrementally; do not create unplanned PRs. Use `Part of #1348/#1349/#1350/#1351` as appropriate until the entire issue acceptance is met. No slice alone claims to close the complete segmentation feature.

| Slice / branch | Engineer / issue | Deliverable and dependencies | Estimate / review-ready target |
| --- | --- | --- | --- |
| S1 `tl/cma-log-segments/reader-foundation` | cma-cost / #1348 | Port envelope v2, immutable codecs/seals, format dispatch, bounded ranges, schema/adoption verification, fenced audit/rebuild and retention guards. Register migration through S2-owned plumbing. S1 fixes near-capacity audit Mongo388. Publish format/range contract first; supports S2–S5. | 18–26h. Format/range contract Wed 21:00; first executable port Thu 06:00; complete reader/compatibility candidate Thu 18:00, final fixes Thu 22:00. |
| S2 `tl/cma-log-segments/response-projection` | **cma-db-contention** / #1349, #1320 | Bound response GET/replay/POST initial snapshot, immutable command→turn/model evidence, paged response events and unchanged-head optimization with current authority. Own shared trait/config/registration glue and integration. Legacy optimization can ship independently default-off; segmented emission waits for S1/S4. | 8–12h response + 6–10h integration/qualification. Response review Thu 10:00 (Thu 14:00 window); integrated contracts tonight 21:00; integration proof Thu 22:00. |
| S3 `tl/cma-log-segments/transcript-consumers` | cma-seg-2 / #1349 | Public transcript/history/current-state projections, pagination inside a long turn, browser + TS/Rust SDK hydration, usage/answer/title/suggestion/provenance/approval reads. Consume S1 proofs and S4-maintained projection format. No lifetime vector on segmented foreground reads; fix task-status snapshot Mongo72. | 18–26h. Trusted legacy transcript candidate Thu 10:00; complete clients/segmented-consumer candidate Thu 20:00; fixes Thu 23:00. |
| S4 `tl/cma-log-segments/writer-publication` | cma-seg-lead / #1350 and transactional #1349 | Single transactional publication helper; bounded current-state/evidence update planner; adoption and whole-batch rollover; command/worker/lifecycle receipt integration; revisioned gate and unknown-commit reconciliation. Depends on frozen S1/S2 contracts, not completed clients. Activation additionally depends on every S3/S5 consumer gate. | 20–28h. Planner/receipt contract Wed 21:00; first Mongo atomicity tests Thu 10:00; full writer candidate Thu 20:00; fixes Thu 23:00. |
| S5 `tl/cma-log-segments/capacity-recovery` | cma-seg-3 / #1351 and recovery #1349 | Bounded checkpoints/maintenance/unpublished recovery; default-closed capacity recovery admission/status/dry-run; preserve original home/native/commands; nine-case disposition; scoped PublishConnection retry pacing to stop further diagnostic growth. Domain fixtures can start immediately against ports; integration uses S1/S4. | 16–24h. Dry-run/domain contract Thu 06:00; recovery + checkpoint candidate Thu 20:00; fixes Thu 23:00. Pacing is a separate single-purpose commit and review section. |

I take S2 and integration. cma-seg-lead owns only the writer slice; the legacy name does not transfer integration authority. cma-reviewer-3 reviews S1/S2 first as contracts arrive, then S3/S4/S5; security review runs on exact authority/receipt/recovery heads. No additional engineers are assumed. If capacity increases, the best sixth assignment is compatibility/load qualification under S2's ownership handoff; seventh is SDK/browser work under a complete S3 file handoff. Until a recorded handoff, the listed owner remains exclusive.

**Strict file ownership and merge mechanics**

The companion `2026-11-11-log-segments-ownership.tsv` is the exact local path manifest; the GitHub copy is `docs/notes/2026-11-11-log-segments-ownership.md`. Both carry the same file-to-owner mapping. It covers all 169 historical reader-diff files, six writer drafts and current/new interface reservations. An entry reserves ownership; it does not authorize copying obsolete contents or require changing an unused file. Historical files absent on develop are new-file candidates. All touched source/test files must be under 500 lines, including historical test files currently exempted by CI.

| Slice | Exclusive file families; consult TSV for exact paths |
| --- | --- |
| S1 | `cma/store/agent_log_segments/{codec,digest,envelope,collections,schema,reads,audit,mod}.rs`, audit children; current `agents.rs`, `agent_log_integrity.rs` + preparation/scan, `agent_log_reads.rs` + integrity/tests, `agent_log_store.rs`, `agent_log_context.rs`, `sandbox_worker_store.rs`; core `agent_reads/window.rs`; predecessor/performance harness; reader tests/docs. |
| S2 | Public `responses/v2/{routes,stream,projection,error,mod}.rs` and new response-window modules; core `agent_reads/stream.rs`, new response modules; common root `agent_reads.rs`, read types, store traits and memory-contract ports; all top-level registration/config/schema inventory/CI runner glue listed in TSV, main/store/sandbox/test roots, `infra/api/configmap.yaml` default-off declarations, integration/design docs. |
| S3 | Public `agent_v2` history/state/projection/shapes/error/parse and REST transcript; core transcript/authorized/answer/fold/usage/suggestions; store title/suggestion/approval/provenance/task-status/trigger-context consumers; store segment projection **readers**; browser Agent hydration/navigation/shared-history; relevant TS/Rust SDK paths and tests. Global `agent_v2/mod.rs` stays S2-owned. |
| S4 | `agent_log.rs`, append receipt modules; segment `publication.rs`, `command.rs`, `projection.rs` and `writer_*`; core `agent_log_projection*` and command evidence/dispatch helpers; admission/reserve/reconcile/targeted-control/conversation/worker/lifecycle transaction call sites; corresponding transaction/receipt tests. |
| S5 | Store checkpoint capture/checkpoints, maintenance evidence, sandbox reconciliation/unpublished recovery; new capacity-recovery modules; core recovery transition/work validation files; worker capacity/recovery/connection retry/pacing; recovery API/status and tests/docs. It calls S4's publication helper; it does not edit S4's transaction or gate files. |

Registry ownership is intentional: authors send required module/export/trait registrations through the lead, and S2 commits the small plumbing changes to the new `tl/cma-log-segments/integration-20261111` branch. S1 alone owns the segment module registry and adds agreed writer/read child declarations. Each slice consumes this common base by normal merge. Do not change another owner's file to unblock compilation. New focused modules belong to their slice; new shared files require manifest allocation before editing. Parent test modules are registered by their owner. No unregistered dead drafts may enter a review-ready PR.

All topic branches start from current `origin/develop` plus the agreed contract commit. The historical local `readers` branch and old `integration` are preserved; use the new reader-foundation and integration-20261111 names. Preserve the writer assessment branch too: S4 starts writer-publication from the common current-develop contract base and ports only the focused drafts. Do not carry the old reader merge resolution into the implementation ancestry. No force push/rebase/reset of published history. The integration branch is a test/contract lane, not a substitute for the planned narrow PRs to develop. Merge develop into each topic before push; exact-head review follows any substantive delta. After merge relay, use the current owner-approved admin procedure; leave owner-controlled `enforce_admins` as directed, never change it merely because an older runbook says so. Already-failed jobs block merge; pending PR CI does not. Release batch CI must be fully green.

**Interfaces to freeze by Wed 21:00**

Names below are contract names to map onto existing types where possible, not permission to create parallel frameworks. Keep #1633's legacy trust/preparation/quarantine semantics. S2 owns the cross-layer traits; S1 owns durable reader format; S4 owns durable publication/projection write schemas. No second schema for the same fact.

| Contract | Producer → consumers | Required semantics |
| --- | --- | --- |
| `CapturedLog` / `LogRange` | S1 → S2/S3/S4/S5 | Agent + immutable creation incarnation, format, exact aggregate revision/global through, active interval, seal identity, projection format/through and quarantine state. Legacy/segmented explicit dispatch. Bounded visitor/page and exact point proof; touched boundary neighbors; no audit/quarantine writes on read fallback. Missing segmented proof is error, never legacy fallback. |
| `ProjectionRead` | S3 read adapters + S4 durable schema → S2/S3/S5 | Versioned bounded current-state, per-turn model/native/usage facts, per-command/approval/output references; source through + seal/head binding. Growing sets use indexed rows/pages. Stale/ahead/unknown proof fails closed; no synchronous full rebuild. Unbound command is a distinct state, not absent output. |
| `ResponseProjectionPage` | S2 using S1/S3 → public response handlers | Exact command/response/turn binding, immutable turn model, frozen source high-water, existing deterministic response replay numbering, bounded source/output page and continuation. Preserve personal/shared redaction and final authority check after backpressure. Unchanged head alone cannot bypass command-status or authority changes. |
| `ProjectionDelta` | S4 (with S3 semantic fixtures) → all append call sites | Pure bounded event-fold planner keyed to expected revision/through, scalar current state plus indexed per-entity upserts. Every event writer updates this in the same publication transaction; no full transcript/queue arrays in head. Mid-turn/usage-generation/steering continuity is exact. |
| `PublicationIdentity` / `PublicationPlan` / exact receipt | S4 → its callers and S5 | Stable key+fingerprint+timestamp before retry; original fences; encode whole batch before capacity choice; checked sequence/byte arithmetic; one existing caller transaction/session. Exact receipt/event interval reconciles after later appends. Unknown or missing outcome never authorizes resend. |
| `BootstrapPlan` / audit progress | S1 validation + S4 commit → S5 | Validate bounded legacy prefix once, build bounded projections, recheck exact aggregate revision/head and creation identity under CAS. Rebuild later in one-segment fenced units with captured through; reader emits unavailable if proof missing, never performs migration writes. |
| `CapacityRecoveryPlan` | S5 → S4 commit + existing runtime adapter | Original failure/operation identity, current parent/stop intent, same home/native authority, dry-run disposition and stable recovery receipt. Storage-only rollover changes no event sequence. Ready requires real runtime confirmation; cleanup stays cleanup. |
| revisioned activation/status | S4 gate implementation + S2 config/plumbing → release owner | Missing/disabled gate closed; CAS revision/status, deterministic staged cohort, no startup blanket conversion, finite exact accepted publication exception after gate closure. S5 independent recovery gate and explicit cohort. Hash/aggregate telemetry only. |

S4's drafts introduce `agent_log_gates` and `agent_log_admissions` in addition to the six design sidecars. Either reuse an existing compatible durable authority record, or explicitly register these two collections in schema/grants/backup/restore/migration inventory; leaving them implicit is a blocker. Freeze this choice with S1 at the contract checkpoint. The selected plan is to reuse the drafts' two records if their fencing tests pass; gate writes occur for adoption/rollover admission, not every fitting append, to avoid adding a fleet-wide write hotspot.

**Slice implementation and test acceptance**

S1: preserve Agent envelope v1 and v2; old readers must reject the dedicated v2 envelope. Preserve segment markers through every current Agent replacement. One global seq and original raw BSON/IDs; immutable seals use `sha256_raw_bson_v1`, predecessor digests and actual encoded <=1MiB manifests. Active normal 10,000 entries/64MiB; stop reserve +64/+512KiB; delete reserve +64/+512KiB; physical 10,128/65MiB. Single event16MiB/batch64 remain independent. Scalar heads only. Verify range gaps/overlaps/duplicate/extra tail/missing head/wrong Agent/seal/ordinal, integer and JavaScript safe-integer bounds, coherent restore, migration partial restart/drift, audit lease replacement/cancel/restart/unknown commit and retention guards. Quarantine one Agent; unrelated Agents remain usable. Fix the fresh near-capacity audit failure (Mongo388): stage bounded evidence batches under a fenced rebuild generation and publish only the completed captured prefix by CAS; interrupted partial generations are never trusted. Keep the canonical cache limit and test intact. Range verification is touched-history validation; full audit lag is explicit.

S2: first ship a bounded legacy target-turn reader behind the response flag, reusing prepared proofs; unsupported legacy proof falls back read-only and records reason. This interim fallback is not sufficient for segmented activation. Full S2 adds indexed output/event pages so a single huge turn never requires a lifetime or target-turn vector. Keep ordinary response shape within a documented 4MiB result budget; above it return a typed pagination-required result pointing to the existing/new documented paged replay contract, never silently omit output. Event replay must continue inside a long turn with the same durable sequence semantics. Preserve queued/unbound/status-only results, initial model vs later settings, terminal/deferred failures, no-output turns and immutable request identity. Tests compare golden legacy Responses objects and replay events, model edits, exact reconnect cursor, concurrent append, command-state change without log change, missing/quarantined proof, no-write fallback, revocation/expiry/deletion after a slow read and after buffered backpressure. Count Mongo queries/checkouts/bytes for initial and unchanged-head polls. No cached authorization.

S3: choose paginated history as the complete long-history contract, plus trusted transcript projection for the existing endpoint within its explicit budget. Page inside a long turn, not merely by turn. Preserve frozen-through cursor/global seq, steering adjacency, usage peaks/totals across generations, complete tools/approvals and hidden-event redaction. Existing unpaginated transcript remains identical within budget and returns a typed pagination instruction above it. Browser/shared view/TS/Rust SDK consume pages without blanking history or losing drafts, selected transport, uncertain-request journals or cursors. Title/suggestions use bounded policy context, approvals an indexed outstanding set, answer/feedback exact evidence. Test golden fold parity, >3 segments/mid-turn boundary, huge single turn, full page traversal/no duplicates, old client within-budget behavior, explicit over-budget behavior, corrupt/stale projections and cross-owner/visitor revocation. Fix the fresh task-status Mongo72 regression by carrying snapshot concern/time consistently through the initial aggregate, pending-command query and fallbacks; verify the actual cause in tests, not only the suspected collection default. Do not treat #1644 title discovery as the public transcript fix.

S4: every append path uses one helper in its existing transaction. The writer assessment identifies nine direct insertion files; S4 owns the helper, and S3/S5 wire their exclusively owned title/trigger/reconciliation files to it. Review all nine as one publication-coverage matrix, including creation and reuse. Adoption of an exact validated legacy snapshot installs v2/head/projections atomically; full legacy prefix is sealed without modifying original rows. Publish seal + successor + entire new batch + Agent/work/command state + projections + immutable receipt atomically. Never split a batch or precommit an orphan seal. Exact stable receipt handles unknown commit, late commit, duplicate/conflicting key and later-head advancement; absence is not abort. Tests: same-Agent concurrent append/rollover, different-Agent isolation, count/byte/reserve/batch/document boundaries, no empty seal, impossible batch no mutation, Mongo failpoints before/after commit, interrupted adoption, lease/owner/runtime changes, all writer families, mixed old/new writers with emission closed, post-drain v2 tripwire and exact accepted retry after gate close. Register each new draft before it enters a PR.

S5: inventory and replace complete scans in checkpoint capture, maintenance, reconciliation and unpublished recovery with captured prefix references, exact receipts and bounded audit/rebuild outside lifecycle transactions. No full-history materialization during a runtime lease. Recovery dry-run classifies all current capacity failures by head/lineage/failure/intent/leases/effect evidence. Tests at legacy 10,000, 10,001 and physical reserve; above-physical/corrupt refuse; same native/home recovery; ambiguous/terminal input never redispatched; stale/replaced/valid leases; parent failure/retirement/credential rotation; owner stop/delete supersedes; crash/unknown admission/runtime result; gate-close accepted completion; unauthorized callers; history and original failure evidence remain accessible. Retry pacing is only for unchanged PublishConnection parent/runtime-unavailable failures, capped5min with authoritative-progress/cleanup wake; persisted retry state survives restart. Preserve existing ledger backoff and every actual attempted-effect receipt. Its runtime delay risk must be measured.

For all slices, record fmt, clippy with warnings denied, affected Rust/domain/real-Mongo tests and security tests in the PR body. Browser/SDK owners run their fmt/lint/typecheck/tests/build on the affected surface. Use disposable Mongo and the canonical contract runner. cma-db-contention uses `.heavy-1.lock`, maximum2 Cargo jobs; other engineers use their lead-assigned lock, never both. Keep >=30GiB free, clean only owned targets/fixtures and no worktree deletion. No production payloads or identities in fixtures/artifacts.

**Complete-consumer inventory gate**

Before #1350 activation, S2 records every reachable caller from `rg 'scan_complete_log|agent_log_prefix|complete_log\('` plus non-name-based lifetime vector/fold review. Each row names a segmented bounded path and query/byte test; intentional offline audit has a one-segment budget. Current families are assigned as follows:

| Family | Owner / bounded replacement |
| --- | --- |
| Prefix/page/head, trust preparation and integrity scan | S1 range/format dispatch; once-only bounded legacy bootstrap or explicit offline audit. Legacy preparation stays #1633; segmented never re-enters legacy scanner. |
| Response snapshots and repeated polls | S2 command-bound projection/pages with current authority. |
| Transcript/usage/answer/title/suggestion/provenance/trigger input | S3 per-turn/current-state evidence and bounded context. |
| Admission/reuse/records, command reserve/reconcile/targeted controls | S4 exact command/projection evidence and original receipts. |
| Conversation/pending approval, worker publication/reconciliation, lifecycle publication | S3 owns approval read adapter; S4 owns transaction callers and immutable publication evidence. No same-file split. |
| Checkpoint capture/export, maintenance, sandbox reconciliation/unpublished recovery | S5 prefix references + exact operation evidence; background resumable rebuild. |
| Test/memory stores and all new registrations | Semantic owner implements dedicated module; S2 owns listed shared roots. Tests cannot pass only against memory while Mongo falls back to lifetime scans. |

The caller table must include all non-test hits in the current integrated tree, not just the historical files. An uncovered segmented foreground lifetime scan blocks writer activation even if HTTP canaries are fast.

**Existing-log migration, compatibility and rollback (#1327)**

1. Additive DDL first through the current migration runner, not service-startup creation or direct ad hoc Mongo writes. Record migration ID/checksum/source/image/floor, grants and readback; backup/restore original Agents/events plus every sidecar as a coherent unit. Planned IDs retain the design's `2026-11-05-am-log-segment-*` series only if not already recorded with a different checksum; do not reuse a completed migration ID for changed bytes.
2. Deploy compatible readers/consumers with writer and recovery gates closed. Run unchanged legacy reads and background writes; assert no v2/new-format emission. Certify both old-data/new-reader and actual old-binary rejection of v2 using the pinned historical executable and actual currently permitted rollback image. Source inspection or a fabricated refusal shim is insufficient.
3. Deploy the segment-aware writer binary with gate still closed. Drain old API/worker writers, including overlap replicas/background leases, and prove fleet capability floor. The v2 decode tripwire does not make a stale in-flight old writer safe. Reader floor evidence must be live before adoption.
4. Lazy per-Agent adoption on an eligible publication, bounded concurrency and isolated failure. Validate existing <=physical history once and recheck exact head/revision before atomic install. Full/reserve prefix seals as segment0; successor starts at old global head+1. Empty initialization stays active-only until the sole Created event. Keep original rows, raw BSON, receipt keys and public cursors unchanged. Reuse #1651 preparation controls where semantically compatible; its trusted metadata is not by itself a segmentation migration certificate.
5. Before activation, rollback may use the pre-format artifact only if there are zero converted Agents and zero admitted new-format operations. After any activation, rollback must use a tested segment-aware reader/writer/recovery binary. Closing a gate does not downgrade stored format, erase metadata or cancel accepted operations.
6. Closing writer admission blocks new adoption and rollover admissions. Existing segmented reads, fitting appends, cleanup and finite exact already-admitted publications continue. An earlier turn/lease is not an unlimited rollover permit. Keep compatible reconciliation available for unknown outcomes. Never fall back to an old image, delete a head/seal, reset seq or delete original history.

**Flags and control behavior**

The names below are the planned public configuration contract; default-off declarations and docs are S2-owned, implementation authority is S4-owned. Existing #1591/#1634/#1644/#1653/#1657 flags remain independent. Reader decoding of already-segmented records cannot be disabled by an optimization kill switch.

| Control | Default / operation | Kill-switch and continuation contract |
| --- | --- | --- |
| `CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED` | false; process config for independently deployable legacy response optimization | Disable restores bounded legacy compatibility behavior; on v2 always use safe segmented path, never lifetime fallback. Config changes may require a release-owner Pod rollout. |
| `CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED` | false; legacy transcript optimization and compatible paginated consumer rollout | Same format-aware behavior; old browser compatibility tested. Complete history remains pageable after activation. |
| durable `agent_log_segments_v1` gate | absent/disabled, revisioned deterministic Agent-hash basis-point cohort, explicit small canary override, max in-flight adoption2 | Release-owner CAS updates 1-Agent →10%→50%→100% without restart; tests prove disable races and persisted revision. If durable gate control is incomplete, activation stays closed rather than silently using startup env as live authority. |
| durable `agent_log_capacity_recovery_v1` gate | absent/disabled; separate explicit reviewed cohort, admission max1 initially | Gate close blocks new recovery; accepted exact recovery/cleanup/unknown settlement continues. Never inferred from the writer percentage or enabled automatically at startup. |
| scoped retry-pacing optimization | false; independent config | Disable restores previous scheduling, retaining operation/fence/attempt state. No effect on terminal-state or input authority. |

Durable gate changes are an operator action, not work this agent performs. Reuse current CLI/migration authority and authentication; add no unauthenticated private endpoint. Emit prepared/adopted/rollover/fallback/quarantined/recovery disposition counts, scan caller/avoided scans/bytes/query count/checkout wait, projection lag/audit lag, gate revision and hashed cohort membership. Do not log identities, content, credentials or native handles. Measure gate contention separately.

**Nine capacity failures and safe interim behavior**

At Nov11 16:59 SGT all nine have head10,001. Last100 intervals grew107.98–112.11 entries/hour;900 recent rows contain693 PublishConnection/runtime-unavailable,198 PublishConnection/parent-not-ready,9 final capacity failures. All11,304 observed capacity refusals were count-bound at4.52–5.46MiB; none reached64MiB. Byte warnings cannot be joined individually to Agents, so keep those aggregates separate.

S5 first emits a read-only current-state disposition for each hash, then the release owner separately authorizes only eligible domain recoveries after the writer canary. Current parents:6Failed,2Retiring,1CredentialRotating. Capacity storage repair alone does not restore those parents. Failed-parent cases keep an explicit ledger/runtime prerequisite and owner; retiring/stop/delete cases continue cleanup or remain terminal; credential rotation retains current authority until completion. Original history/commands remain readable. Do not label an unresolved externally blocked case recovered or close #1351 on code alone.

Scoped pacing can lower persistent retries after ramp-up from~110/hour to~12/hour at5min cap (estimated89% reduction), but missed wake-ups can add up to5min recovery delay. It prevents new growth; it cannot revive an already Failed Agent. Raising64MiB would not address these failures. Raising10,000 without reader/writer compatibility and recovery would increase scan cost and violate old-reader limits.

**Integration, deployment and evidence schedule**

| Deadline (SGT) | Concrete result / owner |
| --- | --- |
| Wed11-11 19:30 | This plan + exact ownership + issue milestone handoff. Train W on-call continues through the lead. |
| Wed21:00 | S1/S4 durable format, receipt, gate and projection contract freeze; S2 shared ports registered on integration-20261111; all five engineers can compile their focused modules. Missing contract is escalated with exact blocked symbol/schema and safe independent work. |
| Thu11-12 06:00 / morning train | Already approved #1653/#1657 and any earlier reviewed safe changes follow their existing train. Do not pretend new segmentation is ready for this window. |
| Thu10:00 / 14:00 merge window | S2 bounded response and S3 interim transcript if independently complete; compatible S1 foundation may land default-off when reviewed. Exact-head local gates; no failed CI jobs. Reader-floor/migration rehearsal starts on integrated source. |
| Thu18:00–23:00 | Final five slice reviews, actual Mongo/old-image/browser/SDK matrix, one-segment/audit/performance stress; review fixes. At22:00 assess complete caller inventory and recovery dependencies. If any core requirement misses, report immediately with the largest safe default-off deliverable; never call partial readers full segmentation. |
| Fri11-13 00:00–06:00 | Integration fixes, exact-head reviewer verdicts and lead merge relays; all source merges by06:00. Watch develop push CI, fix forward/revert real failures within1h. Exact green batch artifact is mandatory for release. |
| Fri morning/afternoon | Release owner deploys compatible closed-gate binary, verifies reader/writer floor and legacy baseline, drains old writers; one-Agent natural-use canary then10% if gates pass. No test input is authorized by this plan. |
| Sat11-14 train/watch | 50% then100% after observations and receipt/audit reconciliation; separately admitted eligible capacity recovery one at a time. Use weekend fix-forward train only from green exact artifact. |
| Sun11-15 | Stable full-cohort watch, final route/checkout/scan/availability evidence and dispositions, reviewer/lead closure checks. Target >=24h at100% by Sunday evening; final stability window restarts after a material corrective deployment. |

Activation observations at every stage: capture exact image/source/migration checksum, gate revision, API/worker reader-floor inventory and cohort denominators. Compare matched route mix before/after W/fleet100 and after segmentation, never credit segmentation for already-shipped pool30/admission changes. For response GET/events/POST initial observation and transcript/snapshot/head, use30min minimum, >=100 completed requests per principal route/status cohort, extend up to2h if low volume; report small-N without passing it. Separate HTTP headers from end-to-end conversation/desktop readiness and censored client timeouts. Reuse `2026-11-11-1320-measurement-plan.md` exact acceptance criteria for #1320 closure.

Local scale matrix:1/10/100/1,000 sealed segments; pages20/100/200; near65MiB active; many tiny segments; a single turn across>=3segments; count/byte rollover; empty/nonempty current turn. Capture Mongo commands/checkouts, bytes/reply, elapsed p50/p95/max, allocations/peak RSS, active-validation count and touched seals. Hold requested-page/active size constant when varying lifetime. Query/byte/memory bounds must follow page+active work, not total lifetime; changed output size is a separate variable. Segment fixture construction is disposable local work, not production input.

Proposed production advancement thresholds (operational guardrails, not invented issue acceptance): zero new integrity/quarantine/schema/sequence/duplicate-dispatch defects; no authority bypass; zero foreground lifetime scans for successfully activated Agents; complete receipts and frozen-page parity; stable bounded audit/projection lag with no monotonic backlog over2windows; p95 <=15s existing authority budget and no >20% regression versus matched pre-stage route baseline for two5min windows; checkout p95/scans/bytes per request must improve or be explained by preparation; 5xx/timeout rate must not increase by>0.5percentage points for two5min windows at adequate N. A missing route sample or missing actual rollover cannot pass by elapsed time.

Canary minimum is36min (two historical18min lease periods) **and one real natural rollover**. No natural rollover means writer production acceptance is still pending, even if reads look fast; do not generate user input to force it. Rollback triggers: any correctness/authority/duplicate-input/unknown-outcome misclassification immediately closes new writer/recovery admission; sustained performance threshold breach stops cohort advancement and invokes release-owner compatible rollback. Preserve evidence, drain uncertainty through exact receipts, keep accepted work/readability. Restart observation only after the cause is resolved.

**Critical-path risks and deliverable limits**

- Reader semantic conflicts, two fresh reader/consumer Mongo failures, uncompiled writer drafts and the actual #1327 rollback matrix are the largest risks. Current evidence does not justify promising full production certification today. Source target Fri06 is an aggressive estimate with18–28h per storage/consumer engineer and serialized build locks; a delayed contract or >one major integration rewrite consumes the margin.
- S2/S3 improve user-visible reads before full rollout, but their interim legacy fallback cannot authorize #1350 activation. All remaining foreground consumers and all writer families must pass on segmented fixtures first.
- Live parent/ledger recovery is an external dependency for some of the nine failures. We own correct capacity recovery/dispositions; parent readiness remains with the assigned availability owners. Full sprint success requires those prerequisites too, not a wider Failed→Ready transition.
- Review-ready by Thu23 leaves7h for fixes/merge. If the Fri06 source gate is missed, the largest safe deliverable is deployed compatible readers + independently verified bounded response/transcript improvements with writer/recovery closed. Report it as incomplete segmentation and use approved weekend trains for remaining fixes; the owner must see the missed stabilization risk immediately.
- Sun stabilization requires real eligible traffic and rollover, sufficient route samples and a certified rollback artifact. No fabricated acceptance, forced input or data deletion fills an evidence gap.

**Issue/PR handoff and cleanup**

Update Project9 In Progress and an aggregate-only comment at each opened/pushed/merged/deployed/verified/blocked milestone; Done only when closed after production evidence. Milestone comments name next action/date; no identities/content or prohibited trailers. S1→#1348, S2/S3 plus transactional/recovery consumer work→#1349, S4→#1350, S5→#1351; post #1320 checkout/route evidence separately. #1343 retains its Train W `/api/v2` discovery/edge-health verification handoff; obsolete-domain cases are excluded.

This planning step publishes the plan and ownership as a documentation-only commit on the new integration-20261111 branch from current develop, plus authorized issue/status handoffs. The previous bounded-close-retries branch at `85bb5c2370068fde06884a09b50df7e46b5abc3a` is preserved. No production mutation, build, target directory, new worktree or implementation PR is part of this plan milestone. Notes/ownership/evidence are0600; retain requested artifacts, clean temporary validation files and only owned build outputs after each subsequent slice.

**Frozen S1/S4 recovery signatures — Wed18:43 amendment**

The normative compile contract is [S1 captured source and S4 atomic publication](2026-11-11-log-segments-s1-s4-contract.md). It fixes exact Rust signatures for `capture::{capture,points,CapturedLog}` and `writer_commit::{prepare,persist,PublicationInput,AggregateUpdate}`, including storage-only recovery, private proof construction, same-session use, generation absence proof and fail-closed temporary bodies. S1/S4 publish their own mergeable signature commits by19:10; S5 may code against these signatures immediately. S2 registers their concrete source checkpoints. No stub is a review-ready or activation claim. This amendment takes precedence over earlier prose signature names; `PublicationReceipt` is the existing `publication::Publication`.


Frozen 2026-11-11 18:43 SGT by the segmentation integration owner, following the
S5 interface handoff. This is a compile contract, not an executable proof claim.
S1 owns `agent_log_segments/capture.rs` and its focused children/tests; S4 owns
`writer_bootstrap.rs`, `writer_commit.rs` and their focused children/tests.
S2 owns shared root registration. These paths are added to the exclusive manifest.

Use the existing `TransactionLogError`, `AppendCapacity`, `Agent`, `Head`,
`AgentLogProjectionState`, `PublicationIdentity`, `Admission` and `Publication`.
Do not introduce another receipt schema or a second transaction framework.
`Publication` below is the existing type in `publication.rs`; the earlier
prose name `PublicationReceipt` means that type, not a new wrapper.
All signatures below are `pub(in crate::store)` unless an accessor is shown.
Opaque proof structs have private fields and no public/default/deserialization
constructor. Private implementation fields may evolve without changing callers.

## S1: captured source, owned by cma-cost

```rust
// module: crate::store::agent_log_segments::capture
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogProjectionState};
use mongodb::{bson::Binary, ClientSession};
use super::Head;
use crate::store::{
    agent_log_integrity::TransactionLogError,
    sandbox_mongo::MongoSandboxStore,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::store) enum CaptureMode {
    Snapshot,
    Transaction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::store) enum CapturedFormat {
    Legacy,
    Segmented,
}

pub(in crate::store) struct CapturedLog { /* private verified source */ }

pub(in crate::store) async fn capture(
    store: &MongoSandboxStore,
    current: &Agent,
    session: &mut ClientSession,
    mode: CaptureMode,
) -> Result<CapturedLog, TransactionLogError>;

impl CapturedLog {
    pub fn agent(&self) -> &Agent;
    pub fn format(&self) -> CapturedFormat;
    pub fn through(&self) -> u64;
    pub fn incarnation(&self) -> &str;
    pub fn latest_entry(&self) -> Option<&AgentLogEntry>;
    pub fn projection(&self) -> &AgentLogProjectionState;
    pub fn max_opened_generation(&self) -> u64;
    pub fn max_retired_generation(&self) -> u64;

    // Exact bounded active-source proof for the writer. Legacy uses a
    // validated synthetic ordinal-zero Head; it is not persisted by capture.
    pub fn active_head(&self) -> &Head;
    pub fn active_leaf_digests(&self) -> &Binary;
}

pub(in crate::store) async fn points(
    store: &MongoSandboxStore,
    captured: &CapturedLog,
    sequences: &[u64],
    session: &mut ClientSession,
    mode: CaptureMode,
) -> Result<Vec<AgentLogEntry>, TransactionLogError>;
```

`capture` is read-only. It reads the current aggregate in the supplied session,
requires exact equality with the caller's Agent, validates creation/incarnation,
quarantine, format/head and the complete certified semantic/projection coverage.
Snapshot means an already-created read-only snapshot session; Transaction means
the caller's existing snapshot/majority transaction. Neither method starts,
commits, aborts, audits, repairs or writes quarantine. The proof records its
session/source context and is not valid in another session or after a changed
capture. Use in another mode/session fails before publication.

Legacy capture may use the existing bounded physical complete validator, up to
10,128 rows/65 MiB; it is not a new fast-path claim. It folds validated source once
and derives the exact raw-BSON leaf digests. Segmented capture uses the verified
Head, at most one active-unit validation, exact projection generation and bounded
indexed facts. It must never visit sealed lifetime history or silently run a
legacy scanner. Missing/unsupported/ahead/inconsistent semantic coverage is an
error, not an empty fact. Until that path exists a stub returns Unavailable.

`latest_entry` is exactly `through`, or None only for a proved empty source.
`projection` is certified through the same captured head, not a caller assertion.
Generation maxima cover every opened and retired native generation through the
capture; zero proves no event of that class. They are independent of the current
native generation and provider allocation generation. S4 adds these scalar maxima
to the semantic fold/envelope; S1 validates their coverage, S5 consumes them.
Absence from a point lookup alone never establishes an unpublished generation.

`points` accepts 1..=32 unique ascending positive sequences <= captured through,
returns exactly one validated original entry per requested sequence in that order,
and enforces 4 MiB total returned BSON. Missing rows, budget overflow, altered
leaves, stale context and wrong lineage fail explicitly. It reuses capture's
active proof and checks only touched sealed membership, never rescans lifetime.
An oversize requested proof is a typed unavailable/corrupt outcome as appropriate,
not permission to replay input or infer no prior work.

S5 mapping: checkpoint capture/publication use through + latest_entry; maintenance
uses latest_entry; runtime reconciliation uses projection; unpublished recovery
uses the two certified maxima; capacity admission uses exact failure/source points
plus the existing original open receipt and parent/home/work fences. The source
proof grants no user, runtime, recovery or gate authority.

## S4: atomic preparation and persistence, owned by cma-seg-lead

```rust
// module: crate::store::agent_log_segments::writer_bootstrap
pub(in crate::store) struct BootstrapPlan { /* private validated staged generation */ }

// module: crate::store::agent_log_segments::writer_commit
use layer2_core::sandbox::{Agent, AgentLogEntry};
use mongodb::{bson::{Binary, Document}, ClientSession};
use super::{
    capture::CapturedLog,
    publication::{Publication, PublicationIdentity},
    writer_admission::Admission,
    writer_bootstrap::BootstrapPlan,
};
use crate::store::{
    agent_log_integrity::{AppendCapacity, TransactionLogError},
    sandbox_mongo::MongoSandboxStore,
};

pub(in crate::store) struct PublicationInput<'a> {
    pub operation: &'a PublicationIdentity,
    pub current: &'a Agent,
    pub next: &'a Agent,
    pub logs: &'a [AgentLogEntry],
    pub capacity: AppendCapacity,
    pub result_digest: Binary,
}

pub(in crate::store) struct AggregateUpdate {
    pub filter: Document,
    pub fields: Document,
}

pub(in crate::store) struct PreparedPublication { /* private validated plan */ }

pub(in crate::store) async fn prepare(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    captured: &CapturedLog,
    input: PublicationInput<'_>,
    bootstrap: Option<&BootstrapPlan>,
    admission: Option<&Admission>,
) -> Result<PreparedPublication, TransactionLogError>;

pub(in crate::store) async fn persist(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    prepared: PreparedPublication,
    aggregate: AggregateUpdate,
) -> Result<Publication, TransactionLogError>;
```

Both calls use the caller's existing snapshot/majority transaction. `prepare`
validates the captured binding, stable key/fingerprint/time, <=64-event batch,
capacity, projection/bootstrap coverage and exact admission/gate proof. It has
no durable side effects. `PreparedPublication` binds the session, current/next
Agent, pre-encoded rows and proposed result; it cannot be constructed by callers.
`persist` owns the single Agent CAS and adds current revision/head/incarnation/
format conditions to the caller's domain/lease predicate. It writes original
rows, optional seal, successor/current Head, projection/evidence and immutable
Publication atomically with the caller's related command/work/checkpoint writes.
It neither starts nor commits nor retries the transaction. Any error requires
aborting the whole caller transaction; no catch-and-commit or ignored CAS miss.

Storage-only recovery is `logs: &[]`, original capacity-failure request identity,
`AdmissionKind::CapacityRecovery`, both independent gate proofs and a validated
legacy BootstrapPlan. The normal empty-batch exception does not grant recovery.
It seals the exact original prefix, starts successor H+1, keeps global H and
all events unchanged, and atomically records S5's non-ready durable recovery work
and exact publication. S5 writes its recovery carrier in the same session before
`persist`; a carrier failure aborts all effects. No use of ordinary first-message
work as a recovery carrier. Parent, native/home and work fences remain S5-owned.
The staged generation is built before this transaction in bounded fenced batches;
only final exact CAS makes it visible. Captured legacy proof alone is not bootstrap.

A completed Publication is storage evidence, not permission to send input or mark
Ready. Unknown commit is reconciled through S4's existing receipt reader under a
majority barrier before any new attempt. Missing receipt means Unknown; it never
means KnownAbort. A later head does not invalidate an exact completed receipt.
No implicit new key/time after restart, native resend or checkpoint acknowledgement.

## Signature-commit delivery and allocation

S1 and S4 each publish a small normal-push signature checkpoint on their OWN
assigned topic branch, then report the exact SHA on #1348/#1350 and #1351.
S1 registers `capture` in its own segment module; S4 supplies the owned writer
files and sends the child declarations to S1. If an implementation is pending,
its temporary body returns `TransactionLogError::Domain(SandboxStoreError::Unavailable(...))`
before any write. Do not use `todo!`, panic, fabricated success, `Ok(None)` fallback,
unsafe data or disabled tests. Such a checkpoint is not review-ready and cannot
activate a gate. Keep the signatures stable while replacing refusal bodies.

S5 codes against these frozen signatures now, then normally merges the exact
signature dependency heads. S2 supplies any root registrations on pinned dependency
commits without importing unrelated implementation slices. Changes to signatures
require an integration-owner amendment before callers diverge. Shared contract
checks are source formatting and compile-link tests; executable success/failure,
Mongo atomicity, no-replay, bounded work and rollback tests remain slice gates.

S5 receives the four previously unallocated pacing paths in its handoff. Its
independent flag is `CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED=false`, consumed
only by the new parent-not-ready observation path. S2 owns config/store/worker
composition; S5 implements a builder accepting that bool in its assigned code.
Existing runtime-unavailable pacing stays as configured. No ledger behavior change.

Next checkpoint: owner signature commits before Wed 2026-11-11 19:10 SGT;
shared executable contract freeze still Wed21:00. Develop merge freeze remains
binding, and no production change is part of this handoff.

## Signature delivery receipt

Both owner signature commits are pushed and remote-readback verified:

| Owner | Branch | Exact signature commit |
| --- | --- | --- |
| S1 | `tl/cma-log-segments/reader-foundation` | `5ed7eaae1cc1cb357a1649a7773a24956adf2518` |
| S4 | `tl/cma-log-segments/writer-publication` | `453b417426e015459d24cc4e95d9f13586949a03` |

The Rust signatures match this freeze. Pending bodies return typed Unavailable
without writes; proof/plan types have no public constructor. S5 may normally
merge the exact dependencies. They do not establish executable success/atomicity.
S2 root/schema dependency `ec6002898d2329e744ea3a26513f500941461593` is published
on `tl/cma-log-segments/reader-registrations`. Shared projection/root declarations
and S3's supporting fold exports are the remaining compile integration, followed
by actual source/persistence implementations and their local/Mongo gates. The
common integration lane combines these checkpoints; it is not a deployable PR.

## Dependency amendment, Wed 2026-11-11 19:30 SGT

S2 has taken the four small shared dependencies to unblock S1: fold serialization/identity exports, store sidecar handles, opened/retired generation maxima and exclusive transaction scope. Continuing ownership returns to S3 (fold), S1 (capture/collection registry), S4 (projection/publication) and S5 (recovery caller) after merging `tl/cma-log-segments/dependency-contracts`. No parallel replacement implementation is needed.

The exact frozen signatures and ownership are in `2026-11-11-segments-dependency-contract.md` beside this plan and `docs/notes/2026-11-11-segments-dependency-contract.md` on the dependency branch. Transaction callers now use `TransactionScope::start(&mut session)` then `capture_transaction(store,current,&mut scope)` and `points_transaction(store,captured,sequences,&mut scope)`. S4 `prepare` and `persist` take `&mut TransactionScope<'_>` instead of raw session in parameter2. `scope.release()` consumes the scope before returning the session for caller commit/abort. Only audited segment internals can access its raw session; no lifecycle calls are allowed there. Private stamps differ across transactions even on the same session. Old raw transaction capture remains closed. New capture/publication bodies remain typed refusals until their owners implement certification; this signature checkpoint is not a green combined candidate.

Required generation fields are `AgentLogProjectionState::{max_opened_generation,max_retired_generation}`; they cannot default on document decode. The store handle is `MongoSandboxStore.segment_logs` with heads/seals/publications/projections/evidence/audits using the existing six collections, no DDL. S3's requested derives and `validate_projection_identity(agent,entries)` are supplied in this checkpoint. Heavy gates await disk headroom and heavy-1; no owned target or disposable Mongo remains at handoff.

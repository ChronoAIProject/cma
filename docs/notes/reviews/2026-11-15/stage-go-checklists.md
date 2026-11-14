# S1 — 15 Nov 2026, 06:00 SGT — AM fixed two

Checklist owner **solution-architect-03**; executor **deployment-engineer-01**; manifest reviewer **developer-16**; GO/advancement **lead**. Issue [#2016](https://github.com/ChronoAIProject/cma/issues/2016). Frozen preparation is not execution approval. Blank evidence = **HOLD**. Use the [command contract and open bindings](stage-command-review.md) and [frozen sources](stage-review-sources.md); no CI wait.

**Pre-checks — initial every box with source, results and actual observations before stopping legacy AM**

- [ ] K01/K15: exact integrated source and AM/EG/runtime digests; [revised M3(a)–(d) plus architect04 source GO](stage-command-review.md#revised-m3-merge-gate), with each Mongo suite result explicit; M4 Batch9 N1/OFF 30-minute soak PASS. Bind schema/roles and retained receiver image/protocol/challenge duration. Missing local real-runtime/auth/provider adapter is not a merge blocker.
- [ ] K03–K08/K13: log-engineer-01's read-only queries cover the actual release, both replicas, matched baseline and30-minute S1 interval, with logical command/effect identity. Bind its immediate failure-notification path to deployment-engineer-01 and approved compatible rollback; never defer failure notification to E+15. Accepted work comes from **real production traffic**. No synthetic WRITE executor, broker creates/sends or NyxID slot change. Local host/component scope remains explicit.
- [ ] K14: reviewed placement constraints, eligible distinct hosts, local placement proof, survivor-load and transition capacity qualified; refresh actual AM memory headroom (3273.7MiB observed versus256MiB requested). Count all active/surge/terminating clients with ≥30% per-server Mongo headroom. Direct relay uses Pod IP:8080. EG stays one; neither service has an HPA or competing release writer.
- [ ] K16–K21: matched final-N1 evidence and frozen A3 classes; Activity postcheck/configuration bound. Keep statistical and policy verdicts separate. The later K21 CRITICAL exception requires valid matched observations, assessed no adverse signal and passing absolute checks; see B6.
- [ ] K22/K24: approved route, selected controller/source and exact operations, current per-step grants, explicit outage approval, controller-order proof, real backend/NEG health access, pre-pulled images, and recovery for **both** failed cutover and healthy 2→1. No production writes 02:00–02:59. Old unbound A2-r5 plans are not executable.

**GO sequence — per-operation prepare/review/execute/readback; no write loop**

`s1-pause → s1-prepare → s1-stop → s1-unpause → s1-start → s1-pdb`. Keep canonical selectors/routes. Prepare ON template while paused1; stop old1→0; prove all legacy effect/socket authority stopped or safely revoked/expired; unpause at0 and verify controller-owned candidate RS0; only then start0→2. RS0-after-stop requires the explicit lead-approved controller variant. Verify each pod serves, both backends healthy and actual pair on distinct hosts, then PDB `minAvailable:1`, `AlwaysAllow`, exact AM selector. A PDB does not prevent scale-to-zero or Deployment deletion.

**No-overlap clock:** last healthy old backend withdrawal → first successful user service on a qualified new backend **<120 seconds**; record both-ready separately. At120 seconds abort promotion even if a long kubectl wait remains. Never overlap legacy/owned writers or shorten leases/grace to meet time.

**30-minute soak; low-traffic extension up to30 more — K02/K03–K08/K13–K24**

- [ ] Start only after two Ready/Available, real backends/membership stable and handoffs/terminations complete. Record initial E=start+30min; report by actual E+15. If accepted production write commands N<20 at30min, extend the same observation by up to30 more minutes. If still N<20 after extension, **accept using passing required in-code multi-instance suites plus zero errors**, disclose actual low N and do not hold S1 for low traffic alone. Count distinct accepted commands, not retries/reads; retain first-window N/errors and cumulative extended N/errors without summing overlapping scopes. Preserve actual initial/extended bounds; image/config/count changes restart stability. Sample each minute and cover≥2 background cycles.
- [ ] From **real production traffic**, log-engineer-01 reports per-replica create/send/stop/stream counts, success and latency; ownership/epoch transitions; fencing refusals; ledger duplicate effects (**same logical command executed twice =0**); and5xx/429 by cause versus baseline. Retain completed/failed/unresolved denominators, coverage and actual release/config. Observe natural handoffs without injecting work or faults. Same-work recovery≤30s and K20/K21/absolute targets apply where observed; unobserved boundaries stay NOT_RUN/INCONCLUSIVE even if the permitted low-N stage acceptance applies.
- [ ] Prod-verifier-02 alone runs the [v3 READ sampler](stage-command-review.md#s1-read-sampler): detail/transcript and usage24h/7d/30d;120slots/15s/exactly1800s, at most8GETs/round,960scheduled plus bounded discovery. Target100 completed per boundary is not guaranteed; report actual N/range/failures. One existing lock/lane, no retry/polling/refresh/prewarming; NyxID slot unchanged. Real-traffic extension never extends/restarts synthetic reads. HTTP body timing remains separate from usable open/render.

**Abort/recovery:** observed accepted-work loss, duplicate/stale effect, auth/dual-desktop breach, unresolved terminal/effect failure, failed in-contract journey or absolute completion target ⇒ immediately stop promotion, halt owned synthetic reads and invoke approved compatible rollback. Do not wait for performance windows or the low-N extension. Restrict real admissions only through approved recovery controls. Other safety/availability/capacity triggers still apply; missing telemetry or unresolved outcomes ⇒ HOLD; low traffic alone follows the explicit exception. Healthy `s1-one` keeps modeON/image/epochs/PDB; wait drain and survivor service. If unsafe, use the approved failure branch. Never blindly resend Unknown, use generic undo or bypass with modeOFF.

**Decision:** GO/HOLD/ABORT ___; executor ___; reviewer ___; lead ___; release/results ___; initial/extended soak ___→___; accepted N ___; low-N acceptance/suite results ___; unobserved rows ___ . Only a signed S1 PASS permits S2.

<div style="page-break-after: always;"></div>

# S2 — 15 Nov 2026, 08:30 SGT — EG fixed two

Checklist owner **solution-architect-03**; executor **deployment-engineer-01**; manifest reviewer **developer-16**; GO/advancement **lead**. Frozen preparation is not execution approval. Blank evidence = **HOLD**. Use the [command contract](stage-command-review.md) and [sources](stage-review-sources.md); no CI wait.

**Pre-checks — initial every box with source, results and actual observations before stopping legacy EG**

- [ ] Signed S1 stage PASS with actual real-traffic results and any explicit low-N acceptance; healthy AM2/PDB stays fixed two. K01/K09–K15: retain exact-source M3 EG suite/fixture scope; verify enabled ingress/console/provider paths, schema/record floors, bot/partition ownership, receipt recovery, fairness and same-work≤30s during the authorized stage. Neither service has an HPA/concurrent rollout.
- [ ] K13: corrected EG template and secure early owned-drain mechanism have source/operation PASS and real failure/drain evidence. **d389's EG shell exec hook is withdrawn.** #1996 native `sleep:30`/grace760 alone does not qualify early drain. Use guarded field patches preserving live env/envFrom; full historical EG apply adds absent owner-key settings. No old exec patch or invented `--replica-drain` use.
- [ ] K04/K07/K10/K11: shared console auth/logout, dedupe, global Identify/reconnect/provider budgets; original idempotency and uncertain-outcome custody survive handoff. Approved canaries cover every enabled provider; monitor/reconciler and 24-hour post-go-live on-call named before GO.
- [ ] K14/K16–K21: survivor/transition capacity including AM2, all clients/terminations and ≥30% Mongo headroom; reviewed placement constraints, eligible distinct hosts and local proof. Matched baseline, A3 classes and separate statistical/policy verdicts bound; apply only the narrow K21 CRITICAL exception in B6. Pod-IP EG origin:8090 matches final binary.
- [ ] K22/K24: selected controller/release/operations/route, per-step/outage/rollback approvals, actual controller/NEG proof and <120-second restoration evidence. Pre-pull is not application readiness. Both failed-cutover and healthy 2→1 recovery branches are bound. No production writes 02:00–02:59.

**GO sequence — separate guarded operation and readback at each checkpoint**

`s2-pause → s2-prepare → s2-stop → s2-unpause → s2-start → s2-pdb`. Pause legacy1, prepare modeON inactive template; old1→0; prove old bot sockets/partition writers closed or safely fenced; unpause0; validate candidate RS0; start0→2. The RS0-after-stop controller variant needs lead approval. Verify each pod and canonical backend, placement and ownership distribution before installing PDB `minAvailable:1`, `AlwaysAllow`, exact EG selector. Preserve AM2/PDB and all canonical routing.

**No-overlap clock:** last healthy old backend withdrawal → successful qualified new user service **<120 seconds**; record both-ready separately. At120 seconds abort promotion and invoke approved compatible recovery. Planned zero backends are allowed only inside this authorized interval; never count readiness alone as service restoration.

**30-minute soak — K02–K05/K07/K09–K11/K13–K24**

- [ ] Start after intended Ready/Available/backends/membership and handoff stabilize; prior terminations complete. Declare E=start+30min and report by E+15; restart on image/config/count change. Per-minute ownership, custody/terminal, reconnect, queue/fairness, provider throttles, CPU/memory, Mongo/headroom and backend observations; ≥2 five-minute background cycles.
- [ ] Every accepted provider canary terminal or authoritatively reconciled; zero unresolved Unknown/Partial, no duplicate/lost/truncated replies. Reconciled failure remains failure. Preserve K10 queue p95 ≤2s/max ≤10s and ≥99.9% receipt/reply, K20 loaded bounds and all K16–K21 client/route targets with separate statistical/policy results. Confirm AM has not regressed. Seal 24-hour monitor start/end, owner, scope and escalation.

**Abort/recovery:** wrong/stale/dual owner, duplicate effect, lost custody/receipt, auth bypass ⇒ immediate stop. Unknown/Partial ⇒ HOLD/page; unresolved at cohort deadline ⇒ compatible recovery, never blind resend. Apply S1's availability/capacity/performance abort rules. Healthy `s2-one` keeps EG modeON/fences/PDB and AM2; wait survivor reconnect and reconciliation. EG image recovery is forward-fix-only unless an exact older digest has explicit retained-record compatibility approval. No default pre-stage image undo; failed-start/unhealthy-owner branch must be separately bound.

**Decision:** GO/HOLD/ABORT ___; executor ___; reviewer ___; lead ___; release/receipts ___; soak ___→___; provider reconciliation/monitor ___ . Only signed S2 PASS permits S3.

<div style="page-break-after: always;"></div>

# S3 — 15 Nov 2026, 10:30 SGT — both HPAs, min2/max5

Checklist owner **solution-architect-03**; executor **deployment-engineer-01**; load/scale evidence **developer-17**; manifest reviewer **developer-16**; GO **lead**. Frozen preparation is not activation. Use the [command contract](stage-command-review.md); missing evidence = HOLD. No CI wait.

**Pre-checks — initial every box before creating either HPA**

- [ ] Signed S1/S2 PASS; both fixed two, healthy canonical backends, PDBs `minAvailable:1`/`AlwaysAllow`, zero terminating; unchanged qualified images/modeON/fences/record floors. Capture narrow pre-S3 specs. No rollout overlaps HPA operation; disable/remove HPA and wait all terminations before any later rollout.
- [ ] K12–K15: revised M3 GO, S1/S2 accepted-work evidence, secure drain and survivor recovery≤30s. Qualified handoff p95≤90s and **separate hard lifetime G** exist; missing/incompatible timing means HOLD. Both templates and actual Pods have grace760s including preStop; S1/S2 must replace live Batch8 grace735s. S3 automatic work remains mandatory.
- [ ] K14: qualify combined active+surge+terminating processes, all consumers, node loss/provider limits and ≥30% Mongo headroom against the selected64/128 CPU ceilings and actual nodes/memory. Illustrative G760/P120 gives12/service: it fits raised quota but exceeds47.67 eligible-node CPU. G remains unqualified; raising quota supplies no node or socket capacity.
- [ ] K21/K25: fresh v2 resource metrics, useful-load response, CPU idle/knee qualification and matched N1/N2/scaling evidence. AM60% of4CPU=2.4 cores; EG200% of250m=500m. CPU-only that cannot add useful capacity ⇒ HOLD for reviewed metric design. No assumed custom metrics API.
- [ ] Exact release/HPA payload and controller commit, route/per-operation grants, owned-load scope, freeze/delete/descent recovery and observation window bound. No production writes 02:00–02:59. Schedule leaves time for stabilization, full termination, final soak and M8; deadline cannot shorten them.

**GO sequence — granted quota pre-step, then AM HPA and EG HPA**

`s3-quota-raise → s3-am-create → s3-eg-create`. Before either HPA, use the exact S3 grant to change only `chrono-bot/sandbox-quota` CPU ceilings40/80→64/128. Save complete before-spec and confirmed result; require64/128 in both spec/status. Preserve memory/Pod limits. Each HPA targets its canonical Deployment; verify min2/max5, fresh metrics/generation and scaling conditions. Unknown writes require reconciliation.

| Direction | Required policy |
| --- | --- |
| Up | stabilization0; Max of +2 pods/60s and +100%/60s |
| Down | stabilization300s; Min, −1 Pod/120s fixed; handoff p95>90s or missing/censored timing ⇒ HOLD, never change cadence silently |

Prove useful **automatic** up/down for each service under approved load within capacity. Claim exercised5 only if observed5; configured ceiling capacity remains K14, useful automatic up/down remains K25. Manual replica patches do not satisfy K25. Remove only owned load, observe full down stabilization/cadence, handoff and actual exits to2. Record transitions, failures and peak clients; a PDB does not cap HPA terminations.

**30-minute final soak — K02–K21/K24/K25**

- [ ] Start after both return stable2, zero terminating, healthy backends/membership; changes restart it. Declare E=start+30min; report by E+15. Keep ramp/downscale strata separate. Each minute: metrics freshness/recommendations, useful throughput, owners/custody/streams/desktop/provider outcomes, process/client bounds, CPU/OOM, checkout/headroom and ≥2 background cycles. No oscillation or missing-metric downscale.
- [ ] All K16–K21 absolute/A3 acceptance checks pass at N2 and scaling; no unobserved route PASS. Report statistical/policy verdicts separately; only valid matched CRITICAL observations meeting B6 may receive the exception. Bind logs plus NyxID/client evidence for M8 13:00–17:00 and final18:00 report. Provider 24-hour monitoring continues beyond that deadline.

**Abort/recovery:** any S1/S2 safety, availability, capacity or supported performance trigger ⇒ stop promotion; halt only owned load. Generate/readback **both** HPA directions Disabled, then UID/resourceVersion-bound delete of only S3-created HPAs. Account for in-flight controller writes. With HPA absent, healthy survivors and zero prior terminations, descend **one at a time** to2, retaining PDBs/modeON/compatible images/custody. If demand requires >2, hold that safe fixed count and report autoscaling not delivered. Run fresh30-minute recovery soak; no image/mode downgrade or bulk5→2.

Then `s3-quota-restore` needs its own grant and confirmed raise result: fixed2, no HPA/terminations, reconciled quota status and used CPU≤40/80. Otherwise retain raised ceilings during recovery. Preserve unrelated fields; no automatic restore after successful S3.

**Decision:** GO/HOLD/ABORT ___; executor ___; reviewer ___; lead ___; release/HPA/receipts ___; ramp/down ___; soak ___→___; M8 ledger ___ . Fixed replicas alone are not S3 PASS.

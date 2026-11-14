# S1 — 15 Nov 2026, 06:00 SGT — AM fixed two

Checklist owner **solution-architect-03**; executor **deployment-engineer-01**; manifest reviewer **developer-16**; GO/advancement **lead**. Issue [#2016](https://github.com/ChronoAIProject/cma/issues/2016). Frozen preparation is not execution approval. Blank evidence = **HOLD**. Use the [command contract and open bindings](stage-command-review.md) and [frozen sources](stage-review-sources.md); no CI wait.

**Pre-checks — initial every box with receipt/hash before stopping legacy AM**

- [ ] K01/K15: exact integrated source and local-build AM/EG/runtime digests; M3 essentials PASS at N1/N2/2→4→2/kill; M4 batch 9 N1/OFF 30-minute soak PASS. Additive schema/roles and every traffic-capable retained receiver's image/protocol/exact challenge duration match; never infer compatibility from the default runtime image.
- [ ] K03–K08/K13: cross-ingress, authorization/quotas, desktop exclusion, fenced ordinary work ≤15-second leases/≤3-second renewal, lost-ACK custody and same-work ≤30-second owner-loss recovery qualified locally. Two background cycles covered. No unresolved implementing-source FAIL.
- [ ] K14: reviewed placement constraints, eligible distinct hosts, local placement proof, survivor-load and transition capacity qualified; all active/surge/terminating clients counted with ≥30% per-server Mongo headroom. Direct relay uses Pod IP:8080. EG stays one; neither service has an HPA or competing release writer.
- [ ] K16–K21: three valid matched final-N1 baseline windows and frozen A3 classes; Activity postcheck/configuration bound. Bind an approved confidence method and post windows: current passive method needs ≥3 baseline AND ≥3 post windows. One soak alone is INCONCLUSIVE; see B6.
- [ ] K22/K24: approved route, exact corrected operation hashes, current per-step grants, explicit outage approval, controller-order proof, real backend/NEG health access, pre-pulled images, and recovery for **both** failed cutover and healthy 2→1. No production writes 02:00–02:59. Old unbound A2-r5 plans are not executable.

**GO sequence — per-operation prepare/review/execute/readback; no write loop**

`S1-pause → S1-prepare-template → S1-stop-legacy → S1-unpause-zero → S1-start-two → S1-pdb`. Keep canonical selectors/routes. Prepare ON template while paused1; stop old1→0; prove all legacy effect/socket authority stopped or safely revoked/expired; unpause at0 and verify controller-owned candidate RS0; only then start0→2. RS0-after-stop requires the explicit lead-approved controller variant. Verify each pod serves, both backends healthy and actual pair on distinct hosts, then PDB `minAvailable:1`, `AlwaysAllow`, exact AM selector. A PDB does not prevent scale-to-zero or Deployment deletion.

**No-overlap clock:** last healthy old backend withdrawal → first successful user service on a qualified new backend **<120 seconds**; record both-ready separately. At120 seconds abort promotion even if a long kubectl wait remains. Never overlap legacy/owned writers or shorten leases/grace to meet time.

**30-minute soak — K02/K03–K08/K13–K24**

- [ ] Start only after two Ready/Available, real backends/membership stable and handoffs/terminations complete. Record start/end; any image/config/count change restarts the clock. Sample aggregates each minute; use approved finer capture for ≤30-second recovery, and observe ≥2 five-minute background cycles.
- [ ] Per-route/client evidence: plain/desktop create ≤5s; open/send/interrupt ≤1s; usage ≤2s; Activity API p95 <1s, separate page completion. K20 load bounds and K21 A3/confidence rules hold. Account accepted = terminal + reconciled + unresolved; no custody loss, duplicate/stale commit or hidden failures.

**Abort/recovery:** immediate safety violation, zero backend outside the approved cutover, service-loss recovery >30s, OOM/permanent renew failure or capacity breach ⇒ stop promotion/admission via approved controls. Performance-only failure in two consecutive five-minute windows or whole-soak comparison ⇒ compatible recovery; missing evidence ⇒ HOLD. Healthy `S1-rollback-one` keeps modeON, compatible image, epochs and PDB; wait actual drain and survivor service. Failed-start/unhealthy-owner recovery needs its separately bound branch. No generic undo or modeOFF/reset shortcut.

**Decision:** GO/HOLD/ABORT ___; executor ___; reviewer ___; lead ___; release/receipts ___; soak ___→___; open rows ___ . Only a signed S1 PASS permits S2.

<div style="page-break-after: always;"></div>

# S2 — 15 Nov 2026, 08:30 SGT — EG fixed two

Checklist owner **solution-architect-03**; executor **deployment-engineer-01**; manifest reviewer **developer-16**; GO/advancement **lead**. Frozen preparation is not execution approval. Blank evidence = **HOLD**. Use the [command contract](stage-command-review.md) and [sources](stage-review-sources.md); no CI wait.

**Pre-checks — initial every box with receipt/hash before stopping legacy EG**

- [ ] Signed S1 PASS and healthy AM2/PDB; AM remains fixed two. K01/K09–K15 local essentials qualify exact EG source/digest, all enabled ingress/console/provider paths, additive schema/record floors, bot/partition ownership, receipt recovery, fairness and same-work ≤30-second recovery. Neither service has an HPA/concurrent rollout.
- [ ] K13: corrected EG template and secure early owned-drain mechanism have exact source/operation PASS and real failure/drain evidence. **d389's EG shell exec hook is withdrawn.** #1996 native `sleep:30`/grace760 fixes credential isolation but alone does not qualify early drain. No old generated exec patch or invented `--replica-drain` use.
- [ ] K04/K07/K10/K11: shared console auth/logout, dedupe, global Identify/reconnect/provider budgets; original idempotency and uncertain-outcome custody survive handoff. Approved canaries cover every enabled provider; monitor/reconciler and 24-hour post-go-live on-call named before GO.
- [ ] K14/K16–K21: survivor/transition capacity including AM2, all clients/terminations and ≥30% Mongo headroom; reviewed placement constraints, eligible distinct hosts and local proof. Matched baseline, A3 classes, confidence method and post windows bound (B6: ≥3 baseline AND ≥3 post for current passive method). Pod-IP EG origin:8090 matches final binary.
- [ ] K22/K24: exact corrected release/operation/route hashes, per-step/outage/rollback approvals, actual controller/NEG proof and <120-second restoration evidence. Pre-pull is not application readiness. Both failed-cutover and healthy 2→1 recovery branches are bound. No production writes 02:00–02:59.

**GO sequence — separate guarded operation and readback at each checkpoint**

`S2-pause → corrected S2-prepare-template → S2-stop-legacy → S2-unpause-zero → S2-start-two → S2-pdb`. Pause legacy1, prepare modeON inactive template; old1→0; prove old bot sockets/partition writers closed or safely fenced; unpause0; validate candidate RS0; start0→2. The RS0-after-stop controller variant needs lead approval. Verify each pod and canonical backend, placement and ownership distribution before installing PDB `minAvailable:1`, `AlwaysAllow`, exact EG selector. Preserve AM2/PDB and all canonical routing.

**No-overlap clock:** last healthy old backend withdrawal → successful qualified new user service **<120 seconds**; record both-ready separately. At120 seconds abort promotion and invoke approved compatible recovery. Planned zero backends are allowed only inside this authorized interval; never count readiness alone as service restoration.

**30-minute soak — K02–K05/K07/K09–K11/K13–K24**

- [ ] Start after intended Ready/Available/backends/membership and handoff stabilize; prior terminations complete. Restart on image/config/count change. Per-minute ownership, custody/terminal, reconnect, queue/fairness, provider throttles, CPU/memory, Mongo/headroom and backend observations; ≥2 five-minute background cycles.
- [ ] Every accepted provider canary terminal or authoritatively reconciled; zero unresolved Unknown/Partial, no duplicate/lost/truncated replies. Reconciled failure remains failure. Preserve K10 queue p95 ≤2s/max ≤10s and ≥99.9% receipt/reply, K20 loaded bounds and all K16–K21 client/route targets with A3 confidence. Confirm AM has not regressed. Seal 24-hour monitor start/end, owner, scope and escalation.

**Abort/recovery:** wrong/stale/dual owner, duplicate effect, lost custody/receipt, auth bypass ⇒ immediate stop. Unknown/Partial ⇒ HOLD/page; unresolved at cohort deadline ⇒ compatible recovery, never blind resend. Apply S1's availability/capacity/performance abort rules. Healthy `S2-rollback-one` keeps EG modeON/fences/PDB and AM2; wait survivor reconnect and reconciliation. EG image recovery is forward-fix-only unless an exact older digest has explicit retained-record compatibility approval. No default pre-stage image undo; failed-start/unhealthy-owner branch must be separately bound.

**Decision:** GO/HOLD/ABORT ___; executor ___; reviewer ___; lead ___; release/receipts ___; soak ___→___; provider reconciliation/monitor ___ . Only signed S2 PASS permits S3.

<div style="page-break-after: always;"></div>

# S3 — 15 Nov 2026, 10:30 SGT — both HPAs, min2/max5

Checklist owner **solution-architect-03**; executor **deployment-engineer-01**; load/scale evidence **developer-17**; manifest reviewer **developer-16**; GO **lead**. Frozen preparation is not activation. Use the [command contract](stage-command-review.md); missing evidence = HOLD. No CI wait.

**Pre-checks — initial every box before creating either HPA**

- [ ] Signed S1/S2 PASS; both fixed two, healthy canonical backends, PDBs `minAvailable:1`/`AlwaysAllow`, zero terminating; unchanged qualified images/modeON/fences/record floors. Capture narrow pre-S3 specs/hashes. No rollout overlaps HPA operation; disable/remove HPA and wait all terminations before any later rollout.
- [ ] K12–K15: M3 actual N1/N2/2→4→2/kill evidence, secure drain and survivor recovery ≤30s. Qualified complete-cohort p95 handoff and **separate hard lifetime G** through process/socket/client exit exist; missing/censored timing cannot choose cadence.
- [ ] K14: capacity covers max5, scale-up and active+surge+terminating clients, all consumers, quota/fragmentation/node loss/provider budgets and ≥30% per-server Mongo headroom. With proven G760/P120/no-rollout, bound =5+ceil(760/120)=12/service; the captured40/80 CPU quota fails that envelope. Require fresh qualified capacity, not a configured cluster-autoscaler maximum.
- [ ] K21/K25: fresh v2 resource metrics, useful-load response, CPU idle/knee qualification and matched N1/N2/scaling evidence. AM60% of4CPU=2.4 cores; EG200% of250m=500m. CPU-only that cannot add useful capacity ⇒ HOLD for reviewed metric design. No assumed custom metrics API.
- [ ] Exact release/HPA payload and renderer hashes, route/per-operation grants, owned-load scope, freeze/delete/descent recovery and observation window bound. No production writes 02:00–02:59. Schedule leaves time for stabilization, full termination, final soak and M8; deadline cannot shorten them.

**GO sequence — render locally; guarded AM create/readback, then EG create/readback**

`S3-api-hpa.bound.json → S3-cmaeg-hpa.bound.json`; each target is its canonical Deployment. Read back `min2/max5`, fresh observedGeneration/metric values, `AbleToScale`/`ScalingActive`; explain `ScalingLimited`.

| Direction | Required policy |
| --- | --- |
| Up | stabilization0; Max of +2 pods/60s and +100%/60s |
| Down | stabilization300s; Min, −1 pod/P; P120s if qualified p95≤90s, otherwise ceil(p95+30s); maximum supported1800s, never clamp |

Prove useful **automatic** up/down for each service under approved load within capacity. Claim exercised5 only if observed5; configured ceiling capacity remains K14, useful automatic up/down remains K25. Manual replica patches do not satisfy K25. Remove only owned load, observe full down stabilization/cadence, handoff and actual exits to2. Record transitions, failures and peak clients; a PDB does not cap HPA terminations.

**30-minute final soak — K02–K21/K24/K25**

- [ ] Start after both return stable2, zero terminating, healthy backends/membership; changes restart it. Retain ramp/downscale strata separately. Each minute: metrics freshness/recommendations, useful throughput, owners/custody/streams/desktop/provider outcomes, process/client bounds, CPU/OOM, checkout/headroom and ≥2 background cycles. No oscillation or missing-metric downscale.
- [ ] All K16–K21 absolute/A3/confidence targets hold at N2 and scaling; no unobserved route PASS. Current passive method needs ≥3 baseline AND ≥3 post windows; one soak alone is INCONCLUSIVE (B6). Bind logs plus NyxID/client evidence for M8 13:00–17:00 and final18:00 report. Provider 24-hour monitoring continues beyond that deadline.

**Abort/recovery:** any S1/S2 safety, availability, capacity or supported performance trigger ⇒ stop promotion; halt only owned load. Generate/readback **both** HPA directions Disabled, then UID/resourceVersion-bound delete of only S3-created HPAs. Account for in-flight controller writes. With HPA absent, healthy survivors and zero prior terminations, descend **one at a time** to2, retaining PDBs/modeON/compatible images/custody. If demand requires >2, hold that safe fixed count and report autoscaling not delivered. Run fresh30-minute recovery soak; no image/mode downgrade or bulk5→2.

**Decision:** GO/HOLD/ABORT ___; executor ___; reviewer ___; lead ___; release/HPA/receipts ___; ramp/down ___; soak ___→___; M8 ledger ___ . Fixed replicas alone are not S3 PASS.

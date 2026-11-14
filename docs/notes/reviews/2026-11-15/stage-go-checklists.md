# S1 — 14 Nov 2026, ~22:30 SGT — AM fixed two

Checklist owner **solution-architect-03**; executor **deployment-engineer-01**; manifest reviewer **developer-16**; GO/advancement **lead**. Issue [#2016](https://github.com/ChronoAIProject/cma/issues/2016). Frozen preparation is not execution approval. Blank evidence = **HOLD**. Use the [command contract and open bindings](stage-command-review.md) and [frozen sources](stage-review-sources.md); no CI wait.

**Pre-checks — initial every box with source, results and actual observations before stopping legacy AM**

- [ ] K01/K15: consume the existing owner B9 packet and retained [M3/source GO](stage-command-review.md#revised-m3-merge-gate). Verifier02 prestaged actual APIgen200/source67b3fbc5; retained attempt2 APIcheckpoint PASS21:40:03.187916SGT. Final EG/web cohort comes from the owner packet; EG73 is a prefill only. B9 API→EG→web each has its15-minute checkpoint; use existing observed outcomes without another dependency-alert loop. Bind receiver/schema compatibility and actual stage grant.
- [ ] K03–K08/K13: bind log01 queries to actual release/replicas and the30-minute S1 interval; retain available prior baseline and explicit gaps without adding new baseline sampling to startup. Preserve logical command/effect identity and the immediate incident/ACK/compatible-rollback path. Accepted work is **real production traffic**; synthetic activity remains READS only, with the existing NyxID slot unchanged.
- [ ] K14: reviewed placement constraints, eligible distinct hosts, local placement proof, survivor-load and transition capacity qualified; refresh actual AM memory headroom (3273.7MiB observed versus256MiB requested). Count all active/surge/terminating clients with ≥30% per-server Mongo headroom. Direct relay uses Pod IP:8080. EG stays one; neither service has an HPA or competing release writer.
- [ ] K16–K21: under lead21:38 use the [prestaged S1 dispatch](stage-command-review.md#s1-prestaged-dispatch), targeting readiness within300s of the web checkpoint on the lead call. No new N1 baseline run, test suite or review wait is added. Prior log-baseline coverage remains explicit; bind actual source/config/backfill state and A3 classes. B10/Activity/U1 follow the blackout under the [amended ordering](stage-command-review.md#usage-and-replica-stage-order).
- [ ] K22/K24: S1 dry-run projections PASS using the #2043 controller from develop9be345523 or later containing the fix. Bind exact controller/release/operations, current fresh grants, outage authority, actual backend/user-service observation and recovery for failed cutover and healthy2→1. Images are pre-pulled. No production writes15Nov02:00–02:59; old unbound A2-r5 plans are not executable.
- [ ] Fresh S1 approval follows all three observed B9 checkpoint successes and projection PASS. B10 API-only source47809dbcf build/push may be queued after all three B9 checkpoints PASS; that queue supplies no B10 deployment approval/image. Each later stage waits for observed prerequisite success. U3 needs U2 `pass_complete` and≥15min from selected S starts; recompute on re-slot. Failure invokes approved rollback/report and lead re-slotting, never nominal-time advancement.

**GO sequence — per-operation prepare/review/execute/readback; no write loop**

`s1-pause → s1-prepare → s1-stop → s1-unpause → s1-start → s1-pdb`. Use the selected #2043 controller and successful projections with fresh operation grants. Keep canonical selectors/routes. Prepare ON template while paused1; stop old1→0; prove all legacy effect/socket authority stopped or safely revoked/expired; unpause0 and verify controller-owned candidate RS0; only then start0→2. Verify each pod serves, both backends healthy and actual pair on distinct hosts, then PDB `minAvailable:1`, `AlwaysAllow`, exact AM selector. A PDB does not prevent scale-to-zero or Deployment deletion.

**No-overlap clock:** last healthy old backend withdrawal → first successful user service on a qualified new backend **<120 seconds**; record both-ready separately. At120 seconds abort promotion even if a long kubectl wait remains. Never overlap legacy/owned writers or shorten leases/grace to meet time.

**30-minute soak; low-traffic extension up to30 more — K02/K03–K08/K13–K24**

- [ ] Start only after two Ready/Available, real backends/membership stable and handoffs/terminations complete. Record initial E=start+30min; report by actual E+15. If accepted production write commands N<20 at30min, extend the same observation by up to30 more minutes. If still N<20 after extension, **accept using passing required in-code multi-instance suites plus zero errors**, disclose actual low N and do not hold S1 for low traffic alone. Count distinct accepted commands, not retries/reads; retain first-window N/errors and cumulative extended N/errors without summing overlapping scopes. Preserve actual initial/extended bounds; image/config/count changes restart stability. Sample each minute and cover≥2 background cycles.
- [ ] From **real production traffic**, log-engineer-01 reports per-replica create/send/stop/stream counts, success and latency; ownership/epoch transitions; fencing refusals; ledger duplicate effects (**same logical command executed twice =0**); and5xx/429 by cause versus baseline. Tag backfill state true/false/unknown; split latency/error comparisons at actual transitions without pooling unlike states or resetting cumulative N/errors. Retain coverage, actual release/config and unobserved boundaries even under low-N acceptance. Observe natural handoffs only; same-work≤30s and K20/K21/absolute targets remain.
- [ ] Once S1 serving is stable, prod-verifier02 binds its [prestaged v3 READ session](stage-command-review.md#s1-read-sampler) to observed post-S1 APIgeneration>200, AM2/EG1, final EG/web and actual window. No30-minute N1 run precedes cutover arming. Keep the shared lock/≥90s retained clock; reads remain1800s/120slots/all3ranges,≤8GETs/round and960scheduled plus discovery. Report actual N/range/failures; target100 is not guaranteed. No automatic extension/restart, retry or polling; HTTP timing is not rendering.

**Abort/recovery:** observed accepted-work loss, duplicate/stale effect, auth/dual-desktop breach, unresolved terminal/effect failure, failed in-contract journey or absolute completion target ⇒ immediately stop promotion, halt owned synthetic reads and invoke approved compatible rollback. Do not wait for performance windows or the low-N extension. Restrict real admissions only through approved recovery controls. Other safety/availability/capacity triggers still apply; missing telemetry or unresolved outcomes ⇒ HOLD; low traffic alone follows the explicit exception. Healthy `s1-one` keeps modeON/image/epochs/PDB; wait drain and survivor service. If unsafe, use the approved failure branch. Never blindly resend Unknown, use generic undo or bypass with modeOFF.

**Decision:** GO/HOLD/ABORT ___; executor ___; reviewer ___; lead ___; release/results ___; initial/extended soak ___→___; accepted N ___; low-N acceptance/suite results ___; unobserved rows ___ . Only a signed S1 PASS permits S2.

<div style="page-break-after: always;"></div>

# S2 — 15 Nov 2026, ~00:30 SGT — EG fixed two

Checklist owner **solution-architect-03**; executor **deployment-engineer-01**; manifest reviewer **developer-16**; GO/advancement **lead**. Frozen preparation is not execution approval. Blank evidence = **HOLD**. Use the [command contract](stage-command-review.md) and [sources](stage-review-sources.md); no CI wait.

**Pre-checks — initial every box with source, results and actual observations before stopping legacy EG**

- [ ] Signed S1 stage PASS with actual real-traffic results and any explicit low-N acceptance; healthy AM2/PDB stays fixed two. K01/K09–K15: retain exact-source M3 EG suite/fixture scope; verify enabled ingress/console/provider paths, schema/record floors, bot/partition ownership, receipt recovery, fairness and same-work≤30s during the authorized stage. Neither service has an HPA/concurrent rollout.
- [ ] K13: corrected EG template and secure early owned-drain mechanism have source/operation PASS and real failure/drain evidence. **d389's EG shell exec hook is withdrawn.** #1996 native `sleep:30`/grace760 alone does not qualify early drain. Use guarded field patches preserving live env/envFrom; full historical EG apply adds absent owner-key settings. No old exec patch or invented `--replica-drain` use.
- [ ] K04/K07/K10/K11: shared console auth/logout, dedupe, global Identify/reconnect/provider budgets; original idempotency and uncertain-outcome custody survive handoff. Approved canaries cover every enabled provider; monitor/reconciler and 24-hour post-go-live on-call named before GO.
- [ ] K14/K16–K21: survivor/transition capacity including AM2, all clients/terminations and ≥30% Mongo headroom; reviewed placement constraints, eligible distinct hosts and local proof. Matched baseline, A3 classes and separate statistical/policy verdicts bound; apply only the narrow K21 CRITICAL exception in B6. Pod-IP EG origin:8090 matches final binary.
- [ ] K22/K24: selected controller/release/operations/route, per-step/outage/rollback approvals, actual controller/NEG proof and <120-second restoration evidence. Pre-pull is not application readiness. Both failed-cutover and healthy 2→1 recovery branches are bound. No production writes 02:00–02:59.
- [ ] Finish S2 and its full30-minute soak before02:00. Confirm enough time for actual termination, observation and approved recovery before the controller's01:58 write guard; never shorten them. If this cannot fit, hold/report for the lead to re-slot. U3 requires U2 `pass_complete` and at least15min from the selected S2 start; U/S grants remain separate.

**GO sequence — separate guarded operation and readback at each checkpoint**

`s2-pause → s2-prepare → s2-stop → s2-unpause → s2-start → s2-pdb`. Pause legacy1, prepare modeON inactive template; old1→0; prove old bot sockets/partition writers closed or safely fenced; unpause0; validate candidate RS0; start0→2. The RS0-after-stop controller variant needs lead approval. Verify each pod and canonical backend, placement and ownership distribution before installing PDB `minAvailable:1`, `AlwaysAllow`, exact EG selector. Preserve AM2/PDB and all canonical routing.

**No-overlap clock:** last healthy old backend withdrawal → successful qualified new user service **<120 seconds**; record both-ready separately. At120 seconds abort promotion and invoke approved compatible recovery. Planned zero backends are allowed only inside this authorized interval; never count readiness alone as service restoration.

**30-minute soak — K02–K05/K07/K09–K11/K13–K24**

- [ ] Start after intended Ready/Available/backends/membership and handoff stabilize; prior terminations complete. Declare E=start+30min and report by E+15; restart on image/config/count change. Per-minute ownership, custody/terminal, reconnect, queue/fairness, provider throttles, CPU/memory, Mongo/headroom and backend observations; ≥2 five-minute background cycles.
- [ ] Every accepted provider canary terminal or authoritatively reconciled; zero unresolved Unknown/Partial, no duplicate/lost/truncated replies. Reconciled failure remains failure. Preserve K10 queue p95 ≤2s/max ≤10s and ≥99.9% receipt/reply, K20 loaded bounds and all K16–K21 client/route targets with separate statistical/policy results. Confirm AM has not regressed. Seal 24-hour monitor start/end, owner, scope and escalation.

**Abort/recovery:** wrong/stale/dual owner, duplicate effect, lost custody/receipt, auth bypass ⇒ immediate stop. Unknown/Partial ⇒ HOLD/page; unresolved at cohort deadline ⇒ compatible recovery, never blind resend. Apply S1's availability/capacity/performance abort rules. Healthy `s2-one` keeps EG modeON/fences/PDB and AM2; wait survivor reconnect and reconciliation. EG image recovery is forward-fix-only unless an exact older digest has explicit retained-record compatibility approval. No default pre-stage image undo; failed-start/unhealthy-owner branch must be separately bound.

**Decision:** GO/HOLD/ABORT ___; executor ___; reviewer ___; lead ___; release/results ___; soak ___→___ before02:00; provider reconciliation/monitor ___ . Observed S2 PASS precedes the blackout and03:00 B10 API rollout→observed success→Activity/U1/U2. B10 and S3 need their own approvals and actual inputs. Failure rolls back/reports for lead re-slotting.

<div style="page-break-after: always;"></div>

# S3 — 15 Nov 2026, ~05:30–06:00 SGT — both HPAs, min2/max5

Checklist owner **solution-architect-03**; executor **deployment-engineer-01**; load/scale evidence **developer-17**; manifest reviewer **developer-16**; GO **lead**. Frozen preparation is not activation. Use the [command contract](stage-command-review.md); missing evidence = HOLD. No CI wait.

**Pre-checks — initial every box before creating either HPA**

- [ ] Signed S1/S2 PASS and observed B10 API rollout success; both fixed two, healthy backends/PDBs, zero terminating, compatible modeON/fences/floors. Bind actual post-B10 API and B9 EG images/Pod lifetimes; web stays B9. Capture pre-S3 specs. No rollout overlaps HPA operation; disable/remove HPA and wait all terminations before a later rollout.
- [ ] K12–K15: retain M3/S1/S2 evidence; qualify secure drain, same-work recovery≤30s, handoff p95≤90s and **separate hard lifetime G** for the actual post-B10 API/B9 EG cohort. Keep each source/replica lifetime separate; no pooled B9/B10 timing or old-API substitution. Missing/incompatible timing means HOLD. Both actual Pods/templates require grace760s including preStop; S3 automatic work remains mandatory.
- [ ] K14: qualify combined active+surge+terminating processes, all consumers, node loss/provider limits and ≥30% Mongo headroom against the selected64/128 CPU ceilings and actual nodes/memory. Illustrative G760/P120 gives12/service: it fits raised quota but exceeds47.67 eligible-node CPU. G remains unqualified; raising quota supplies no node or socket capacity.
- [ ] K21/K25: fresh v2 resource metrics, useful-load response, CPU idle/knee qualification and matched N1/N2/scaling evidence. AM60% of4CPU=2.4 cores; EG200% of250m=500m. CPU-only that cannot add useful capacity ⇒ HOLD for reviewed metric design. No assumed custom metrics API.
- [ ] Exact release/HPA payload and controller commit, route/per-operation grants, owned-load scope, freeze/delete/descent recovery and observation window bound. No production writes 02:00–02:59. Schedule leaves time for stabilization, full termination, final soak and M8; deadline cannot shorten them.
- [ ] Confirm observed prior-stage success and fresh S3 grant. Engineer02 targets03:00 for separately approved B10 API-only rolling at AM2/PDB, then **observed B10 success→Activity→U1→U2**. B10 approval/image/actuals remain unset until supplied. U2 missed before04:30 waits until after S3 success/fresh grant. #2038 is merged as8c44c3058; S3b still needs qualified inputs/approval. U3 needs `pass_complete` and≥15min from selected S starts.

**GO sequence — CPU quota64/128 → HPAs → separately approved merged S3b**

`s3-quota-raise → s3-am-create → s3-eg-create`. Before either HPA, use the exact S3 grant to change only `chrono-bot/sandbox-quota` CPU ceilings40/80→64/128. Save complete before-spec and confirmed result; require64/128 in both spec/status. Preserve memory/Pod limits. Each HPA targets its canonical Deployment; verify min2/max5, fresh metrics/generation and scaling conditions. Unknown writes require reconciliation.

| Direction | Required policy |
| --- | --- |
| Up | stabilization0; Max of +2 pods/60s and +100%/60s |
| Down | stabilization300s; Min, −1 Pod/120s fixed; handoff p95>90s or missing/censored timing ⇒ HOLD, never change cadence silently |

Run the separately approved S3b exercise from merged #2038 for observed automatic scale/drain evidence. It does not establish useful-load calibration or max-five performance acceptance; those qualifications remain separate. Claim exercised5 only if observed5; configured ceiling capacity remains K14, useful automatic up/down remains K25. Manual replica patches do not satisfy K25. Remove only owned load, observe full down stabilization/cadence, handoff and actual exits to2. Record transitions, failures and peak clients; a PDB does not cap HPA terminations.

**30-minute final soak — K02–K21/K24/K25**

- [ ] Start after both return stable2, zero terminating, healthy backends/membership; changes restart it. Declare E=start+30min; report by E+15. Keep ramp/downscale strata separate. Each minute: metrics freshness/recommendations, useful throughput, owners/custody/streams/desktop/provider outcomes, process/client bounds, CPU/OOM, checkout/headroom and ≥2 background cycles. No oscillation or missing-metric downscale.
- [ ] All K16–K21 absolute/A3 acceptance checks pass at N2 and scaling; no unobserved route PASS. Report statistical/policy verdicts separately; only valid matched CRITICAL observations meeting B6 may receive the exception. Bind logs/read/client evidence to prod-verifier-01's M8 kit for15Nov09:00–16:00 and the final18:00 report. Provider24-hour monitoring continues beyond that deadline. Failure invokes compatible rollback, immediate report and lead re-slotting.

**Abort/recovery:** any S1/S2 safety, availability, capacity or supported performance trigger ⇒ stop promotion; halt only owned load. Generate/readback **both** HPA directions Disabled, then UID/resourceVersion-bound delete of only S3-created HPAs. Account for in-flight controller writes. With HPA absent, healthy survivors and zero prior terminations, descend **one at a time** to2, retaining PDBs/modeON/compatible images/custody. If demand requires >2, hold that safe fixed count and report autoscaling not delivered. Run fresh30-minute recovery soak; no image/mode downgrade or bulk5→2.

Then `s3-quota-restore` needs its own grant and confirmed raise result: fixed2, no HPA/terminations, reconciled quota status and used CPU≤40/80. Otherwise retain raised ceilings during recovery. Preserve unrelated fields; no automatic restore after successful S3.

**Decision:** GO/HOLD/ABORT ___; executor ___; reviewer ___; lead ___; release/HPA/receipts ___; ramp/down ___; soak ___→___; M8 ledger ___ . Fixed replicas alone are not S3 PASS.

# Stage command review and execution bindings

Issue [#2016](https://github.com/ChronoAIProject/cma/issues/2016), reviewer solution-architect-03. Companion to the three [stage go/no-go pages](stage-go-checklists.md). **Documentary freeze may PASS while S1/S2/S3 execution remains HOLD.** The lead alone advances stages; deployment-engineer-01 executes. Developer-16 reviews exact manifest/projection bytes. This review ran no production commands, server dry-runs, builds or workloads.

## Authority, dates and evidence precedence

The hard-deadline plan and current essentials K01–K25 control: M2 integration 14 Nov22:00; freeze 15 Nov00:00; M3 local N1/N2/2→4→2/kill and merged/local-built batch9 by01:30; M4 N1/OFF and Activity index03:00–05:00; S1 06:00; S2 08:30; S3 10:30; M8 production verification13:00–17:00; final18:00 SGT. No production writes02:00–02:59, including rollback. Admin merge/local builds use the lead's integration plan; **never wait for CI**.

These replace the old03:15/04:15/05:30 schedule and13:00 final deadline. Essentials removes inherited AM101/EG10/ten-discovery/LC-16 repetition gates and max5 local trial requirements. Actual M3 boundary evidence remains required; production K22/K23/K25 observations occur in their scheduled stages, not before M3. A2 supplies compatible cutover, cadence and capacity semantics; A3 supplies class-specific comparisons. Historical packets remain immutable.

## Open execution bindings at preparation freeze

All entries below are **HOLD / NOT SUPPLIED IN THE REVIEWED PACKET**, not proof that a teammate has done no subsequent work. Close each with an exact reviewed receipt before the affected stage. Recheck material production changes; reuse unchanged evidence.

| ID / rows | Required closure | Responsible handoff |
| --- | --- | --- |
| B1 · K01/K15 | Final integrated #1370/EG source; local gates and image provenance; all retained receiver durations/floors; schema/role and configured capacity bindings. Current checked integration `15306eed8` is source provenance only. | developer-13, local verifiers, release engineer |
| B2 · K22/K24 | Lead-approved supported-controller variant and selected route; exact release-bound commands, grants and live-checkpoint dry-runs; canonical backend/NEG observer access. Current policy is `reviewed-release-controller`; A2-r5 direct executor refuses it and implements no controller route. | lead + deployment-engineer-01 |
| B3 · K22/K24 | Actual CMA/GKE/NEG no-overlap/restoration proof <120s, concurrent-writer/restart/interrupted-cutover cases; separately authorized recovery from failed start or unhealthy owners. Kind1.34 pause containers prove only their controller observations. | deployment-engineer-01 + local verifier |
| B4 · K13 | Secure early EG drain: authenticated mechanism, helper credential isolation, bounded success/failure, socket close before exact-token release, independent TERM handling. Native sleep30 alone does not close this gate. | developer-15/08 + developer-16 + architect-04 |
| B5 · K13/K14/K25 | Complete-cohort handoff p95; distinct hard effective lifetime G; all-process/per-server client/node/quota/provider inventory and ≥30% headroom; CPU calibration and actual metrics API. G760/P120→12 is illustrative, not an approved maximum. | debugger-03 + developer-16/17 + lead |
| B6 · K16–K21/K23/K25 | Three adequate final-N1 repeats, frozen classes/noise, current app/client telemetry and scoped canary/load permission; approved confidence method with adequate post windows; S2 reconciliation and named24h monitor. Current passive comparison needs ≥3 baseline AND ≥3 post windows: one31-minute post window cannot supply its bound by actual soak end+15minutes. Measurement-tool GO alone is not production evidence. | performance-analyst-02 + production verifiers + engineer |

Before each stage the reviewer records closures or reports `BLOCKED: URGENT — Bn, missing receipt, owner`; continue independent preparation. The00:00 freeze fixes the checklist criteria. Final source, passing/failing tests and a brief stage result supply closure; do not turn historical FAILs into PASS.

## Reviewed source facts

Six manifests at #1968 merge `d38966aab5c77fa2262da2d14ffba0da84cd958c` supply identities, HPA policy and PDBs. **#1996 merge `37f2bedbfce3be4fe5b8bd1370ba2c81cd872be5` supersedes its EG exec hook** with native `preStop.sleep.seconds:30`, keeping grace760 and guaranteed2Gi. At inspected integration `15306eed8aacb67964f95442c1f29ba47d161502`, the other five manifests are identical to d389; EG differs by the documented hook/comment correction. Final release must preserve this or a separately reviewed replacement.

Both PDBs select exactly `app.kubernetes.io/name` equal to their Deployment name, with `minAvailable:1` and `unhealthyPodEvictionPolicy:AlwaysAllow`. They protect eligible voluntary evictions, **not** Deployment scale-to-zero/deletion, involuntary loss or HPA-driven pod removal. Never combine `minAvailable` with a second `maxUnavailable` field. Rollback to one retains minAvailable1. Both topology rules use revision-aware `matchLabelKeys:[pod-template-hash]`; verify distinct hosts after old pods disappear, not only during overlap.

HPAs target canonical Deployments, min2/max5; CPU AM60% of4CPU and EG200% of250m. Up stabilization0, Max(+2 pods,+100%)/60s. Down stabilization300, Min(−1 pod)/P: P120 if p95≤90, otherwise ceil(p95+30), ≤1800. Missing/censored/failed timing cannot select P; over1800 requires a new design, no clamp. No simultaneous rollout/HPA. For no-rollout capacity, use5+ceil(G/P) per service only after proving G bounds all deletion delay, hooks and client/socket tails including failure/zombie behavior. Rollout generations need their own bound.

For each Mongo server: other consumers + Σ(max service processes × Σ(actual client pool cap + monitoring/topology sockets)) ≤0.70×verified server limit. Include jobs/startup/terminating processes; historical AM30+30+4+6 and illustrative EG10 are not final receipts. Historical40/80 requested/limit CPU quota fails the illustrative12/service envelope (51.025/120.25 cores including the captured other consumer). No quota/node change is authorized by these arithmetic examples.

## Exact command interface — execution HOLD until B1–B6 close

The following interface is transcribed from reviewed A2-r5. **Do not run its frozen old EG payload.** The corrected84-artifact engineer packet `bindings/corrected-eg-37f2bedbf-deadline-r1` is metadata/templates only: it has neither `scale_guard.py` nor `rollback_ops.py` and **cannot be `scale_root`, including after documentary PASS**. Deployment-engineer-01 must supply a separately reviewed executable packet with final operation/release bindings. `scale_root` must name that approved executable packet, not silently point to historical A2-r5. Retain old snapshots/hashes; never bind into frozen originals. For the selected reviewed-controller route, the engineer must provide the reviewed controller invocation for the same payloads. The direct helper below is usable only if the lead explicitly selects its permitted route and grants the exact operation; it cannot bypass the current route refusal.

```bash
set -euo pipefail
umask 077
scale_context=gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke
scale_namespace=chrono-bot
k() { kubectl --context "$scale_context" -n "$scale_namespace" "$@"; }
: "${scale_root:?approved corrected immutable operation packet}"
: "${scale_receipts:?new private receipt directory}"
: "${scale_review:?complete exact packet/tool/operation PASS file}"
: "${scale_review_sha256:?SHA256 of that verdict}"
: "${scale_op:?one approved operation JSON, never a directory}"
: "${scale_plan:?new plan file for this checkpoint}"
: "${scale_authorization:?fresh exact operation authorization JSON}"
TL_HOME='/Users/shining/code/work/tech-lead' \
  '/Users/shining/.claude/skills/shining-tech-lead/scripts/team.sh' \
  policy show ChronoAIProject/cma
python3.14 -B "$scale_root/scale_guard.py" prepare \
  --operation "$scale_op" --plan "$scale_plan"
shasum -a 256 "$scale_op" "$scale_plan" "$scale_root/scale_guard.py"
# Lead reviews the plan and supplies its authorization before this invocation.
python3.14 -B "$scale_root/scale_guard.py" execute \
  --plan "$scale_plan" --authorization "$scale_authorization" \
  --review "$scale_review" --review-sha256 "$scale_review_sha256"
```

The executor's single-use policy check binds context `cma-scale/<release_sha>/<stage>/<operation_id>`, not just a bare release SHA. It must retain exact image, plan, guard, verdict and authorization hashes; UID/resourceVersion/spec tests; fresh checkpoint status; write intent and readback. The A2-r5 implementation refuses01:58–02:59:59 to reserve command time; reserve complete stage/recovery duration separately. A timeout/readback mismatch is an uncertain mutation: reconcile reads before any retry. Spec drift stops; never regenerate around it silently. Fresh state-dependent dry-runs cannot be pre-completed against the wrong checkpoint. No Secret reads, pod exec, whole-object exports, hand-created ReplicaSets, generic rollout undo or directory-wide apply.

### S1/S2 operation names and actual IDs

These are historical interface names for reviewed replacements, **not ready-to-execute files**. Prefix `S` is S1 or S2; lowercase `s` is s1 or s2.

| Operation file | Actual JSON ID | Checkpoint/readback |
| --- | --- | --- |
| `S-pause.json` | `s-pause` | legacy desired1, pausedtrue, no HPA/competing writer; observed generation current |
| `S-prepare-template.json` | `s-prepare-paused-template` | release-bound final image/modeON only in inactive paused template; old process unchanged |
| `S-stop-legacy.json` | `s-stop-legacy` | approved1→0; no old/terminating processes or outstanding legacy authority |
| `S-unpause-zero.json` | `s-unpause-zero` | controller creates candidate RS0 after old stop; explicit variant approval; all owned RS0 |
| `S-start-two.json` | `s-start-two` | A2 exception0→2; exact template; independent service, both backends/owners/distinct hosts |
| `S-pdb.json` | `s-pdb` | install only after qualifiedtwo; exact absent-or-reviewed existing object and selector |
| `S-rollback-one.json` | `s-rollback-one` | healthy2→1 only; modeON, compatible image/PDB, no prior terminations, fresh survivor capacity |

Pause is not a replica lock. Bind controller ownership, eliminate other release/scale writers and verify every checkpoint. No shared ConfigMap activation under live legacy processes. Preserve canonical Service/Gateway/HTTPRoute/NEG identities; changed route needs a separate reviewed diff. Both AM/EG direct origins use Pod IP (8080/8090), with downward API `POD_IP` before address expansion.

Outage observer independently measures last old healthy backend withdrawal → successful user service on qualified replacement. At120s without restored service, abort promotion and use approved recovery even if a20m pod-deletion or35m rollout wait is outstanding. Neither wait extends the budget. Old socket/authority expiry must be proven independently of pod disappearance. Recovery may remain quiesced to preserve fences if safe takeover is impossible; report the breached interval, do not restart stale legacy writers.

### Recovery commands and limitations

S1/S2 normal recovery uses the exact `s1-rollback-one`/`s2-rollback-one` operation through the same per-operation contract. Fresh prepare/execute require every owner Ready/Available, generation current, no prior termination, survivor capacity ≤120s old. Therefore these operations **cannot stand in for failed-start/unhealthy-owner recovery**. Bind the separate compatible branch and outage authority before the first stop; missing branch is B3 HOLD. AM modeOFF requires all-stop, revoked/expired transport authority and authenticated exact-token receiver reset preserving epochs, with a separately reviewed tool. EG image downgrade requires explicit compatible retained-writer/record proof; routine repair is forward-only. M8 fixes are forward-only.

For S3, these generator commands only prepare operations from live reads. Each resulting write still needs the full policy/review/prepare/execute contract, and a unique output path; execute neither a write loop nor a bulk decrement.

```bash
: "${scale_hpa_name:?cma-agent-manager or cma-event-gateway}"
: "${scale_hpa_create_receipt:?actual S3 HPA creation readback receipt}"
: "${scale_freeze_op:?new freeze operation path}"
python3.14 -B "$scale_root/rollback_ops.py" freeze --name "$scale_hpa_name" \
  --created-receipt "$scale_hpa_create_receipt" --out "$scale_freeze_op"
# Review/authorize/execute; repeat for the other S3-created HPA before deletion.
: "${scale_delete_op:?new deletion operation path}"
python3.14 -B "$scale_root/rollback_ops.py" delete --name "$scale_hpa_name" \
  --created-receipt "$scale_hpa_create_receipt" --out "$scale_delete_op"
# Review/authorize/execute exact UID/RV deletion; confirm HPA absence.
: "${scale_descent_op:?new one-owner descent operation path}"
python3.14 -B "$scale_root/rollback_ops.py" descend --name "$scale_hpa_name" \
  --out "$scale_descent_op"
```

IDs are `s3-freeze-<name>`, `s3-delete-<name>`, `s3-descend-<name>-<n>-to-<n-1>`. Freeze **both directions** Disabled for both installed HPAs; delete only exact S3-created UIDs after server DELETE dry-run; account for last in-flight controller writes. No Deployment replicas patch with an HPA present. Descend only after prior actual termination and survivor proof; retain >2 if current demand requires it. Preserve PDBs, compatible images, modeON and custody. Fresh30-minute recovery soak is mandatory; fixed replicas are not autoscaling PASS.

### HPA rendering and readbacks

The following local renderer is already part of the repository; do not invent another policy generator. Use an approved exact source snapshot with the corrected hook, not a moving checkout. Bind its JSON into each separate reviewed S3 HPA operation; rendering is not a create approval.

```bash
: "${scale_source:?approved exact manifest source snapshot}"
: "${qualified_handoff_p95_seconds:?complete qualified cohort p95}"
ruby "$scale_source/.github/scripts/cma-scale-manifest-contract.rb" \
  --hpa api "$qualified_handoff_p95_seconds" > "$scale_receipts/api-hpa-qualified.json"
ruby "$scale_source/.github/scripts/cma-scale-manifest-contract.rb" \
  --hpa cmaeg "$qualified_handoff_p95_seconds" > "$scale_receipts/cmaeg-hpa-qualified.json"
```

The historical create IDs are `s3-api-hpa` / `s3-cmaeg-hpa`; bound files are `S3-api-hpa.bound.json` / `S3-cmaeg-hpa.bound.json`. AM create/idle readback precedes EG. Read only scoped fields, retain aggregates/hash-only identities:

```bash
k get deployment cma-agent-manager cma-event-gateway \
  -o custom-columns=NAME:.metadata.name,GEN:.metadata.generation,OBSERVED:.status.observedGeneration,DESIRED:.spec.replicas,READY:.status.readyReplicas,AVAILABLE:.status.availableReplicas
k get pdb cma-agent-manager cma-event-gateway -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{.spec}{"\n"}{.status}{"\n"}{end}'
k get hpa cma-agent-manager cma-event-gateway -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{.spec}{"\n"}{.status}{"\n"}{end}'
```

Absent HPA before S3 is expected, not a healthy HPA result. EndpointSlice readiness cannot replace canonical external backend/NEG health. The authorized observer must bind the exact backend name and use the reviewed GCP health path; permission denial or unknown mapping is HOLD. Do not print raw endpoint/user identities or scrape a Secret to fill a capacity gap.

## Soak evidence and verdict rules

Start30 minutes only after intended Ready/Available, backend/NEG, membership and handoff are stable, with prior terminations complete. Reset on image/config/count changes; final S3 soak is after automatic return to2. Per-minute samples plus finer approved failure capture must cover two five-minute background cycles, all process/client/capacity bounds and K02 custody accounting. Metadata-only watchers cannot prove user-service recovery, complete client journeys or absence of committed stale effects.

K16–K19: usable plain/desktop create ≤5s, full open ≤1s, shown durable send ≤1s, delivered/UI-acknowledged interrupt ≤1s, rendered usage summary/page ≤2s. Activity API p95 <1s; Activity page completion is measured separately with no invented absolute page ceiling. K20: request p95≤500ms/p99≤2s, stream-header p99≤2s, terminal p99≤5s/every terminal≤30s; EG checkout p99≤50ms, ordinary event≤50 Mongo operations and ≤10% same-fixture cost regression. Count failed, refused, timed-out and incomplete journeys separately; missing terminal fails.

K21 A3: CRITICAL within preregistered measured noise; UX-AFFECTING ≤+10%p95 (aim≤5%); detached NON-SENSITIVE ≤+30ms. The upper confidence bound must fit; no extra noise allowance on10%/30ms, no post-result reclassification. User-waited background/relay work inherits the critical boundary. Retain every43 catalog membership and distinct route/mode; no pooled PASS. Three matched final-N1 windows; final-N1 versus approved release, N2/scaling/fault versus matching N1; p99 needs≥1000/stratum. Inadequate samples are INCONCLUSIVE, not zero failures. Full-matrix repetition counts are not reintroduced.

The current passive bootstrap requires **at least three comparable baseline AND three post windows**. A single31-minute post window cannot supply its confidence bound by actual soak end E+15minutes. No qualified production-method substitute is supplied. A30-minute soak is the stability minimum, not automatic statistical acceptance. Bind an approved matched driver/confidence method and raw receipts or a preregistered adequate longer arrangement before execution; otherwise report INCONCLUSIVE on time. Record actual stabilization R and soak end E; never extend them retrospectively, treat overlapping windows as independent, invent a new estimator or borrow local noise. Preserve early-stage failures in full-stage denominators.

M4 retains the lead's after03:00 Activity order. The preferred31-minute baseline proposal needs final candidate/configuration and index postcheck stationary by03:55, warm-up to04:05, then04:05–04:36 /04:38–05:09 /05:11–05:42 and validated handoff05:55. It is a conditional scheduling proposal, not an observation grant or feasibility receipt. Actual valid midpoint coverage must be≥1800s per window; timing alone proves neither independence nor matched work. A pre-blackout index needs an explicit lead sequence change. Neither option moves S1 or authorizes shortening/making up windows. B6 remains HOLD until the actual schedule, method and authority are bound.

Immediate ABORT for stale/wrong-owner committed effect, duplicate external action, lost accepted custody/terminal/receipt, dual desktop/bot authority, auth bypass, epoch/cursor regression or incompatible receiver. Stop promotion/admission using approved controls; preserve custody; if rollback risks duplicates, remain safely quiesced with compatible fences and escalate.

Availability/capacity ABORT: zero healthy canonical backends outside the approved cutover; no restored service at120s within it; same-work sustained recovery>30s; OOM/repeated crash/permanent renewal failure; process/client/provider cap breach or Mongo headroom<30%. Widespread timeouts/5xx do not wait for performance windows. Performance-only supported class/absolute failure across two consecutive five-minute windows triggers recovery; any failing whole30-minute comparison fails the stage. Missing data, unbound configuration, unstable/low-sample baseline or Unknown outcome means HOLD; the approved cohort window expiry selects compatible recovery, not silent promotion.

Each stage receipt records source/image/config/tool/operation/review hashes, approval context, UTC/SGT interval and both outage/ready clocks, K-row PASS/FAIL/NOT_RUN/INCONCLUSIVE with sample denominators, per-route confidence results, peak process/socket/headroom, PDB/HPA/backend history and actual recovery actions. Review owner initials every gate; engineer attests observations; lead signs GO/next stage. S2 also records every provider's accepted/terminal/reconciled/unresolved counts and assigned24-hour monitoring. M8 logs plus NyxID read/client evidence complete production verdicts by17:00 for18:00 reporting.

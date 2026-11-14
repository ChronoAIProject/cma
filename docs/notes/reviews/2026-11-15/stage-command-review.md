# Stage command review and execution bindings

Issue [#2016](https://github.com/ChronoAIProject/cma/issues/2016), reviewer solution-architect-03. Companion to the three [stage go/no-go pages](stage-go-checklists.md). **Documentary freeze may PASS while S1/S2/S3 execution remains HOLD.** The lead alone advances stages; deployment-engineer-01 executes. Developer-16 supplies manifest and capacity input. This review ran no production commands, server dry-runs, builds or workloads.

## Authority, dates and evidence precedence

The hard-deadline plan and current essentials K01–K25 control: M2 integration 14 Nov22:00; freeze 15 Nov00:00; M3 local N1/N2/2→4→2/kill and merged/local-built batch9 by01:30; M4 N1/OFF and Activity index03:00–05:00; S1 06:00; S2 08:30; S3 10:30; M8 production verification13:00–17:00; final18:00 SGT. No production writes02:00–02:59, including rollback. Admin merge/local builds use the lead's integration plan; **never wait for CI**.

These replace the old03:15/04:15/05:30 schedule and13:00 final deadline. Essentials removes inherited AM101/EG10/ten-discovery/LC-16 repetition gates and max5 local trial requirements. Actual M3 boundary evidence remains required; production K22/K23/K25 observations occur in their scheduled stages, not before M3. A2 supplies compatible cutover, cadence and capacity semantics; A3 supplies class-specific comparisons. Historical packets remain immutable.

## Open execution bindings at preparation freeze

The following qualification and observation requirements remain open at this update. Close the applicable entries with source, passing/failing results and the required production approvals before advancing. Recheck material production changes; reuse unchanged evidence.

| ID / rows | Required closure | Responsible handoff |
| --- | --- | --- |
| B1 · K01/K15 | Final integrated #1370/EG source; local gates and image provenance; all retained receiver durations/floors; schema/role and configured capacity bindings. Intermediate integration heads are source provenance only. | developer-13, local verifiers, release engineer |
| B2 · K22/K24 | Select and qualify controller `08ce4653d` and RS0-after-stop variant; bind final release and per-operation grants/dry-runs. Concrete next dependency: authorized canonical backend/NEG plus user-service observer invocation and old/new mapping. Historical A2-r5 cannot execute the current route. | lead + deployment-engineer-01 |
| B3 · K22/K24 | Actual CMA/GKE/NEG no-overlap/restoration proof <120s, concurrent-writer/restart/interrupted-cutover cases; separately authorized recovery from failed start or unhealthy owners. Kind1.34 pause containers prove only their controller observations. | deployment-engineer-01 + local verifier |
| B4 · K13 | Secure early EG drain: authenticated mechanism, helper credential isolation, bounded success/failure, socket close before exact-token release, independent TERM handling. Native sleep30 alone does not close this gate. | developer-15/08 + developer-16 + architect-04 |
| B5 · K13/K14/K25 | Complete-cohort handoff p95; distinct hard effective lifetime G; all-process/per-server client/node/quota/provider inventory and ≥30% headroom; CPU calibration and actual metrics API. G760/P120→12 is illustrative, not an approved maximum. | debugger-03 + developer-16/17 + lead |
| B6 · K16–K21/K23/K25 | Matched final-N1 comparisons, frozen classes/noise, observed client/absolute checks and approved measurement scope; S2 reconciliation and named24h monitor. Keep30-minute soaks and E+15 reporting. Statistical INCONCLUSIVE may receive CRITICAL policy PASS only under the later K21 conditions below; no exception for missing data or UX/background. | performance-analyst-02 + production verifiers + engineer |

Before each stage the reviewer records closures or reports `BLOCKED: URGENT — Bn, missing receipt, owner`; continue independent preparation. The00:00 freeze fixes the checklist criteria. Final source, passing/failing tests and a brief stage result supply closure; do not turn historical FAILs into PASS.

## Reviewed source facts

Six manifests at #1968 merge `d38966aab5c77fa2262da2d14ffba0da84cd958c` supply identities, HPA policy and PDBs. **#1996 merge `37f2bedbfce3be4fe5b8bd1370ba2c81cd872be5` supersedes its EG exec hook** with native `preStop.sleep.seconds:30`, keeping grace760 and guaranteed2Gi. At inspected integration `15306eed8aacb67964f95442c1f29ba47d161502`, the other five manifests are identical to d389; EG differs by the documented hook/comment correction. Final release must preserve this or a separately reviewed replacement.

Both PDBs select exactly `app.kubernetes.io/name` equal to their Deployment name, with `minAvailable:1` and `unhealthyPodEvictionPolicy:AlwaysAllow`. They protect eligible voluntary evictions, **not** Deployment scale-to-zero/deletion, involuntary loss or HPA-driven pod removal. Never combine `minAvailable` with a second `maxUnavailable` field. Rollback to one retains minAvailable1. Both topology rules use revision-aware `matchLabelKeys:[pod-template-hash]`; verify distinct hosts after old pods disappear, not only during overlap.

HPAs target canonical Deployments, min2/max5; CPU AM60% of4CPU and EG200% of250m. Up stabilization0, Max(+2 pods,+100%)/60s. Down stabilization300, Min(−1 pod)/P: P120 if p95≤90, otherwise ceil(p95+30), ≤1800. Missing/censored/failed timing cannot select P; over1800 requires a new design, no clamp. No simultaneous rollout/HPA. For no-rollout capacity, use5+ceil(G/P) per service only after proving G bounds all deletion delay, hooks and client/socket tails including failure/zombie behavior. Rollout generations need their own bound.

For each Mongo server: other consumers + Σ(max service processes × Σ(actual client pool cap + monitoring/topology sockets)) ≤0.70×verified server limit. Include jobs/startup/terminating processes; historical AM30+30+4+6 and illustrative EG10 are not final receipts. Historical40/80 requested/limit CPU quota fails the illustrative12/service envelope (51.025/120.25 cores including the captured other consumer). No quota/node change is authorized by these arithmetic examples.

## Current execution commands — selection and qualification still required

Engineer commit `08ce4653d5777ca766ab449d9e7ec6b1e6acf983` supplies the [execution runbook](https://github.com/ChronoAIProject/cma/blob/08ce4653d5777ca766ab449d9e7ec6b1e6acf983/docs/notes/reviews/2026-11-15/deployment-runbooks.md), with complete `s1-execution.md`, `s2-execution.md` and `s3-execution.md` pages beside it. Use those canonical commands after lead selection and actual qualification. This checklist update is not a tooling review or execution grant. The engineer reports33 offline tests passing; they do not prove CMA/GKE/NEG cutover or user service.

The new `.github/scripts/cma_scale/controller.py` implements one-operation plan/server dry-run, context-bound approval, CAS apply and readback for `reviewed-release-controller`. The historical A2-r5 direct-helper commands are superseded here. The old corrected metadata directory still contains no helpers; never use it as an executable root or reuse old EG exec derivatives.

```bash
set -euo pipefail
: "${scale_tools:?selected controller source directory}"
: "${scale_release:?qualified release JSON}"
: "${scale_action:?one action from the stage runbook}"
: "${scale_plan:?new plan path for the current checkpoint}"
# plan reads production and performs server dry-run; observation authority is required.
python3 -B "$scale_tools/controller.py" plan --action "$scale_action" \
  --release "$scale_release" --out "$scale_plan"
# Lead grants the exact printed context before apply; one grant per operation.
: "${scale_approval:?fresh exact operation approval}"
python3 -B "$scale_tools/controller.py" apply \
  --plan "$scale_plan" --approval "$scale_approval"
```

Stage-specific extra arguments, especially recovery `--created` and pre-stop restoration inputs, come from the canonical stage page. Do not run a sequence/write loop. Every checkpoint rechecks actual object identity/generation/spec, observed state, images, ownership and prior terminations. An uncertain write stops progression; reconcile it before a new plan. A new approval cannot justify replay of an ambiguous mutation. The controller refuses writes01:58–02:59 SGT, including recovery; reserve full recovery time separately.

| Purpose | Current action names | Required evidence |
| --- | --- | --- |
| S1/S2 progression | `s1-pause`, `s1-prepare`, `s1-stop`, `s1-unpause`, `s1-start`, `s1-pdb`; corresponding `s2-` actions | Qualified old1; inactive ON template; old0 and authority cessation; controller candidate RS0; new2 on distinct hosts with real service; then PDB. |
| Abort before old stop | `s1-abort-before-stop`, `s2-abort-before-stop` | Original serving state still qualified; exact original template inputs; uncertain abort reconciled before any stop. |
| Healthy recovery | `s1-one`, `s2-one` | Current Ready/Available generation, no prior terminations, survivor capacity≤120s old; retain compatible mode/image/floors/PDB. |
| Failed-start/unhealthy recovery | `s1-quiesce`, `s1-recovery-image`, `s1-unpause`, `s1-recover-one`; corresponding `s2-` actions | Separately authorized outage; all-stop and proven authority cessation; qualified compatible ON image; controller-owned restart; real survivor service and fresh soak. |
| S3 creation | `s3-am-create`, then `s3-eg-create` | Both fixed2, timing/capacity qualified, no rollout, valid metrics and exact final HPA policy. |
| S3 recovery | `s3-am-freeze`, `s3-eg-freeze`; then their `delete`; then one `descend` at a time | Both directions Disabled; exact-created UID/RV deletion using actual creation results; HPA absence and in-flight-write reconciliation; survivor proof and actual exit between decrements. |

Pause is not a replica lock. Reserve one release/scale writer, preserve canonical Service/Gateway/HTTPRoute/NEG identities, and never hand-create/adopt a ReplicaSet. No shared ConfigMap activation under live legacy processes. Both direct origins use Pod IP (AM8080/EG8090). No Secret reads, pod exec, whole-object exports, generic rollout undo, unverified modeOFF or bulk downscale.

The concrete observer invocation and old/new backend mapping are still absent. EndpointSlice or Pod Ready cannot establish successful user service. Independently time last old healthy backend withdrawal → successful service on a qualified replacement, strictly<120s; record both-ready separately. At120s without restoration, abort promotion and use approved compatible recovery even if a long controller wait remains. Do not shorten grace/authority or force-delete. Pod disappearance alone does not prove remote authority cessation.

S3 freeze/delete/descent preserves modeON, compatible images, custody and PDBs. Do not patch Deployment replicas while HPA exists. If demand needs>2, retain the safe fixed count and report autoscaling incomplete. A fresh30-minute recovery soak follows. Eight author driver tests are preparation evidence; the proposed8RPS workload remains uncalibrated and needs its distinct load approval. Engineer02 alone owns M4/index operations.

## Soak evidence and verdict rules

Start30 minutes only after intended Ready/Available, backend/NEG, membership and handoff are stable, with prior terminations complete. Reset on image/config/count changes; final S3 soak is after automatic return to2. Per-minute samples plus finer approved failure capture must cover two five-minute background cycles, all process/client/capacity bounds and K02 custody accounting. Metadata-only watchers cannot prove user-service recovery, complete client journeys or absence of committed stale effects.

K16–K19: usable plain/desktop create ≤5s, full open ≤1s, shown durable send ≤1s, delivered/UI-acknowledged interrupt ≤1s, rendered usage summary/page ≤2s. Activity API p95 <1s; Activity page completion is measured separately with no invented absolute page ceiling. K20: request p95≤500ms/p99≤2s, stream-header p99≤2s, terminal p99≤5s/every terminal≤30s; EG checkout p99≤50ms, ordinary event≤50 Mongo operations and ≤10% same-fixture cost regression. Count failed, refused, timed-out and incomplete journeys separately; missing terminal fails.

K21 keeps statistical and acceptance verdicts separate. CRITICAL uses measured noise; UX p95≤+10% (aim≤5%); detached NON-SENSITIVE p95≤+30ms, with the existing confidence methods. No added noise allowance, result-driven reclassification or borrowed local noise. User-waited background/relay work inherits the critical boundary. Keep every catalog membership and route/mode separate; p99 needs≥1000/stratum.

**Later lead ruling, S1/S2/S3 only:** a valid observed matched CRITICAL statistical INCONCLUSIVE may receive policy PASS when the signal is explicitly assessed as no adverse signal and all applicable absolute checks pass. Statistical INCONCLUSIVE remains visible. Beyond-noise signal, failed/incomplete observations or absolute failure means FAIL. Missing observations are NOT_RUN; invalid data INVALID; unassessed context/signal INCONCLUSIVE. No exception applies to UX/background, missing client boundaries, empty denominators or M3 local gates.

**Keep30-minute soaks; report by actual E+15minutes.** The longer-soak request is closed. Three baseline/post windows remain requirements of the existing statistical method, not an independent reason to block the expressly permitted CRITICAL policy PASS. Do not change the estimator, expand measured noise or relabel statistical results. Record actual stable start S and declare E=S+30min; changes restart stability, with all earlier failures retained.

Before HTTP/resource windows are S1 05:28–05:58, S2 07:58–08:28 and S3 09:58–10:28 SGT. After evidence covers actual S–E. Optional31-minute direct metrics adds scrape coverage without extending E. The existing authorized NyxID harness reads Agent detail/transcript and retained usage24h against the same final-N1 profile; it adds no create/send/stop/refresh mutation. HTTP body timing does not prove browser rendering or unmeasured client journeys. Retain the after03:00 Batch9/soak/index order; engineer02 is its sole operator.

Immediate ABORT for stale/wrong-owner committed effect, duplicate external action, lost accepted custody/terminal/receipt, dual desktop/bot authority, auth bypass, epoch/cursor regression or incompatible receiver. Stop promotion/admission using approved controls; preserve custody; if rollback risks duplicates, remain safely quiesced with compatible fences and escalate.

Availability/capacity ABORT: zero healthy canonical backends outside the approved cutover; no restored service at120s within it; same-work sustained recovery>30s; OOM/repeated crash/permanent renewal failure; process/client/provider cap breach or Mongo headroom<30%. Widespread timeouts/5xx do not wait for performance windows. Performance-only supported class/absolute failure across two consecutive five-minute windows triggers recovery; any failing whole30-minute comparison fails the stage. Missing data, unbound configuration, invalid/unmatched baseline or Unknown outcome means HOLD. Statistical sample inadequacy follows the narrow K21 policy rule above; the approved cohort window expiry selects compatible recovery, not silent promotion.

Each stage result records source/controller commit, images/configuration, exact operation/approval outcome, UTC/SGT interval and both outage/ready clocks, separate statistical/policy verdicts and K-row PASS/FAIL/NOT_RUN/INVALID/INCONCLUSIVE with sample denominators, per-route confidence results, peak process/socket/headroom, PDB/HPA/backend history and actual recovery actions. Review owner initials every gate; engineer attests observations; lead signs GO/next stage. S2 also records every provider's accepted/terminal/reconciled/unresolved counts and assigned24-hour monitoring. M8 logs plus NyxID read/client evidence complete production verdicts by17:00 for18:00 reporting.

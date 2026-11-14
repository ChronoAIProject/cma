# Stage command review and execution bindings

Issue [#2016](https://github.com/ChronoAIProject/cma/issues/2016), reviewer solution-architect-03. Companion to the three [stage go/no-go pages](stage-go-checklists.md). **Documentary freeze may PASS while S1/S2/S3 execution remains HOLD.** The lead alone advances stages; deployment-engineer-01 executes. Developer-16 supplies manifest/capacity input. Initial review was offline; later authorized observation reads are recorded in the [S1 companion](s1-observation.md). No production writes, builds or workloads were run by this reviewer.

## Authority, dates and evidence precedence

The lead20:30 schedule, with engineer02's reported20:45 amendment, sets B9 API→EG→web from14Nov21:00 with a15-minute checkpoint each; S1~22:30; S2~15Nov00:30 fully soaked before02:00; no writes02:00–02:59; B10 API-only rolling at AM2/PDB targeted03:00 before observed-success→Activity/U1/U2; S3~05:30–06:00; M8 09:00–16:00 using prod-verifier-01's kit; final18:00. Fresh approval and observed prerequisite success govern each stage. Failure invokes approved rollback/report and lead re-slotting. Admin integration uses local gates, **never CI waiting**.

This20:30 schedule supersedes all earlier stage times, including the19:56 U/S timeline. The18:00 final deadline and current K01–K25/A3 behavior criteria remain. Essentials removes inherited repetition gates; the revised M3 scope below remains the local qualification contract, without a new campaign. A2 supplies compatible cutover/capacity definitions where consistent with later lead selections.

**Current scope — verifier02 reports lead22:15 PREPARE ONLY:** prepare API + web retry targets while retaining B8 EG. The attempt2 APIg200/EG73 anchor is archived; earlier checkpoint PASS does not survive the later failed train as current readiness. The former21:38/300s dispatch target is inactive pending lead disposition. Compatibility disposition, fresh deployment approval and actual retry observations remain architect/lead/execution-owner inputs. Add no new N1 baseline sampling, review, test suite or repeated dependency-alert loop. Missing baseline remains explicit; no retry, build, binder or runtime action is authorized by this preparation.

## Usage and replica stage order

The20:30 schedule plus engineer02's reported lead20:45 B10 amendment supplies the retained plan below. The later22:15 PREPARE ONLY scope governs current readiness; a failed train cannot advance by nominal time. Engineer01 records actual disposition in `.github/.cmaeg-local/scale-stages-20261115/TIMELINE.md` and `s1-execution-packet.json` in its `cma-cmaeg-release` worktree. Developer13 owns the shared [stage timeline](stage-timeline.md); engineer02 owns retry/B10/Activity/U1–U3, engineer01 S1–S3. B10 deploy approval/image/actuals remain unset. Existing fresh stage/per-operation controls continue.

| Step | Controlling order |
| --- | --- |
| B9 original train / retry preparation | The original API→EG→web train failed and rolled back. Current preparation targets API+web with B8 EG retained at N1/shipped defaults. Bind the future retry's actual approval, compatibility disposition and required checkpoint results; archived API/EG PASS is not retry success. No retry is launched under PREPARE ONLY. |
| B10 build/push queue | Retained planned API-only source `47809dbcf2193b4e28fca679f1b84bd0c27ac82c`; the failed train does not satisfy the original queue prerequisite. Lead/owner disposition must bind any future queue. Source is not a completed build, selected image or deploy approval. |
| S1 ·former14Nov~22:30 target | AM2+PDB only after approved retry outcomes, mixed-cohort compatibility and successful S1 projections with #2043 at develop9be345523 or later retaining the fix. Actual time/grant remains unset. Prior baseline gaps stay explicit; no new N1 sampling wait. |
| S2 ·15Nov~00:30 | EG2 after observed S1 success and fresh S2 grant. Finish operation and full30-minute soak before02:00, preserving time for termination and approved recovery; respect the controller's earlier01:58 write guard. If it cannot fit, report for lead re-slotting. |
| Blackout ·15Nov02:00–02:59 | No production writes, including recovery; do not enter it with a planned write-dependent completion. |
| B10 API ·15Nov03:00 target | Engineer02 uses a **separate B10 deploy approval** for API-only rolling at AM2/PDB. Bind actual retained EG/web sources, including B8 EG if retained; no HPA/rolling overlap. Actual B10 image, start/end, source/replica binding and checkpoint result remain unset. |
| Activity/U1/U2 ·after observed B10 success | B10 API success precedes Activity index → successful U1 collection/index Job → U2, with observed predecessor success and fresh stage grants. No assumed03:00 Activity start. U1 remains `GRANT_NOOP` for credentials: existing AM `readWrite`+`dbAdmin` requires no new role/credential operation. |
| U2 cutoff/fallback | After U1 success, U2 must start strictly before04:30; exactly04:30 or later defers until after successful S3 and a fresh U2 grant. The former06:45/after-S1 fallback is withdrawn. |
| S3 ·15Nov~05:30–06:00 | After preceding-stage success, fresh S3 grant: quota64/128 → HPAs → qualified/approved S3b from merged #2038 (`8c44c3058`). Bind actual **post-B10 API/retained EG** sources and lifetimes; do not assume B9 EG. Deferred U2 remains after S3, so its completion is not a new S3 prerequisite. |
| U3 | Only after actual U2 `pass_complete`, with launch at least15minutes from every selected S-stage start. Engineer02 confirms the current schedule with engineer01 and recomputes after any lead re-slot; do not reuse old fixed exclusion times. |
| M8/final ·15Nov | Production verification09:00–16:00 using prod-verifier-01's kit; final owner report by18:00. Stage reports remain due at actual E+15. |

Log01 binds S1/S2 to actual approved retry images and serving processes, including retained B8 EG if selected. The former21:30–22:30 baseline is historical; prior collectors/maintenance intervals do not cover new Pods automatically. Every before interval is `[actual_operation_start−60min, actual_operation_start)`; rebind when the operation moves and retain missing/partial coverage without a new N1 run. For S3, record actual B10 API rollout boundaries and each post-B10 API/retained EG replica/process lifetime. Never pool across failed-attempt, recovery, retry or B10 boundaries; old-API observations cannot replace required post-B10 observations.

Operation start, stable-soak S/E and synthetic-read S/E remain distinct. B10→observed success→Activity/U1/U2 follows S2 and the blackout and is not an evening S1 prerequisite. Planned ordering or an unset U2 start does not prove inactivity. Engineer02 supplies actual rollout and U-state identities/timestamps; compare known equal source/config/backfill states. Image transition cohorts stay separate, with earlier errors retained and affected stable observations rebound; no automatic synthetic read restart or borrowed prior-source qualification.

If an actual backfill-state transition occurs within an observation, split latency/error results without resetting original S, accepted-command N or cumulative errors. Image/config/count changes restart stability with prior failures retained; backfill segmentation does not replace that reset. Do not pool unlike states or sum overlapping scopes. The fixed1800-second owned READ session, low-N rule and immediate incident/ACK/compatible-recovery path remain. Revised times require actual owner session bindings, never automatic execution/restart. #2038 qualification remains separate.

## Revised M3 merge gate

The controlling lead ruling is published at the top of architect04's `essentials.md` and `production-stages.md`. It defines these four inputs; the status attribution below does not claim new reviewer execution:

| Input | Required result / owner |
| --- | --- |
| (a) Local source/resources, placement, PDB, readiness | PASS accepted under the lead ruling. |
| (b) f6 cross-ingress host diagnostic at N1/N2, OFF/ON | PASS accepted under the lead ruling; host diagnostic scope remains explicit. |
| (c) All required integration-head multi-instance suites | Local-verifier02 runs against one real Mongo with two or more application instances; developer17 supplies exact filters. Every suite needs its actual source/command and PASS/FAIL. Pending or unrun is not PASS. |
| (d) Environment01 EG fixture cases | PASS accepted under the lead ruling; fixture scope remains explicit. |

Suite(c) covers killed-owner takeover, leases/fencing/no duplicate effects, recovery leases, title custody, desktop exclusive sessions/receiver boundaries and EG delivery/SSE. Retain the owners' actual tested source/results and architect04 disposition from merged #1370; this is the qualification contract, not a request to repeat the completed campaign. The lead20:30 ruling reports Batch9 ready. Actual deployment inputs and the separate #2043 controller projections still belong to the execution owners.

The missing local real-runtime/auth/provider adapter is no longer an M3 merge blocker. This changes **where qualification occurs**: it does not turn host/component evidence into deployed end-to-end PASS. Real accepted-work verification moves to production S1 with immediate approved compatible rollback on failure. **The later lead answer to #116 and19:10 ruling supersede the synthetic-write plan:** use real production traffic observed by log-engineer-01 during30minutes, with the explicit low-N extension/acceptance below. Synthetic reads cover detail/transcript and usage24h/7d/30d only; no broker creates/sends or NyxID slot change. A3, absolute targets, safety and production operation grants remain unchanged.

## Open execution bindings at preparation freeze

The following qualification and observation requirements remain open at this update. Close the applicable entries with source, passing/failing results and the required production approvals before advancing. Recheck material production changes; reuse unchanged evidence.

| ID / rows | Required closure | Responsible handoff |
| --- | --- | --- |
| B1 · K01/K15 | Retain accepted local/source results; current scope is PREPARE ONLY API+web targets/B8 EG. Bind compatibility, receiver floors, schema/role/capacity, future fresh retry approval and actual required checkpoints. Old APIg200 is archived, EGg74 only a recovery prefill. Retry/B10 queue/deploy inputs remain unset until authorized and observed. | developer-13, verifier02, architect04, release engineer |
| B2 · K22/K24 | Use the selected #2043 controller from develop9be345523 or later containing the fix; require S1 projection dry-run PASS, actual release binding and fresh grants. Read-only observer approval is granted. [Kubernetes mapping and owner probe CLI are supplied](s1-observation.md); the earlier Compute403 still requires a working authorized identity/resource binding. Bind actual N1 pre-arm window/clock and independently associate the response with replacement service. | lead + deployment-engineer-01 + prod-verifier-02 |
| B3 · K22/K24 | Actual CMA/GKE/NEG no-overlap/restoration proof <120s, concurrent-writer/restart/interrupted-cutover cases; separately authorized recovery from failed start or unhealthy owners. Kind1.34 pause containers prove only their controller observations. | deployment-engineer-01 + local verifier |
| B4 · K13 | Secure early EG drain: authenticated mechanism, helper credential isolation, bounded success/failure, socket close before exact-token release, independent TERM handling. Native sleep30 alone does not close this gate. | developer-15/08 + developer-16 + architect-04 |
| B5 · K13/K14/K25 | Bind qualified handoff p95≤90s/P120, distinct hard G and actual/template grace760s separately to actual post-B10 API and retained EG lifetimes. No pooled attempt/rollout timing. Combined client/node/provider capacity and≥30% Mongo headroom remain required; quota64/128 needs its S3 grant/readback. Grace760 alone proves no lifetime bound. | debugger-03 + developer-16/17 + lead |
| B6 · K02–K08/K16–K21/K23/K25 | Target-only prestage is unusable for binding. After authorized retry, consume actual whole-cohort observations and reanchor; no new N1 baseline/review/test wait. Bind log01 coverage, ledger check and failure path without extending old collector intervals. Actual traffic/error/READ results, low-N policy, cumulative errors and E+15 remain required. Preparation is not comparison PASS. | engineer02 + log-engineer-01 + production verifiers + analyst02 + engineer01 |

Before each stage the reviewer records closures or reports `BLOCKED: URGENT — Bn, missing result or binding, owner`; continue independent preparation. The00:00 freeze fixes checklist criteria; it is not a write blackout and does not prohibit the lead-selected00:30 S2 under its fresh grant. The actual no-write window is02:00–02:59, with the controller's01:58 margin. Final source, passing/failing tests and a brief stage result supply closure; do not turn historical FAILs into PASS.

## Reviewed source facts

Six manifests at #1968 merge `d38966aab5c77fa2262da2d14ffba0da84cd958c` supply identities, HPA policy and PDBs. **#1996 merge `37f2bedbfce3be4fe5b8bd1370ba2c81cd872be5` supersedes its EG exec hook** with native `preStop.sleep.seconds:30`, keeping grace760 and memory request=limit2Gi. EG Pod QoS is **Burstable**, because CPU request250m differs from limit2; equal memory values do not establish Guaranteed QoS. At inspected integration `15306eed8aacb67964f95442c1f29ba47d161502`, the other five manifests are identical to d389; EG differs by the documented hook/comment correction. Final release must preserve this or a separately reviewed replacement.

Engineer [server admission results at c07984cd0](https://github.com/ChronoAIProject/cma/blob/c07984cd0c1d166f4b5fdd85b708161ade3ab333/docs/notes/reviews/2026-11-15/server-dry-run-20261114.md) report six strict server dry-runs and two Pod-template checks PASS at18:04–18:07 SGT on14November, using Batch8 images. All six diffs returned1 (changes), with no errors; no resources persisted. This does not qualify Batch9 runtime, scheduling or cutover. Full historical EG apply adds explicit `CMAEG_OWNER_KEY_CUTOVER_ID` / `CMAEG_OWNER_KEY_SERVICE_ID` absent from live env. Use guarded approved-field patches, preserving live env/envFrom and unrelated configuration; admission success does not authorize full-manifest apply.

Both PDBs select exactly `app.kubernetes.io/name` equal to their Deployment name, with `minAvailable:1` and `unhealthyPodEvictionPolicy:AlwaysAllow`. They protect eligible voluntary evictions, **not** Deployment scale-to-zero/deletion, involuntary loss or HPA-driven pod removal. Never combine `minAvailable` with a second `maxUnavailable` field. Rollback to one retains minAvailable1. Both topology rules use revision-aware `matchLabelKeys:[pod-template-hash]`; verify distinct hosts after old pods disappear, not only during overlap.

HPAs target canonical Deployments, min2/max5; CPU AM60% of4CPU and EG200% of250m. Up stabilization0, Max(+2 pods,+100%)/60s. The later lead selection fixes downscale at stabilization300s, Min(−1 Pod)/120s. Qualified handoff p95 must be≤90s; missing/censored/failed timing or p95>90s means HOLD. This supersedes the earlier adaptive-period formula; do not lengthen or shorten cadence silently. Require grace760s in both staged templates and actual Pods, including preStop delay; the reported live Batch8 value735s must be replaced by S1/S2. Grace is not hard lifetime G. No rollout/HPA overlap; use5+ceil(G/120) only after proving G includes deletion delay, hooks and client/socket tails, including failure/zombie behavior.

For each Mongo server: other consumers + Σ(max service processes × Σ(actual client pool cap + monitoring/topology sockets)) ≤0.70×verified server limit. Include jobs/startup/terminating processes; historical AM30+30+4+6 and illustrative EG10 are not final receipts.

The18:04 production snapshot confirms namespace CPU request/limit quotas40/80. With its captured other use, require `4*AM + 0.25*EG + 0.025 <=40` and `8*AM + 2*EG + 0.25 <=80`, plus memory/Pod quotas and node placement. Steady2/2 and5/5 fit; eight of each need80.25 CPU limits and fail. Illustrative12/12 needs51.025 CPU requests/120.25 limits and exceeds the three eligible nodes' entire47.67 CPU; sandbox nodes are tainted and excluded. Passing separate admission calls reserves no combined capacity. Seven each fitting arithmetic is not a qualified lifetime/socket budget. Keep S3 HOLD until qualified G/cadence yields a safe combined envelope or an explicitly approved capacity remedy. Do not silently alter quota, requests, CPU targets or taints.

The lead subsequently selected `s3-quota-raise` for `chrono-bot/ResourceQuota/sandbox-quota`: CPU ceilings40/80→64/128, before either HPA and only with the exact S3 grant. The plan saves complete before-spec; UID/RV/full-spec tests guard two CPU leaf replacements. Preserve memory/Pod quotas and all unrelated fields. Require confirmed64/128 in spec.hard and reconciled status.hard. Engineer's18:19 guarded server dry-run passed; readback remained40/80, so no raise has occurred. After the real raise the CPU inequalities use64/128; illustrative12/12 fits those ceilings but still fails observed node capacity. Recompute actual nodes/memory/socket/provider bounds independently.

`s3-quota-restore` uses the confirmed raise result and its own grant. First remove the exact S3 HPAs and recover both services to fixed2 with no terminating Pods; require reconciled quota status and used CPU≤40/80. Restore saved CPU ceilings only, refusing object replacement or unrelated drift. If preconditions fail, retain raised ceilings while recovering workload. Lowering quota does not evict Pods; it can block replacement admission. Successful S3 retains64/128 unless separately directed.

AM memory was3273.7MiB against256MiB requested (8GiB limit). Refresh actual memory pressure/headroom with final images/load and survivor placement; scheduler request arithmetic alone understates occupancy. EG memory request=limit2Gi and both services' Burstable QoS remain explicit. Fresh metrics were available in this snapshot; that proves neither useful-load calibration nor automatic scaling.

## Current execution commands — selection and qualification still required

The selected S1 controller is #2043, merged at `9be345523fd2768d8e8daf1c0f4ccd487191f9b0`, or a later develop descendant retaining its fix. Require successful current projections before S1; historical `08ce4653d`/`d2a28a360` commands and tests alone do not meet this requirement. Use the current owner S1/S2/S3 runbooks and operation grants; #2038 adds the separately merged/qualified S3b exercise. This documentation update runs no controller or tool tests and grants no execution authority.

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
| S3 quota pre-step | `s3-quota-raise` | Both fixed2, no HPA/terminations, actual/template grace760s; saved before-spec, exact S3 grant and confirmed spec/status64/128. |
| S3 creation | `s3-am-create`, then `s3-eg-create` | Both fixed2, timing/capacity qualified, no rollout, valid metrics and exact final HPA policy. |
| S3 recovery | `s3-am-freeze`, `s3-eg-freeze`; then their `delete`; then one `descend` at a time | Both directions Disabled; exact-created UID/RV deletion using actual creation results; HPA absence and in-flight-write reconciliation; survivor proof and actual exit between decrements. |
| S3 quota recovery | `s3-quota-restore` | Confirmed raise result; fixed2, HPA absent, no terminations, reconciled status and CPU use≤40/80; own grant and no unrelated drift. |

Pause is not a replica lock. Reserve one release/scale writer, preserve canonical Service/Gateway/HTTPRoute/NEG identities, and never hand-create/adopt a ReplicaSet. No shared ConfigMap activation under live legacy processes. Both direct origins use Pod IP (AM8080/EG8090). No Secret reads, pod exec, whole-object exports, generic rollout undo, unverified modeOFF or bulk downscale.

The [agreed observer companion](s1-observation.md) contains approved read commands, actual preparation results and the owner's concrete cutover CLI. Working Compute identity/resource mapping and actual execution/source-association inputs remain missing; no repeat read-only observation approval is needed. EndpointSlice or Pod Ready cannot establish successful user service. Bracket old backend withdrawal → qualified full user-service completion; require a defensible upper bound strictly<120s and record both-ready separately. At120s from the conservative lower bound without restoration, use approved compatible recovery even if a long wait remains. Do not shorten grace/authority or force-delete. Pod disappearance alone does not prove remote authority cessation.

S3 freeze/delete/descent preserves modeON, compatible images, custody and PDBs. Do not patch Deployment replicas while HPA exists. If demand needs>2, retain the safe fixed count and report autoscaling incomplete. A fresh30-minute recovery soak follows. Eight author driver tests are preparation evidence; the proposed8RPS workload remains uncalibrated and needs its distinct load approval. Engineer02 alone owns M4/index operations.

## Soak evidence and verdict rules

Start30 minutes only after intended Ready/Available, backend/NEG, membership and handoff are stable, with prior terminations complete. Reset on image/config/count changes; final S3 soak is after automatic return to2. Per-minute samples plus finer approved failure capture must cover two five-minute background cycles, all process/client/capacity bounds and K02 custody accounting. Metadata-only watchers cannot prove user-service recovery, complete client journeys or absence of committed stale effects.

**S1 low-traffic rule, predeclared by the lead at19:10:** count distinct accepted production write commands during the initial30minutes; retries, refused commands and synthetic reads do not increase N. If N<20 at30minutes, extend the same observation by up to30 additional minutes. Retain the first-window count/errors and the cumulative extended count/errors; never sum overlapping windows or reset the original denominator. If still below20 after extension, accept using the required passing in-code multi-instance suites plus **zero errors**, disclose actual N and do not hold S1 for low traffic alone. Record initial/extended S–E and report by actual E+15. This is a stage-acceptance exception, not a per-route sample count, measured client-boundary PASS or statistical-confidence claim. It never waives observed failures, duplicates/stale effects, missing telemetry or unresolved work; preserve all errors and absolute targets. The exception changes S1 only.

Log-engineer-01 runs its supplied `accepted_command_count` query at the initial closed end and declared extended end, keeping the same start. From its own workspace:

```bash
: "${s1_log_db:?actual safe local analysis database}"
: "${s1_run_id:?actual S1 run identity}"
: "${s1_start_ns:?original S1 start in UTC nanoseconds}"
: "${s1_closed_end_ns:?actual closed observation end in UTC nanoseconds}"
python3 logs/batch9-scale-queries/query.py --db "$s1_log_db" \
  --query accepted_command_count --run-id "$s1_run_id" --stage S1 \
  --start-ns "$s1_start_ns" --end-ns "$s1_closed_end_ns"
```

Use `initial_30m.accepted_command_n` for analyst `accepted_first_30m` and `full_observation.accepted_command_n` for `accepted_total`; `extension_only` is diagnostic, not another scope to add to the full count. The count is distinct canonical durable real-production command identity at original admission, excluding replay/fanout, synthetic traffic and pre-S admissions. Missing/conflicting identity, original time, origin, accepting replica or accepted-command ledger coverage makes qualified N null, never0. Keep `count_qualified` and source/ledger/identity coverage. These are local analysis fields, not permission to change the standing collector database. The query supplies a decision input, never stage PASS; independent failure/duplicate/absolute/coverage checks still apply. Owner reports37 SQL tests PASS, including19 initial+1 extension+replays yielding19/1/20.

**Mandatory S1 real-traffic verification — lead answer to #116:** log-engineer-01 owns the read-only log/metric queries over the actual30-minute S–E interval and matched baseline. Bind the deployed source/images/config, Pod identities/restarts and query coverage; retain per-replica create/send/stop/stream counts, success/failure/completion denominators and latency. Report ownership/epoch transitions, fencing refusals and5xx/429 by cause against baseline. A correctly refused stale request is distinct from a committed stale effect; distinguish expected admission limits from service failures. Join logical command identity across attempts/replicas to the effect ledger: **the same logical command executed twice must be0**. Distinguish duplicate delivery/log rows from duplicate execution. A missing ledger join, sampling gap or zero relevant traffic cannot establish zero duplicate effects or successful work. Keep custody accounting disjoint: accepted = terminal + reconciled + unresolved.

There is **no synthetic WRITE executor at S1**, no broker create/send and no NyxID slot change. Observe real auth/runtime/provider delivery and natural ownership transitions without generating write traffic or injecting owner loss. The read harness supplements only **open detail/transcript and usage24h,7d,30d**, each route/range with sufficient N under the existing authorized owner's pacing and abort rules. Keep ranges separate; verify complete bodies, identity and requested range. Log-engineer-01's exact command/output mapping is `/Users/shining/code/work/tech-lead.staff/log-engineer-01/logs/batch9-scale-queries/S1-CHECKLIST.md`: `tools/run_scale_query_set.py` reads a local safe analysis database using actual S1 bindings, while `tools/measure_create_completions.py` reads the retained completion store. Query preparation alone supplies no live coverage or ledger data. In particular, `duplicate_effects` reports distinct authoritative committed effects; if these receipts cannot establish execution identity/coverage, same-command-executed-twice remains UNKNOWN. Log-engineer-01 supplies the actual inputs/results; the read owner binds its routes/ranges without a new scheduler or credentials.

Report observable create≤5s, Send/stop≤1s, stream/terminal bounds and same-work recovery≤30s with their actual boundary definitions. Complete-body read timing is separate from usable open≤1s or coherent usage render≤2s. Logs, metrics and synthetic reads do not manufacture unobserved client completion or fault coverage: missing is NOT_RUN, invalid is INVALID, insufficient/unassessed is INCONCLUSIVE. Those labels remain explicit even if S1 receives the permitted low-N stage acceptance. Retain failures, incomplete work, offered/accepted/refused/failed/completed/reconciled/unresolved counts and per-boundary N. Production results remain separate from local M3 evidence.

Observed accepted-work loss, duplicate/stale effect, authorization or dual-desktop breach, unresolved terminal/effect failure, failed in-contract journey or absolute completion target requires **immediate stop of promotion, halt of owned synthetic reads and approved compatible rollback**. Do not wait for two performance windows. Restrict real admissions only through the approved recovery controls. Use healthy ownership-enabled2→1 when safe; otherwise quiesce via the approved failure branch. Preserve uncertain custody, with no blind resend, generic image undo or switch-off bypass. Log-engineer-01 and the read owner supply results for the E+15 decision; the narrow CRITICAL exception never passes missing work or an absolute failure.

Log-engineer-01's supplied failure path is an immediate `BLOCKED: URGENT` report in its lead-visible pane plus guarded delivery of the same sanitized incident to deployment-engineer-01, copying health-monitor-01. Include detection UTC, actual S/E, source/image/replica, safe work/record identity, observed failure/boundary and coverage; request acknowledgement. Bind this existing path in the actual stage execution. The E+15 report is not the failure-notification mechanism. Log-engineer-01 uses:

```bash
: "${s1_incident:?absolute path to sanitized incident text}"
TL_HOME='/Users/shining/code/work/tech-lead' \
  '/Users/shining/.claude/skills/shining-tech-lead/scripts/team.sh' \
  peer-send deployment-engineer-01 "$s1_incident" --force
TL_HOME='/Users/shining/code/work/tech-lead' \
  '/Users/shining/.claude/skills/shining-tech-lead/scripts/team.sh' \
  peer-send health-monitor-01 "$s1_incident" --force
```

Delivery exit0 is transport success, not executor acknowledgement. Deployment-engineer-01 acknowledges the incident and its promotion/compatible-recovery action or status; the failure remains open until that reply. A rejected/failed delivery or missing acknowledgement immediately escalates through the already-issued urgent lead-pane report and health-monitor copy, with delivery/no-ACK status explicit. Never bypass a directed-grant rejection. Deployment-engineer-01 alone invokes approved compatible rollback; observation ownership grants no mutation authority. Missing coverage remains UNKNOWN, not a fabricated failure or clean result.

## S1 prestaged dispatch

Owner/workspace: prod-verifier-02, `/Users/shining/code/work/tech-lead.staff/prod-verifier-02`; current reference `deadline-20261115/s1-prestage/README.md`. State is `PREPARE_ONLY_TARGETS_NOT_OBSERVED`, with `binding_usable=false` and `runtime_started=false`. Targets: API digest `322f08a15db3026560193e22eae6c590549366c2758edae2dff9b22be8ffa9e8` and web digest `ddd7ec5a241b0b2d1ea4e43ee10573ef963bf2f4517c14ae21075867b4122f1f`, source `67b3fbc5df5f09fcecd9ddbbba723ae010df9cc6`; retain B8 EG digest `f8f502aed8b5ea2c68924994c98f988c483dcf689d65998f4d94aeabdf696330`. EGg74 is a recovery prefill with Git source unset. Future API/web generations, API checkpoint, whole-cohort observed UTC, runtime windows and grants remain null.

The former APIg200/EG73 inputs are archived mode0600 in `s1-prestage/history/attempt2-g200/`; their checkpoint PASS remains preincident history. Owner-retained rollback completed at14Nov14:17:27.617690UTC; snapshot14:19:05.305747UTC records restored B8 APIg201/EGg74/webg224, eachReady1, nondeleting Pods, containerrestart0 and no outgoing B9 candidate. `manual_repair=false`; failed-forward exit1 and outgoing EG OOM/restart1 remain retained. Recovery readback proves neither service/performance acceptance nor the underlying memory-growth cause.

Before future binding, owners must consume the actually approved retry's source/images/generations/checkpoint and current whole-cohort observation, reanchor API fields, and bind retained EG config/lifetime, web generation, actual topology/controls and clock/window. Historical maintenance/collector intervals never extend automatically to new Pods. The following commands are retained interfaces only: **do not invoke the binder or runtime commands during PREPARE ONLY**. After owner reanchoring and the applicable lead call, the unchanged offline binder validates complete inputs and prints a new mode0600 session path:

```bash
: "${s1_cutover_input:?actual owner cutover-input JSON}"
node deadline-20261115/s1-prestage/bind.mjs --cutover "$s1_cutover_input"
# Use the exact path printed by the binder, then validate before authorized arming.
: "${s1_cutover_session:?exact printed validated session path}"
node deadline-20261115/cutover-capture.mjs --validate "$s1_cutover_session"
node deadline-20261115/cutover-capture.mjs --arm "$s1_cutover_session"
```

Arming is an execution action within the owner's actual window, not part of offline preparation. `RUN_ID` comes only from `operator_trigger_ready`; engineer01/architect03 supplies the actual withdrawal bracket under the existing observer contract:

```bash
: "${s1_cutover_run_id:?RUN_ID from operator_trigger_ready}"
: "${s1_withdrawal:?actual observer withdrawal JSON}"
node deadline-20261115/cutover-capture.mjs --trigger "$s1_cutover_run_id" "$s1_withdrawal"
```

No full30-minute N1 read run is added before future arming. Preserve the shared lock and retained90s clock; do not remove another live lock. Missing baseline remains explicit. A target image or archived generation is not a runtime observation; the probe retains `replacement_source_qualified=false` until the external observer joins actual serving evidence. Owner reports4/4 binder checks using isolated historical fixtures, unchanged21 runner checks, and no bound session/lock/control directory. These results do not make the target-only preparation usable. No binder, arming, trigger or sample was executed here.

## S1 read sampler

Only after an approved retry, owner reanchoring and stable S1 serving does prod-verifier02 fill `s1-prestage/read-input.example.json` with observed post-S1 API generation, actual AM2/EG1/controls, current EG/web, whole-cohort observed UTC and read start. Never derive a generation from old200 or repeated images. The session derives exactly1800s/120slots/all3ranges. The former21:30–22:00 N1 proposal remains historical. `release.source_sha` identifies API only; retained EG Git source remains unknown until independently supplied. The following interface remains uncalled during PREPARE ONLY:

```bash
: "${s1_read_input:?actual owner post-S1 read-input JSON}"
node deadline-20261115/s1-prestage/bind.mjs --read "$s1_read_input"
: "${s1_read_session:?exact validated session path printed by binder}"
node deadline-20261115/read-capture.mjs --validate "$s1_read_session"
# Execution owner only, at the bound read window; not a second collector.
node deadline-20261115/read-capture.mjs --run "$s1_read_session"
```

The sampler schedules120slots at15s intervals. Each round reads the concurrent existing-Agent detail/transcript pair, then serialized view+coherent firstpage for each of24h/7d/30d, rotating range order. At most8GETs/round and960scheduled GETs plus bounded fixture discovery; target100completed observations/boundary, cap120, with actual N reported. Slow/discovery rounds skip slots without catch-up. The existing buffered lane,1s ordinary pacing and shared lock remain. Use `refresh=false`, no explicit step, polling, retry or prewarming. Nonready/202,429/5xx, identity/coherence/broker failure or hard cutoff stops subsequent requests and retains prior valid range results. Only the owner's emitted PID may receive SIGINT/SIGTERM for cleanup. The real-traffic extension never extends this hard cutoff, doubles its cap or automatically starts another sampler.

Output is `deadline-20261115/results/<UTC>/capture.json`, schema `cma-m8-read-capture-v3`. Use `samples[].usage[range]` and `usage_summary[range][metric]` for separate range outcomes, N/p50/p95, freshness and report identity. Legacy top-level usage fields and v2sessions are24h-only; never pool them with7d/30d. Report missed/failed/incomplete/unissued slots and natural-versus-synthetic attribution; unknown attribution remains explicit. API wall includes broker waits; no transport-time subtraction or client-render claim. Owner reports21/21 local tests PASS, including actual CLI with a local fake broker and no credentials/network; this checklist does not rerun those tools.

## Latency acceptance and reporting

K16–K19: usable plain/desktop create ≤5s, full open ≤1s, shown durable send ≤1s, delivered/UI-acknowledged interrupt ≤1s, rendered usage summary/page ≤2s. Activity API p95 <1s; Activity page completion is measured separately with no invented absolute page ceiling. K20: request p95≤500ms/p99≤2s, stream-header p99≤2s, terminal p99≤5s/every terminal≤30s; EG checkout p99≤50ms, ordinary event≤50 Mongo operations and ≤10% same-fixture cost regression. Count failed, refused, timed-out and incomplete journeys separately; missing terminal fails.

K21 keeps statistical and acceptance verdicts separate. CRITICAL uses measured noise; UX p95≤+10% (aim≤5%); detached NON-SENSITIVE p95≤+30ms, with the existing confidence methods. No added noise allowance, result-driven reclassification or borrowed local noise. User-waited background/relay work inherits the critical boundary. Keep every catalog membership and route/mode separate; p99 needs≥1000/stratum.

**Later lead ruling, S1/S2/S3 only:** a valid observed matched CRITICAL statistical INCONCLUSIVE may receive policy PASS when the signal is explicitly assessed as no adverse signal and all applicable absolute checks pass. Statistical INCONCLUSIVE remains visible. Beyond-noise signal, failed/incomplete observations or absolute failure means FAIL. Missing observations are NOT_RUN; invalid data INVALID; unassessed context/signal INCONCLUSIVE. No exception applies to UX/background, missing client boundaries, empty denominators or M3 local gates.

**S2/S3 keep30-minute soaks; S1 alone uses the explicit low-N extension above. Report by actual E+15minutes.** Earlier requests for longer statistical soaks remain closed. Three baseline/post windows remain requirements of the existing statistical method, not an independent reason to block the expressly permitted CRITICAL policy PASS. Do not change the estimator, expand measured noise or relabel statistical results. Record stable start S, initial E=S+30min and any authorized S1 extension; changes restart stability, with all earlier failures retained.

The earlier fixed HTTP windows are superseded. S1/S2 bind to the actual approved retry/retained EG cohort; S3 uses actual post-B10 API/retained EG process lifetimes and selected before/after windows. Keep failed-attempt, recovery, retry, B10 and replica/range results separate. Prod-verifier02 binds each1800s read session to actual source/config/state; no automatic extra sampler. Retain cumulative errors and original accepted-command scopes across backfill strata; image/config/count changes restart stability with failures retained. HTTP body timing is separate from rendering. The retained03:00 B10 plan needs its own grant and observed success before Activity→U1→U2.

Immediate ABORT for stale/wrong-owner committed effect, duplicate external action, lost accepted custody/terminal/receipt, dual desktop/bot authority, auth bypass, epoch/cursor regression or incompatible receiver. Stop promotion/admission using approved controls; preserve custody; if rollback risks duplicates, remain safely quiesced with compatible fences and escalate.

Availability/capacity ABORT: zero healthy canonical backends outside the approved cutover; no restored service at120s within it; same-work recovery>30s; OOM/repeated crash/permanent renewal failure; process/client/provider cap breach or Mongo headroom<30%. Widespread timeouts/5xx and S1 work/absolute failures do not wait for performance windows. Other supported performance failures across two consecutive five-minute windows trigger recovery; a failing whole-soak comparison fails the stage. Missing/unmatched baseline leaves that comparison unqualified without adding a new N1 sampling gate; PREPARE ONLY still supplies no startup permission. Unknown safety/effects or unbound runtime inputs remain explicit and cannot be marked PASS. Retain the narrow K21/low-N policies without hiding actual failures.

Each stage result records source/controller commit, images/configuration, exact operation/approval outcome, UTC/SGT interval and both outage/ready clocks, separate statistical/policy verdicts and K-row results with denominators, route confidence, peak process/socket/headroom, PDB/HPA/backend history and actual recovery. Engineer attests observations; lead grants each next stage only after prior success. Failure invokes approved rollback and immediate report for lead re-slotting. S2 records provider reconciliation and assigned24-hour monitoring. M8 uses prod-verifier-01's kit plus log/read/client evidence09:00–16:00 on15Nov for the18:00 final report.

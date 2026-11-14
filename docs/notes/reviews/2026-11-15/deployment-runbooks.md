# Production scale execution — 15 November 2026

Executor: deployment-engineer-01. GO and advancement: lead. Freeze: **00:00 SGT**.
Stage starts are fixed: [S1 06:00](s1-execution.md), [S2 08:30](s2-execution.md),
[S3 10:30](s3-execution.md). Production verification is 13:00–17:00; final owner
deadline is 18:00. These documents supply commands; they do not grant execution.

Use solution-architect-03's `stage-go-checklists.md` and the canonical 25-row
`/Users/shining/code/work/tech-lead.staff/solution-architect-04/essentials.md`.
The lead speed directive replaces intermediate hash/review chains with source,
tests and results. The later lead ruling keeps **30-minute soaks** and reporting
by actual end **E+15 minutes**; see `performance-analyst-02/cma-scale/hard-deadline/lead-soak-ruling.md`.
It supersedes the older checklist's single-postwindow acceptance restriction,
without changing statistical verdicts or absolute targets.

## Execution inputs and explicit holds

Before stopping either legacy process, obtain the final integrated source,
local M3 N1/N2/2→4→2/kill results, actual AM/EG image digests, Batch9 N1/OFF
deployment and stable-soak result, schema/retained-receiver compatibility and
Activity postcheck. Engineer02 is the sole Batch9/index operator. Its sequence
remains after03:00: Batch9 plus30-minute soak, then separately approved indexes.
This runbook neither creates an index route nor supplies database credentials.

The lead must select this controller implementation and the RS0-after-old-stop
variant. Qualification must demonstrate actual CMA/GKE/NEG no-overlap and
service restoration **strictly below120 seconds**, including command and
approval latency, controller restart, concurrent writer and interrupted cutover.
Offline tests do not establish this. Reserve one release writer across both
Deployments/HPAs; Kubernetes CAS protects individual writes, not cross-object
transactions. No other controller/operator may scale or roll out concurrently.

Name the authorized canonical backend/NEG observer and user-service probe owner,
their exact approved invocation and old/new backend mapping before GO. These
inputs are not present in the preparation source. EndpointSlice/Pod Ready alone
cannot fill them. The prepared controller and runbooks remain **execution HOLD**
until these runtime inputs and the affected essentials pass. Do not turn input
booleans into qualification evidence.

Bind compatible modeON recovery images before cutover, survivor capacity before
downscale, and K13 secure EG drain before S2. Native EG sleep30 is propagation
only. S3 additionally needs complete-cohort handoff p95, a distinct hard
process/socket/client lifetime, all-consumer capacity and calibrated useful load.
No fallback to old modeOFF writers or historical EG shell hooks is supported.

## Operator setup and one-operation transaction

Use a checkout at the selected controller commit. Copy the example locally and
fill only from qualified inputs; its nulls intentionally fail validation. Do not
put credentials or private user/Agent/Workspace identities in this file.

```bash
set -euo pipefail
umask 077
scale_repo=/Users/shining/code/work/.tl-worktrees/cma-cmaeg-release
scale_tools="$scale_repo/.github/scripts/cma_scale"
: "${scale_work:?new private operation directory outside tracked source}"
mkdir -p "$scale_work"
scale_release="$scale_work/release.json"
test -e "$scale_release" || cp "$scale_tools/release.example.json" "$scale_release"
scale_context=gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke
k() { kubectl --context "$scale_context" --request-timeout=15s -n chrono-bot "$@"; }
plan_scale() {
  local action="$1" label="$2"
  shift 2
  python3 -B "$scale_tools/controller.py" plan --action "$action" \
    --release "$scale_release" --out "$scale_work/$label.plan.json" "$@"
}
apply_scale() {
  python3 -B "$scale_tools/controller.py" apply \
    --plan "$scale_work/$1.plan.json" --approval "$2"
}
TL_HOME=/Users/shining/code/work/tech-lead \
  /Users/shining/.claude/skills/shining-tech-lead/scripts/team.sh \
  policy show ChronoAIProject/cma
python3 -B "$scale_tools/controller.py" --help
```

Images must use actual single-image GAR references:
`us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api@sha256:…`
and `…/cma-cmaeg@sha256:…`. Source is the full40-character final application SHA.
Recovery images need independently qualified retained-mode compatibility.
`survivor_capacity.<role>` supplies actual UTC `observed_at`,
`minimum_safe_replicas` and `qualified:true`; its observation must be ≤120s old
at both plan and apply. HPA values are measured, never copied from test examples.

Every listed `plan_scale` performs fresh reads and **server dry-run**. It prints
the exact single-use approval context. The lead supplies a grant for that context;
only then run the corresponding `apply_scale`. No loop runs the sequence.
The controller rechecks status, object identity/generation/full spec, policy and
blackout after approval; writes use fresh UID/RV/generation/spec CAS. Create uses
strict validation and name conflict protection; HPA delete uses UID/RV conditions.
Unexpected admission/defaulting changes are HOLD, not automatically accepted.

Each apply consumes `team.sh policy-check deploy --approval … --context …` and
requires `ALLOWED once via reviewed-release-controller`. An old image-only grant
does not authorize these changes. Plans bind the concrete operation and selected
controller code; `.result.json` records one attempt/readback, not a seal chain.
Never edit a plan or reuse a completed/uncertain plan. New state requires a new
plan, fresh dry-run and fresh exact grant. Do not make files writable to bypass
an existing attempt marker. All writes, including recovery, refuse01:58–02:59
SGT; the extra two minutes are command margin. Reserve full drain/recovery time.

## Readback, service clock and source updates

Run one bounded observation at each checkpoint; existing health/log owners keep
their current monitors. These commands do not start another watcher:

```bash
k get deployment cma-agent-manager cma-event-gateway \
  -o 'custom-columns=NAME:.metadata.name,GEN:.metadata.generation,OBSERVED:.status.observedGeneration,DESIRED:.spec.replicas,READY:.status.readyReplicas,AVAILABLE:.status.availableReplicas'
k get pdb cma-agent-manager cma-event-gateway --ignore-not-found \
  -o 'jsonpath={range .items[*]}{.metadata.name}{"\n"}{.spec}{"\n"}{.status}{"\n"}{end}'
k get hpa cma-agent-manager cma-event-gateway --ignore-not-found \
  -o 'jsonpath={range .items[*]}{.metadata.name}{"\n"}{.spec}{"\n"}{.status}{"\n"}{end}'
```

`observe.py check` validates actual Deployment/ReplicaSet ownership, configured
and running digest, all selected/owned Pods, observed generation and distinct
hosts for the fixed pair. Larger cohorts use the qualified placement/capacity
plan. At zero it rejects remaining or terminating owned Pods/RSs.
`--paused-template` is only for the original serving process under an inactive
template; it does not claim the running process uses that inactive image.
Each read may take time. Never use a long wait to extend the outage allowance.

The independent observer starts the outage clock at **last old healthy backend
withdrawal**, and ends it at successful user service on a qualified replacement.
Record both-ready and application/socket authority cessation separately. The
following command calculates a supplied observation; it does not probe service:

```bash
: "${scale_withdrawn_utc:?actual observer UTC timestamp}"
python3 -B "$scale_tools/observe.py" outage --withdrawn "$scale_withdrawn_utc"
# Run after the observer proves successful replacement user service:
: "${scale_restored_utc:?actual successful service UTC timestamp}"
python3 -B "$scale_tools/observe.py" outage --withdrawn "$scale_withdrawn_utc" \
  --restored "$scale_restored_utc" > "$scale_work/outage-result.json"
```

At120s without restoration, abandon promotion and invoke the approved recovery
branch; report the breach even if recovery later succeeds. Do not shorten grace,
clear claims, force-delete Pods, or start stale legacy writers to beat the clock.
Pod absence does not prove all remote sockets or durable authority ended.

At the real start and each apply/convergence/change/end, produce a fresh private
snapshot. Set `scale_previous_packet` to the actual Batch9/prior-stage/latest
packet; preserve all serving, replaced, restarted and terminating identities.

```bash
: "${scale_stage:?S1 S2 or S3}"
: "${scale_run_id:?actual run identifier}"
: "${scale_stage_start_utc:?actual UTC start}"
: "${scale_previous_packet:?actual prior packet}"
: "${scale_packet:?new unique local JSON path}"
python3 -B "$scale_tools/observe.py" snapshot --release "$scale_release" \
  --stage "$scale_stage" --run-id "$scale_run_id" --start "$scale_stage_start_utc" \
  --history "$scale_previous_packet" --out "$scale_packet"
```

A missing/restarted old source is retained as retired with the **first observed
absence upper bound**, not an invented exact exit time. Reappearing retired
identities fail. Pending unknown runtime identity stops this snapshot and leaves
the prior packet intact; refresh once discovered. The established health reader
retains retired-source coverage and detects retired sources still serving.
After checking the new local packet, publish it to the existing reader and bind:

```bash
scale_queries=/Users/shining/code/work/tech-lead.staff/log-engineer-01/logs/batch9-scale-queries
: "${scale_packet_name:?new unique filename ending json}"
test ! -e "$scale_queries/packets/$scale_packet_name"
install -m 600 "$scale_packet" "$scale_queries/packets/$scale_packet_name"
python3 -B /Users/shining/code/work/tech-lead.staff/log-engineer-01/tools/bind_scale_queries.py \
  --packet "$scale_queries/packets/$scale_packet_name"
```

## Every stage's soak, acceptance and abort

Start S only after intended Pods, real backends, membership and handoffs are
stable and all previous terminations complete. Declare E=S+30min in advance.
Image/config/count changes restart stability; retain full-stage failures.
S3's S is after actual automatic return to2. Existing monitors supply per-minute
capacity/ownership/custody and two five-minute background cycles. The authorized
NyxID owner runs existing detail/transcript and retained24h usage reads during
S–E, against the matched final-N1 profile; no Activity/create/send/stop/refresh
mutation is added. Body timing does not prove browser rendering or user journeys.

Before HTTP windows in SGT: S1 05:28–05:58, S2 07:58–08:28, S3 09:58–10:28.
After is actual S–E. Analyst owns capture and the exact query inventory; reuse
existing coverage instead of starting an overlapping collector. Optional31min
direct metrics ends about E+1 and does not change E. Report by E+15.

Analyst's exact reduction after its captures exist (local, no production writes):

```bash
scale_analysis=/Users/shining/code/work/tech-lead.staff/performance-analyst-02/cma-scale
: "${scale_before_http:?qualified before http/window.json}"
: "${scale_after_http:?actual soak http/window.json}"
: "${scale_before_raw:?actual raw schema2 directory}"
: "${scale_after_raw:?actual raw schema2 directory}"
: "${scale_stage_evidence:?actual matched source image context and client assessment}"
python3 -B "$scale_analysis/compare_baseline.py" \
  --baseline "$scale_before_http" --post "$scale_after_http" \
  --baseline-live "$scale_before_raw" --post-live "$scale_after_raw" \
  --stage-evidence "$scale_stage_evidence" --output "$scale_work/comparison.json"
```

Keep statistical and policy verdicts separate. Under the later lead ruling,
CRITICAL statistical INCONCLUSIVE may receive acceptance PASS only with valid
observed matched comparisons, explicitly assessed no adverse signal and satisfied
applicable absolute checks. Failures/incomplete observations, adverse signal
beyond noise or absolute failure mean FAIL. Missing=NOT_RUN, invalid=INVALID,
unassessed=INCONCLUSIVE. UX retains≤10%p95/aim5%; detached non-sensitive≤30ms;
their existing confidence methods remain. No noise addition or local-noise reuse.

Retain all essential route/mode boundaries: usable plain/desktop create≤5s,
open/send/interrupt≤1s, rendered usage≤2s, Activity API p95<1s and separate page
completion. No invented Activity page ceiling. K20 request p95≤500ms/p99≤2s,
stream headers p99≤2s, terminal p99≤5s/every terminal≤30s, checkout p99≤50ms,
ordinary event≤50 Mongo ops/≤10% cost regression also apply. Missing client
measurements cannot be replaced by HTTP headers or Pod Ready.

Immediate abort: stale/wrong/dual-owner committed effect, duplicate external
action, lost accepted custody/receipt/terminal, auth bypass, epoch/cursor
regression or incompatible receiver. Availability/capacity abort: zero healthy
backend outside approved cutover, outage≥120s, same-work recovery>30s,
OOM/repeated crash/permanent renewal failure, process/client/provider bound
breach or Mongo headroom<30%. Do not wait for a full statistical window.
Performance-only supported failure in two consecutive5min windows or a failing
whole-soak comparison triggers recovery. Unknown outcomes retain original keys
and custody; never blindly resend. If safe takeover is impossible, remain
quiesced with compatible fences and escalate.

## Uncertain operation and final report

Timeout, lost response, interruption after WRITE_ATTEMPTED or readback mismatch
is UNKNOWN. Stop subsequent operations, read the named object through the above
safe projections and compare against the plan's intended fields in memory.
Have the lead choose reconciliation/recovery; a new approval is not permission
to repeat an ambiguous mutation. CAS conflict is HOLD, not permission to silently
regenerate around another writer. PDBs remain during recovery; they do not stop
Deployment scale-to-zero, involuntary loss or HPA Pod removal.

Report source/controller commit, image refs, actual operation outcomes and
S/E/outage clocks, essentials PASS/FAIL/NOT_RUN/INCONCLUSIVE, per-route counts,
failed/incomplete outcomes, peak processes/clients and headroom, HPA transitions,
recovery and next-stage lead decision. No intermediate seals/hash inventories.

Local verification of this implementation:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s .github/scripts/cma_scale -p 'test*.py' -v
```

All tests default-deny subprocess/Kubernetes calls. They exercise approval/CAS,
status drift, unknown-write replay, blackout, native EG sleep, capacity/cadence,
recovery ownership, Pod placement/image identity and strict outage arithmetic.
They do not qualify the actual cutover, image behavior, controller integration,
secure early EG drain, timing/capacity or production service.

## Production dry-run findings — 14 November

Read [the six-manifest server dry-run and capacity results](server-dry-run-20261114.md)
before stage planning. All six admission checks and diffs completed; two Pod
admission checks also passed. This does not qualify runtime execution. Use the
guarded stage patches, preserving live env/envFrom and unrelated configuration:
the historical EG full-manifest apply would add two owner-key settings absent
from the live explicit env list.
Current namespace quotas cover steady AM2/EG2 and AM5/EG5, but eight processes
of each already exceed CPU-limit quota; the illustrative G760/P120 envelope
of twelve each also exceeds CPU-request quota and current eligible CPU. Refresh
quota, eligible-node placement, actual memory and all terminating processes
before each stage. Do not count tainted sandbox nodes or infer autoscaler capacity.

The subsequent lead ruling selects an [S3 CPU-quota pre-step](s3-execution.md):
requests 40→64 and limits 80→128, with a saved before-spec, server dry-run, exact
S3 approval, two guarded field replacements and a usage-checked rollback. This
is planned only; it does not revise the historical dry-run snapshot or add node
capacity. HPA downscale is fixed at one Pod per 120s; actual/template grace must
read back 760s before S3. All stage Deployment changes retain the existing
guarded patches; never apply the historical full Deployment manifests.

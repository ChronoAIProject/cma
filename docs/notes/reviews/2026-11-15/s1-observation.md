# S1 read-only backend and restoration observation

Observer: solution-architect-03; cutover executor: deployment-engineer-01; user-service probe owner: prod-verifier-02. The lead approved these read-only Kubernetes/Compute observations on14November. Engineer01 agreed the scope and event boundaries. No rollout, scale, IAM, credential, Secret, pod-exec or probe-traffic changes are included.

**Kubernetes mapping observed; owner probe CLI supplied; execution remains HOLD.** Compute mapping/access, actual probe window/clock inputs and the replacement-source association are still required. Read-only observation permission is already granted; no repeat generic approval is needed.

## Actual preparation results

| Item | Observed14November; refresh immediately before S1 |
| --- | --- |
| Context | `gke_example-project-02_asia-northeast1_example-cluster-02` |
| Namespace/canonical Service | `chrono-bot` / `cma-agent-manager` |
| Selector/ports | `app.kubernetes.io/name=cma-agent-manager`; Service80 → EndpointSlice8080 |
| Canonical NEG annotation | `k8s1-002d2683-chrono-bot-cma-agent-manager-80-620084a5` |
| Annotated zones | `asia-northeast1-a`, `asia-northeast1-b`, `asia-northeast1-c` |
| EndpointSlice | One ready/serving/nonterminating endpoint in zone C at preparation; refresh old/new identities after the observed Batch9 checkpoint PASS. |
| Compatibility alias | `cma-api` selects the workload but has no NEG annotation; never use it as canonical external-health evidence. |
| Ingress | No Ingress objects in `chrono-bot`; repository HTTPRoute targets the canonical Service through the shared Gateway. |
| Compute result | All three NEG reads using `--project=example-project-02` returned403, missing `compute.networkEndpointGroups.get`, under inherited gcloud authentication. Failed commands also printed `[]`: that is UNKNOWN, not an empty NEG. |
| Project/backend/scope | Unverified. Context names do not establish Compute resource project; GAR identifies artifacts only. Engineer01/02 found no successful retained mapping or Compute principal. |

Before GO, supply actual project, backend-service name, global/regional scope and successful complete reads below. Match returned backend group URLs to the canonical NEG/project/zone. Join endpoint address+port+zone to EndpointSlice target Pod UID and the executor's existing UID/restart/image/ReplicaSet qualification. EndpointSlice does not expose restart count/image or prove authority cessation. Preserve old/new cohorts; IP reuse alone is insufficient. No additional pod or Secret query is introduced here.

## Bounded command capture

Capture each query's UTC and monotonic start/end, exit status, stdout/stderr and completeness in a new private directory. This local wrapper runs one command, with no retry or watcher:

```bash
set -euo pipefail
umask 077
: "${scale_observation_dir:?new private local observation directory}"
mkdir -p "$scale_observation_dir"
observe_read() {
  python3 - "$scale_observation_dir" "$@" <<'PY'
import datetime as dt, json, pathlib, subprocess, sys, time
root, label, *command = sys.argv[1:]
if not label or pathlib.Path(label).name != label or not command:
    raise SystemExit('one safe label and command required')
def utc():
    return dt.datetime.now(dt.timezone.utc).isoformat()
def decoded(value):
    return value.decode(errors='replace') if isinstance(value, bytes) else value
with (pathlib.Path(root) / (label + '.json')).open('x') as output:
    record = {'command': command, 'start_utc': utc(), 'start_monotonic_ns': time.monotonic_ns()}
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=10, check=False)
        record.update(exit_code=result.returncode, stdout=result.stdout, stderr=result.stderr,
                      complete=result.returncode == 0)
    except subprocess.TimeoutExpired as error:
        record.update(exit_code=124, stdout=decoded(error.stdout), stderr=decoded(error.stderr),
                      complete=False, error='timeout')
    except OSError as error:
        record.update(exit_code=127, complete=False, error=str(error))
    record.update(end_utc=utc(), end_monotonic_ns=time.monotonic_ns())
    json.dump(record, output, indent=2)
print(f'{label}: exit={record["exit_code"]}, complete={record["complete"]}')
raise SystemExit(record['exit_code'])
PY
}
scale_context=gke_example-project-02_asia-northeast1_example-cluster-02
scale_k=(kubectl --context "$scale_context" --request-timeout=8s -n chrono-bot)
: "${scale_sample:?unique checkpoint and sample label}"
observe_read "$scale_sample-service" "${scale_k[@]}" get service cma-agent-manager \
  -o 'jsonpath={.metadata.name}{"\n"}{.spec.selector}{"\n"}{.spec.ports}{"\n"}{.metadata.annotations.cloud\.google\.com/neg-status}{"\n"}'
observe_read "$scale_sample-slices" "${scale_k[@]}" get endpointslices \
  -l kubernetes.io/service-name=cma-agent-manager \
  -o 'jsonpath={range .items[*]}{.metadata.name}{"\n"}{.ports}{"\n"}{.endpoints}{"\n"}{end}'
```

For discrepancies, the same wrapper may run `get endpoints cma-agent-manager`, `describe service cma-agent-manager`, or `get ingress` with this context/namespace. EndpointSlice is normal. Nonzero/timeout/partial/invalid results are UNKNOWN; retain them and stop the checkpoint. A successful empty result still needs the correct resource/scope before it proves absence. Use unique labels; never overwrite prior observations.

## Compute commands — actual bindings required

Run through the authorized existing observer's authentication. Variables are verified inputs, not permission to guess names, switch global accounts, read credentials or change IAM.

```bash
: "${scale_compute_project:?verified backend resource project}"
: "${scale_backend_service:?verified canonical backend-service name}"
: "${scale_backend_scope:?global or regional from actual mapping}"
case "$scale_backend_scope" in
  global) scale_scope_args=(--global) ;;
  regional)
    : "${scale_backend_region:?verified backend-service region}"
    scale_scope_args=(--region "$scale_backend_region") ;;
  *) printf '%s\n' 'Unknown backend scope: HOLD' >&2; exit 1 ;;
esac
scale_neg=k8s1-002d2683-chrono-bot-cma-agent-manager-80-620084a5
# Preflight/changed cohort: inspect every annotated zone; this is a read loop.
for scale_zone in asia-northeast1-a asia-northeast1-b asia-northeast1-c; do
  observe_read "$scale_sample-neg-$scale_zone" gcloud compute network-endpoint-groups \
    list-network-endpoints "$scale_neg" --project "$scale_compute_project" \
    --zone "$scale_zone" --format=json --quiet
done
observe_read "$scale_sample-backend" gcloud compute backend-services get-health \
  "$scale_backend_service" --project "$scale_compute_project" \
  "${scale_scope_args[@]}" --format=json --quiet
```

Validate all expected groups/zones/endpoints and health states against the mapping. NEG membership is not HEALTHY; HEALTHY is not successful user service. Unknown/unmapped endpoints, changed Service/NEG/backend mapping or unbounded stale coverage stop acceptance. Refresh dynamic values from the actual approved mapping; do not retain these preparation identities if the real objects change.

## Events and strict120-second bound

Before S1 step3, refresh the outgoing cohort after the observed Batch9 checkpoint PASS, prove canonical health and arm the independent observer/recovery operator. Agree a bounded sampling cadence and measured query latency that fit the outage budget. The10-second command limit is a maximum wait, not a measured interval or an extension of120s. No slow/missed query is continuous coverage.

Keep four events distinct:

1. **Old backend withdrawal:** retain the last complete health sample showing an old endpoint healthy and first complete sample showing no old endpoint healthy, with query start/end and freshness uncertainty. First-absent poll time is not the exact withdrawal timestamp.
2. **Old effect/socket authority cessation:** executor/authority owner's separate proof; endpoint/Pod/NEG absence cannot supply it.
3. **Replacement user service:** full decoded/predicate-complete result from the bound probe below, attributable to qualified replacement service. Both request starts follow the qualified old-withdrawal bound. Preserve completion and local observer receipt times plus clock uncertainty.
4. **Both-ready:** both qualified replacement Pod identities on distinct hosts, canonical backend health, current generation and membership. This does not end the user-service outage clock.

Let `D_low` be a defensible earliest withdrawal bound including query/freshness uncertainty, and `R_high` the conservative full user-service completion bound. PASS requires **`R_high - D_low <120s`**. Same-observer monotonic receipt time may give a conservative restoration upper bound; never subtract different hosts' monotonic clocks. Unbounded freshness/clock uncertainty means the bound is unavailable. A nominal duration below120 does not pass if its uncertainty crosses120.

Run the deadline alarm independently of blocking reads. At120s from the conservative withdrawal lower bound without proven restoration, stop promotion and use the already approved compatible recovery. UNKNOWN never resets the clock. Preserve a breach even if recovery later succeeds; never force-delete or shorten authority/grace.

## Owner's concrete cutover probe

Prod-verifier-02 executes from `/Users/shining/code/work/tech-lead.staff/prod-verifier-02`. The full [owner contract](/Users/shining/code/work/tech-lead.staff/prod-verifier-02/deadline-20261115/cutover/README.md) and [session](/Users/shining/code/work/tech-lead.staff/prod-verifier-02/deadline-20261115/cutover/session.example.json) / [withdrawal](/Users/shining/code/work/tech-lead.staff/prod-verifier-02/deadline-20261115/cutover/withdrawal.example.json) templates define the inputs. Use new private completed inputs; unset templates deny execution. Existing soak/ paced profiles, older Activity/legacy-usage rounds and health diagnostics are not substitutes.

Bind **actually observed Batch9 N1/OFF** source/images/generations/controls, `cohort=n1_repeat`, AM1/EG1 and `observed_at<=arming_start<=window_start`. Do not fill predicted target generations or replicas. The owner confirmed this pre-arm state is supported; replacement identities/config are a separate post-cutover join. Supply actual UTC boundaries and a justified host-clock bound, never zero for unknown uncertainty.

Offline validation only:

```bash
node deadline-20261115/cutover-capture.mjs --validate /absolute/path/to/session.json
```

For the selected authorized execution window, the owner arms once. **Arming is an execution action**: authenticated owner/fixture discovery and the shared collector lock/private IPC are established. It sends no detail/transcript pair before a valid trigger. Take `RUN_ID` only from `operator_trigger_ready` after setup succeeds.

```bash
node deadline-20261115/cutover-capture.mjs --arm /absolute/path/to/session.json
```

Trigger only after the observer establishes the actual withdrawal bracket and the executor has independently established safe authority cessation. This consumes the single attempt before pacing; no automatic retry or new arm to hide failure:

```bash
node deadline-20261115/cutover-capture.mjs --trigger RUN_ID /absolute/path/to/withdrawal.json
```

The schema is `cma-cutover-withdrawal-v1`: `lower_utc`, `upper_utc`, `query_start_utc`, `query_end_utc`, `freshness_bound_ms`, `clock_uncertainty_ms`, `established` and `observer`. Preserve the full query/freshness bracket; there is no one-second bracket cap. Lower must cover query start minus freshness. Host-clock uncertainty is separate, must be known and within the owner's supported0–1000ms, with the additional1ms UTC rounding retained. Never shrink a conservative bracket or replace unknowns with zero. Both request-start lower bounds must exceed the expanded withdrawal upper bound.

The attempt is one detail/transcript GET pair, cap1, no retry/usage/Activity. Keep the shared lock, retained≥90-second sample spacing, ordinary1-second admission spacing, sibling drain and429/5xx/broker/identity aborts. Reserve that sample spacing before stopping legacy; do not reset the shared clock or bypass another collector. Arming starts at most30minutes before the execution window; the window is at most10minutes. Request timeout is1–30seconds, clamped to the remaining window. These limits never extend the120-second cutover budget.

Read the final `deadline-20261115/cutover/results/<UTC>/capture.json`, not trigger acknowledgment. Require both decoded HTTP200 responses, detail/transcript identity matching the pinned Agent, both starts after the expanded withdrawal bound and timely predicate completion. Events retain each `request_start` / `decoded_body_complete`, `pair_settled`, `both_decoded_bodies_complete` when true, `identity_predicate_complete` and `cutover_finished`. Failed/incomplete sibling results remain failures; clock discontinuity/expiry fails closed. No response payloads, raw IDs or credentials are persisted.

Every event/result retains **`replacement_source_qualified=false`**; the best owner-local result is `IDENTITY_QUALIFIED_SOURCE_UNQUALIFIED`. The reported `R_high-D_low<120000` numeric check is insufficient for acceptance. Join actual exclusive qualified replacement routing throughout the request or a server/request source association, and retain that separate stage evidence. Never rewrite the probe's pre-arm metadata or false source flag into serving-replica proof. Different hosts' monotonic clocks are never subtracted.

Engineer01 reports the owner passed24 shared and9 focused local tests; those are author results, not production or duplicate reviewer execution. Neither arming nor trigger traffic was performed by this checklist update.

Retain actual cohorts, query brackets/errors, four-event times, outage upper bound, operation/recovery outcomes and lead decision. Continue the separate30-minute soak/E+15 report. This preparation executed only Kubernetes and failed Compute reads; no S1 restoration time is established.

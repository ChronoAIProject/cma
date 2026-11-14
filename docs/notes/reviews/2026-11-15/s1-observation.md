# S1 read-only backend and restoration observation

Observer: solution-architect-03; cutover executor: deployment-engineer-01; user-service probe owner: prod-verifier-02. The lead approved these read-only Kubernetes/Compute observations on14November. Engineer01 agreed the scope and event boundaries. No rollout, scale, IAM, credential, Secret, pod-exec or probe-traffic changes are included.

**Kubernetes mapping observed; Compute mapping/access and the cutover probe remain HOLD.** Read-only permission is already granted. The technical dependency is a working authorized Compute identity/invocation and actual resource mapping, not another generic approval.

## Actual preparation results

| Item | Observed14November; refresh immediately before S1 |
| --- | --- |
| Context | `gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke` |
| Namespace/canonical Service | `chrono-bot` / `cma-agent-manager` |
| Selector/ports | `app.kubernetes.io/name=cma-agent-manager`; Service80 → EndpointSlice8080 |
| Canonical NEG annotation | `k8s1-002d2683-chrono-bot-cma-agent-manager-80-620084a5` |
| Annotated zones | `asia-northeast1-a`, `asia-northeast1-b`, `asia-northeast1-c` |
| EndpointSlice | One ready/serving/nonterminating endpoint in zone C at preparation; refresh old/new identities after M4. |
| Compatibility alias | `cma-api` selects the workload but has no NEG annotation; never use it as canonical external-health evidence. |
| Ingress | No Ingress objects in `chrono-bot`; repository HTTPRoute targets the canonical Service through the shared Gateway. |
| Compute result | All three NEG reads using `--project=chronoai-501608` returned403, missing `compute.networkEndpointGroups.get`, under inherited gcloud authentication. Failed commands also printed `[]`: that is UNKNOWN, not an empty NEG. |
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
scale_context=gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke
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

Before S1 step3, refresh the outgoing cohort after M4, prove canonical health and arm the independent observer/recovery operator. Agree a bounded sampling cadence and measured query latency that fit the outage budget. The10-second command limit is a maximum wait, not a measured interval or an extension of120s. No slow/missed query is continuous coverage.

Keep four events distinct:

1. **Old backend withdrawal:** retain the last complete health sample showing an old endpoint healthy and first complete sample showing no old endpoint healthy, with query start/end and freshness uncertainty. First-absent poll time is not the exact withdrawal timestamp.
2. **Old effect/socket authority cessation:** executor/authority owner's separate proof; endpoint/Pod/NEG absence cannot supply it.
3. **Replacement user service:** full decoded/predicate-complete result from the bound probe below, attributable to qualified replacement service. Both request starts follow the qualified old-withdrawal bound. Preserve completion and local observer receipt times plus clock uncertainty.
4. **Both-ready:** both qualified replacement Pod identities on distinct hosts, canonical backend health, current generation and membership. This does not end the user-service outage clock.

Let `D_low` be a defensible earliest withdrawal bound including query/freshness uncertainty, and `R_high` the conservative full user-service completion bound. PASS requires **`R_high - D_low <120s`**. Same-observer monotonic receipt time may give a conservative restoration upper bound; never subtract different hosts' monotonic clocks. Unbounded freshness/clock uncertainty means the bound is unavailable. A nominal duration below120 does not pass if its uncertainty crosses120.

Run the deadline alarm independently of blocking reads. At120s from the conservative withdrawal lower bound without proven restoration, stop promotion and use the already approved compatible recovery. UNKNOWN never resets the clock. Preserve a breach even if recovery later succeeds; never force-delete or shorten authority/grace.

## User-service probe — owner adaptation pending

Prod-verifier-02 owns the invocation. Existing `read-capture.mjs` soak/ paced profiles are not cutover-qualified: discovery consumes time, the soak starts after convergence, failures stop traffic, and the result lacks required completion/source association. Do not substitute an older Activity/legacy-usage round or `/livez`/`/ready` diagnostics.

Engineer01 requested: pre-armed pinned fixture/broker; one Agent detail/transcript pair; cap1/no retries; existing lock/pacing/abort rules; bounded window/explicit trigger; sanitized UTC and monotonic request starts, decoded completion and predicate completion. `replacement_source_qualified` stays false until joined to actual exclusive replacement routing throughout the request or a server/request join. The owner must supply the concrete CLI, full-body predicate and source join before the stop checkpoint passes. No probe traffic was requested or launched by this preparation.

Retain actual cohorts, query brackets/errors, four-event times, outage upper bound, operation/recovery outcomes and lead decision. Continue the separate30-minute soak/E+15 report. This preparation executed only Kubernetes and failed Compute reads; no S1 restoration time is established.

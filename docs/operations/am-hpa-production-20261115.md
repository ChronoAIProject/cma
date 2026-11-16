# AM HPA production handoff — 15 November 2026

The lead's 17:48 SGT instruction selects AM minimum 2 / maximum 4, CPU 60%,
300 seconds of scale-down stabilization and one Pod removed per 120 seconds.
Solution Architect 04 owns the S1 packet; Deployment Engineer 02 applies it.
This handoff is ready independently of the local two-AM and rollback workstreams.

The [production manifest](../../infra/api/hpa.yaml) and
[controller template](../../.github/scripts/cma_scale/templates/S3-api-hpa.json)
have identical specs. The template keeps its existing S3 operation name for
controller compatibility. The current lead instruction places this HPA in S1;
the older controller's paired-stage scheduling is not an additional prerequisite.

| Field | Value |
| --- | --- |
| Context | `gke_example-project-02_asia-northeast1_example-cluster-02` |
| Namespace / HPA / target Deployment | `chrono-bot` / `cma-agent-manager` / `cma-agent-manager` |
| API | `autoscaling/v2` |
| Replicas | minimum 2, maximum 4 |
| Metric | CPU utilization, 60% of CPU requests |
| Scale up | stabilization 0; Max of +2 Pods or +100% per 60 seconds |
| Scale down | stabilization 300 seconds; Min, -1 Pod per 120 seconds |

At 17:54 SGT the production `api` container requested 4 CPUs and 256Mi memory,
with limits of 8 CPUs and 8Gi memory. Thus 60% currently represents an average
2.4 CPUs per Pod. The HPA does not change those resources, image, coordination
mode, POD_IP/relay, placement, PDB or drain settings. S1 owns those Deployment
changes. A later CPU-request change also changes the absolute utilization target.

## S1 application

Install this HPA **after** S1 has performed its coordination activation and
1→0→2 replica transition. This keeps the manual cutover and the autoscaler from
managing AM replicas concurrently. The previously installed EG HPA has a
different target and is not modified by these commands.

From the repository root at the AM HPA step:

```bash
am_context=gke_example-project-02_asia-northeast1_example-cluster-02
am_namespace=chrono-bot
am_manifest=infra/api/hpa.yaml

kubectl --context "$am_context" -n "$am_namespace" apply \
  --field-manager=ws4-am-hpa -f "$am_manifest"
kubectl --context "$am_context" -n "$am_namespace" get hpa cma-agent-manager -o yaml
kubectl --context "$am_context" -n "$am_namespace" get deployment cma-agent-manager
kubectl --context "$am_context" -n "$am_namespace" get pods \
  -l app.kubernetes.io/name=cma-agent-manager -o wide
```

Production strict server dry-run accepted this manifest and returned the exact
template spec. It establishes API acceptance, not production scaling success.

## Controlled scale-up/down verification

Developer16's watcher records populated CPU metrics, `AbleToScale=True`,
`ScalingActive=True`, desired/current replicas, Ready and terminating Pods,
and requests served by each AM. `ScalingLimited=True` at a replica floor or
ceiling is expected when its reason identifies that bound.

Deployment02 lowers only the CPU target for the lead-selected exercise. Choose
a positive target from observed utilization so the HPA recommends more than
two replicas. At two replicas, a target below half the observed utilization
requests four, capped by `maxReplicas`. Zero utilization cannot establish
scale-up by lowering a positive target alone. After observing additional Ready
AMs and request service, restore **60**:

```bash
kubectl --context "$am_context" -n "$am_namespace" patch hpa cma-agent-manager \
  --type=json -p='[{"op":"replace","path":"/spec/metrics/0/resource/target/averageUtilization","value":60}]'
kubectl --context "$am_context" -n "$am_namespace" get hpa cma-agent-manager --watch
```

Observe the full 300-second stabilization, -1 Pod per 120-second policy and
actual drain until two remain. Report if real demand continues to require more
than two. Manual Deployment scaling is not proof of HPA behavior.

## S1 rollback integration

Before S1 rollback performs its own replica/coordination transition, freeze both
AM scaling directions and remove this AM HPA. This leaves its current desired
Deployment count in place for the rollback owner. It does not remove the EG HPA.

```bash
kubectl --context "$am_context" -n "$am_namespace" patch hpa cma-agent-manager \
  --type=json -p='[{"op":"replace","path":"/spec/behavior/scaleUp/selectPolicy","value":"Disabled"},{"op":"replace","path":"/spec/behavior/scaleDown/selectPolicy","value":"Disabled"}]'
kubectl --context "$am_context" -n "$am_namespace" delete hpa cma-agent-manager
kubectl --context "$am_context" -n "$am_namespace" get deployment cma-agent-manager
```

# EG HPA production handoff — 15 November 2026

The lead's 17:23 SGT instruction selects EG minimum 2 / maximum 4 with S2
around 20:00, before the AM S1 activation. Deployment Engineer 02 owns production
application. This handoff adds no approval, soak, receipt or review prerequisite.

Use [the production manifest](../../infra/cmaeg/hpa.yaml). The controller's
[EG template](../../.github/scripts/cma_scale/templates/S3-cmaeg-hpa.json) has
the same spec. The EG-only actions introduced by #2065 retain older scheduling
assumptions, including an S2 soak, AM1/OFF and removal of both HPAs before S1.
They do not implement the new S2-with-HPA followed by AM-only S1 sequence.
Deployment02 can apply this manifest directly under the current lead instruction.

| Field | Production value |
| --- | --- |
| Context | `gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke` |
| Namespace / HPA / target Deployment | `chrono-bot` / `cma-event-gateway` / `cma-event-gateway` |
| API | `autoscaling/v2` |
| Replicas | minimum 2, maximum 4 |
| Metric | CPU utilization, 200% of the container CPU request |
| Existing CPU request / limit | 250m / 2 CPUs; 200% is a 500m average CPU target |
| Existing memory request / limit | 2Gi / 2Gi |
| Scale up | stabilization 0; Max of +2 Pods or +100% per 60 seconds |
| Scale down | stabilization 300 seconds; Min, -1 Pod per 120 seconds |

The CPU target and scaling behavior come from #2065. The new ceiling is four.
CPU utilization is relative to requests, so a later request change also changes
the absolute target. This HPA does not modify images, resources, replica-mode
configuration, PDB, placement or termination grace; those belong to S2's
Deployment preparation. At 17:25 SGT production had one Ready EG, a 250m CPU
request, grace 735 seconds, and no EG HPA or PDB.

## Apply with S2

Run from the repository root when deployment02 reaches the EG2/HPA step:

```bash
eg_context=gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke
eg_namespace=chrono-bot
eg_manifest=infra/cmaeg/hpa.yaml

kubectl --context "$eg_context" -n "$eg_namespace" apply \
  --field-manager=ws5-eg-hpa -f "$eg_manifest"
kubectl --context "$eg_context" -n "$eg_namespace" get hpa cma-event-gateway -o yaml
kubectl --context "$eg_context" -n "$eg_namespace" get deployment cma-event-gateway
kubectl --context "$eg_context" -n "$eg_namespace" get pods \
  -l app.kubernetes.io/name=cma-event-gateway -o wide
```

Production strict server dry-run passed on the delivered manifest. Its admitted
spec matched the controller template exactly. This validates API acceptance;
actual scaling is established by the production observations below.

## Verify automatic scale up and scale down

Developer16's read-only watcher and deployment02's controlled exercise can use:

```bash
kubectl --context "$eg_context" -n "$eg_namespace" get hpa cma-event-gateway --watch
kubectl --context "$eg_context" -n "$eg_namespace" top pods \
  -l app.kubernetes.io/name=cma-event-gateway --containers
kubectl --context "$eg_context" -n "$eg_namespace" get deployment,pods \
  -l app.kubernetes.io/name=cma-event-gateway -o wide
```

Observe `AbleToScale=True`, `ScalingActive=True`, populated current CPU metrics,
and current/desired replicas. `ScalingLimited=True` can be expected at the
two-replica floor or four-replica ceiling; identify its reason.

For the controlled exercise, lower only
`spec.metrics[0].resource.target.averageUtilization`. Choose a positive target
from observed utilization that makes the recommendation exceed the current
replica count. At two replicas, a target below half the observed utilization
requests four, capped by `maxReplicas`. If utilization is zero, lowering the
target alone cannot demonstrate scale-up. Restore **200** after additional
EG replicas become Ready:

```bash
kubectl --context "$eg_context" -n "$eg_namespace" patch hpa cma-event-gateway \
  --type=json -p='[{"op":"replace","path":"/spec/metrics/0/resource/target/averageUtilization","value":200}]'
```

Record the actual transition from two to more than two and back to two, Ready
counts, HPA recommendations, per-pod request service, failed requests/restarts
and terminating Pods. Allow the 300-second stabilization, one-Pod-per-120-second
policy and actual drain to finish. If real demand still needs more than two,
report that result. Patching Deployment replicas does not demonstrate HPA
behavior. AM's later S1 activation does not require changing this EG manifest.

## Stop autoscaling while retaining serving capacity

Deployment02 can freeze both directions, then remove this HPA. The Deployment
retains its current desired count; any reduction belongs to the S2 rollback plan.

```bash
kubectl --context "$eg_context" -n "$eg_namespace" patch hpa cma-event-gateway \
  --type=json -p='[{"op":"replace","path":"/spec/behavior/scaleUp/selectPolicy","value":"Disabled"},{"op":"replace","path":"/spec/behavior/scaleDown/selectPolicy","value":"Disabled"}]'
kubectl --context "$eg_context" -n "$eg_namespace" delete hpa cma-event-gateway
kubectl --context "$eg_context" -n "$eg_namespace" get deployment cma-event-gateway
```

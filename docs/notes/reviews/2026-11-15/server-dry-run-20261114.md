# Production scale admission and capacity check — 14 November 2026

DONE: at 18:04–18:07 SGT, all six requested server-side apply dry-runs passed
strict validation against production `chrono-bot`; all six `kubectl diff`
commands returned **1 (differences present), with no diff errors**. Two additional
Pod-template admission checks passed. These were nonpersistent requests. No
Deployment, Pod, PDB, HPA, quota or node was changed by this check.

Source: #1968 (`d38966aab5c77fa2262da2d14ffba0da84cd958c`) with the merged
#1996 EG native `preStop.sleep.seconds: 30` correction
(`37f2bedbfce3be4fe5b8bd1370ba2c81cd872be5`). The correction is the only change
between these commits in the six manifests. Only image references were replaced
for the requested checks. This is admission evidence using Batch 8, not a
Batch 9 runtime/cutover qualification or permission to apply these full files.

| Target | Live generation | Batch 8 digest | Server dry-run | Diff |
|---|---:|---|---|---|
| AM Deployment, replicas 2 | 197 | `4c5497b9c3fbe2a4cd3157e7f692885df1cdebfa0c01bc29fa37878840885873` | PASS | 1 |
| AM PDB | absent | — | PASS | 1 |
| AM HPA | absent | — | PASS | 1 |
| EG Deployment, replicas 2 | 72 | `f8f502aed8b5ea2c68924994c98f988c483dcf689d65998f4d94aeabdf696330` | PASS | 1 |
| EG PDB | absent | — | PASS | 1 |
| EG HPA | absent | — | PASS | 1 |

Images use `us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/`,
repositories `cma-api` and `cma-cmaeg`, respectively. Final readback retained both
original generations, resource versions, images, one Ready replica per service,
zero terminating replicas, no PDBs/HPAs and no persisted test Pods.

## Findings that change operator decisions

**Use the guarded field patches in the stage runbooks. Do not apply the full
historical Deployment manifests.** The admitted EG diff adds historical explicit
`CMAEG_OWNER_KEY_CUTOVER_ID` and `CMAEG_OWNER_KEY_SERVICE_ID` entries that are absent
from the live container env list. Do not import those settings as an incidental
scale change. Preserve live env/envFrom, ownership controls and configuration.
Neither value was read from a Secret or included in the report. The AM diff
also changes the drain hook, probe timing, progress deadline, metrics port/label
and historical annotations. EG changes probes, labels and annotations too.
Only approved stage fields belong in the actual cutover patch. No env changes
were observed in the literal AM admission diff. Retain native EG sleep30;
it remains routing propagation, not proof of secure early drain.

**Steady counts fit this snapshot; the long termination envelope does not.**
The namespace-wide `sandbox-quota` applies to these services despite its name.
It has no scope restriction. Hard limits are requests CPU 40 / memory 80 GiB,
limits CPU 80 / memory 160 GiB, and 50 Pods. Current use is requests 4.275 CPU /
2.3125 GiB, limits 10.25 CPU / 10.125 GiB, and 3 Pods. `sandbox-limits` permits
at most 8 CPU and 8 GiB per container; both proposed templates comply.

The table includes the currently observed non-AM/EG namespace use. It assumes
unchanged requests/limits and counts every still quota-charged Pod, including
terminating Pods until actually released. Admission dry-runs do not reserve
quota or accumulate usage across calls.

| AM / EG processes | Namespace Pods | CPU requests | CPU limits | Memory requests / limits, GiB | Quota result |
|---|---:|---:|---:|---:|---|
| S1: 2 / 1 | 4 | 8.275 | 18.25 | 2.5625 / 18.125 | fits |
| S2: 2 / 2 | 5 | 8.525 | 20.25 | 4.5625 / 20.125 | fits |
| S3 steady max: 5 / 5 | 11 | 21.275 | 50.25 | 11.3125 / 50.125 | fits |
| 6 / 6, arithmetic surge example | 13 | 25.525 | 60.25 | 13.5625 / 60.125 | fits; not a rollout grant |
| 8 / 8 | 17 | 34.025 | **80.25** | 18.0625 / 80.125 | CPU limit quota exceeded |
| 12 / 12, illustrative G760/P120 | 25 | **51.025** | **120.25** | 27.0625 / 120.125 | both CPU quotas exceeded |

With current other use, enforce `4*AM + 0.25*EG + 0.025 <= 40` and
`8*AM + 2*EG + 0.25 <= 80`, plus memory/Pod quotas. Eight of each already fails;
seven of each fitting quota is not a qualified process or socket budget.
A hard lifetime G has not been established by a Kubernetes grace setting.
Before creating either HPA, recompute each service's `5 + ceil(G/P)` envelope
using measured/qualified inputs and all consumers. If it fails, keep fixed two
and obtain an explicitly approved cadence/capacity remedy. Do not silently
change quota, resource requests, CPU targets, node taints or lifecycle bounds.

Three Ready, schedulable shared nodes, one per zone a/b/c, are eligible. Each
has 15.89 allocatable CPU and about 27.726 GiB allocatable memory. Their free
requests at 18:04 SGT were:

| Zone / node suffix | Free CPU | Free memory, GiB | Assigned / allowed Pods |
|---|---:|---:|---:|
| a / `f1f574f3-vq72` | 12.208 | 20.866 | 39 / 110 |
| b / `0f240dfa-3mt9` | 11.981 | 19.467 | 32 / 110 |
| c / `1b38e993-sm41` | 8.806 | 21.330 | 20 / 110 |

Total free requests: **32.995 CPU / 61.663 GiB**. The two sandbox nodes have
an untolerated `sandbox.gke.io/runtime=gvisor:NoSchedule` taint and are excluded.
No pending unscheduled Pods were observed. At these resources, AM2/EG2 can use
distinct hosts for each service; steady AM5/EG5 admits a 2/2/1 distribution per
service. Six per service also fits a 2/2/2 request placement with the existing
other consumers. This is resource arithmetic, not a scheduler reservation.
Twelve AM alone request 48 CPU, exceeding the shared nodes' entire 47.67 CPU
before EG or other consumers. Do not assume unobserved node autoscaling.

AM's observed memory was **3273.7 MiB against a 256 MiB request** (limit 8 GiB);
its request therefore understates observed memory occupancy. Recheck actual
memory pressure/headroom with the final image and load. EG was 20.15 MiB against
2 GiB request and limit. Both are **Burstable**, because CPU requests differ from
limits; EG's equal memory request/limit does not establish Guaranteed Pod QoS.

## PDB, HPA and admission scope

Both PDBs admit `minAvailable: 1`, `unhealthyPodEvictionPolicy: AlwaysAllow` and
the exact respective Deployment selector. There were no existing PDBs or HPAs
to overlap. At two healthy Pods, expect one allowed voluntary eviction after
controller reconciliation; at one, expect zero. Read actual PDB status after
installation. PDBs do not prevent Deployment scale-to-zero or HPA downscale.

Both HPAs admit autoscaling/v2, min2/max5, AM60% / EG200% CPU, up Max(+2,+100%)/60s,
down stabilization300s and Min(-1)/120s. The server accepted native sleep,
`matchLabelKeys: [pod-template-hash]`, `nodeTaintsPolicy: Honor` and the hostname
hard / zone soft topology rules. P120 remains a structural value until actual
handoff/lifetime qualification. Creating an HPA at today's N1 could immediately
scale to its minimum; these checks did not create one.

Metrics API Available=True; fresh AM/EG PodMetrics were present (timestamps
18:04:13 / 18:04:03 SGT, about 33–34s sampling windows). CPU was 0.474 / 0.289
cores, below their 2.4 / 0.5 core per-Pod targets. This establishes metric
availability, not useful-load calibration or successful automatic scaling.
There is no live HPA status to validate until authorized installation.

The two additional isolated Pod dry-runs exercise current Pod admission,
LimitRange/defaulting and current quota against the proposed templates. They
do not create ReplicaSets, execute hooks, check image pull/startup, reserve a
combined stage's resources, run scheduling or establish cutover availability.

## Exact nonpersistent reproduction

From the repository root, extract the corrected source and bind the observed
Batch 8 image references. JSON is accepted by kubectl and avoids a YAML Python
dependency. The output directory is local and ignored. Re-read actual image
identity before any later final-candidate check.

```bash
set -euo pipefail
umask 077
export PATH=/usr/local/bin:/opt/homebrew/share/google-cloud-sdk/bin:/opt/homebrew/bin:$PATH
scale_context=gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke
scale_check=.github/.cmaeg-local/scale-dry-run-20261114-batch8
scale_manifest_source=37f2bedbfce3be4fe5b8bd1370ba2c81cd872be5
mkdir -p "$scale_check"
k() { /usr/local/bin/kubectl --context "$scale_context" -n chrono-bot --request-timeout=45s "$@"; }
export SCALE_CHECK_AM_IMAGE=us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api@sha256:4c5497b9c3fbe2a4cd3157e7f692885df1cdebfa0c01bc29fa37878840885873
export SCALE_CHECK_EG_IMAGE=us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-cmaeg@sha256:f8f502aed8b5ea2c68924994c98f988c483dcf689d65998f4d94aeabdf696330
for scale_role in api cmaeg; do
  for scale_kind in deployment pdb hpa; do
    git show "$scale_manifest_source:infra/$scale_role/$scale_kind.yaml" |
      ruby -ryaml -rjson -e 'puts JSON.generate(YAML.safe_load(STDIN.read))' |
      python3 -c '
import json,os,sys
obj=json.load(sys.stdin)
if obj["kind"] == "Deployment":
    role="AM" if sys.argv[1] == "api" else "EG"
    container="api" if role == "AM" else "cmaeg"
    for c in obj["spec"]["template"]["spec"]["containers"]:
        if c["name"] == container:
            c["image"]=os.environ["SCALE_CHECK_"+role+"_IMAGE"]
json.dump(obj,sys.stdout,indent=2)
' "$scale_role" > "$scale_check/$scale_role-$scale_kind.json"
  done
 done
```

Run exactly the six nonpersistent apply checks and diffs. Suppress raw responses
and diffs because the live merge can contain private env/annotations. Exit1
from diff means changes; any other nonzero result is an error requiring diagnosis
through a safe projection, never a reason to invoke real apply.

```bash
for scale_role in api cmaeg; do
  for scale_kind in deployment pdb hpa; do
    scale_file="$scale_check/$scale_role-$scale_kind.json"
    if k apply --dry-run=server --validate=strict -f "$scale_file" -o name >/dev/null 2>&1; then
      printf '%s admission PASS\n' "$scale_file"
    else
      printf '%s admission ERROR; stop and inspect sanitized diagnostics\n' "$scale_file"
      exit 1
    fi
    if k diff --field-manager=kubectl-client-side-apply -f "$scale_file" >/dev/null 2>&1; then
      printf '%s diff 0: no changes\n' "$scale_file"
    else
      scale_diff_rc=$?
      if [ "$scale_diff_rc" -ne 1 ]; then
        printf '%s diff ERROR %s\n' "$scale_file" "$scale_diff_rc"
        exit "$scale_diff_rc"
      fi
      printf '%s diff 1: changes present\n' "$scale_file"
    fi
  done
 done
k get resourcequota,limitrange,pdb,hpa
k get deployments cma-agent-manager cma-event-gateway \
  -o 'jsonpath={range .items[*]}{.metadata.name}{" generation="}{.metadata.generation}{" replicas="}{.spec.replicas}{" ready="}{.status.readyReplicas}{"\n"}{end}'
```

Local sanitized results: `.github/.cmaeg-local/scale-dry-run-20261114-batch8/`
contains `inspection.json`, `admission.json`, `final-checks.json` and the six
rendered files. No raw live Deployment, Secret contents or raw diff was exported
by the checker. No rollback is required for nonpersistent checks. Actual stage
recovery remains the state-specific branches in S1/S2 and freeze/delete/serial
descent in S3; the capacity findings do not authorize new recovery operations.

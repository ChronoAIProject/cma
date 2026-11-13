# CMA scale: staged apply and rollback

Operational handoff for **deployment-engineer-01/02**, issue #1956. This PR targets
develop **after #1370**. It is an inert, conditional manifest delivery; neither
merge nor this document authorizes production writes. Application switches remain
default-off in the integration. This procedure follows architect contractv1
(`d7497675aca28b48c7d1b72b0c385a8b145dbf54f7a04489ef2731a21f5dea1a`)
and A1 (`1b9530fc06b814dc8733c405ad51be06e4375978c8a7d8010fca898722326ebc`).
Read [capacity qualification](CMA_Scale_Capacity.md) before any stage.

## Source of truth and execution authority

AM manifests are `infra/api/{deployment,pdb,hpa}.yaml`; EG manifests are
`infra/cmaeg/{deployment,pdb,hpa}.yaml`. Release publication does not apply them:
`docs/Release.md` defines explicit promotion; workflows validate/package images.
Live image pins can be newer than checked-in Deployment pins. **Never apply an
entire Deployment or directory for this change.** Use the tested narrow patch
projection below and install the HPA files only in S3. Apply individual PDB files. PDB `unhealthyPodEvictionPolicy: AlwaysAllow`
lets a non-Ready pod be evicted without indefinitely blocking node drain;
`minAvailable:1` protects the healthy survivor.

Lead admin-merges reviewed exact heads using `--match-head-commit`; the team
builds locally under approved build authority, without waiting for/polling CI.
Before each gated write, deployment engineers reread
`team.sh policy show ChronoAIProject/cma` and consume a fresh
`team.sh policy-check deploy --approval <step-id> --context <release-SHA>` grant.
The observed route is `reviewed-release-controller`: submit these exact narrow
command payloads through that reviewed controller. They are **not** a bypass.
Direct commands require the lead to record `owner-authorized-direct`, deploy/skip-ci
allow and matching per-step approvals. A refusal stops the step; never edit policy.
Obtain explicit rollback permissions before starting. Server dry-run does not
replace permission, application tests or live backend verification.

Earliest stage start15 Nov03:00 SGT; no write02:00–02:59, including rollback.
Reserve S1~03:00–04:15, S2~04:15–05:30, S3~05:30–09:30, review09:30–12:00,
owner result before13:00. Recheck time immediately before every write; do not
start an operation whose bounded duration can cross a prohibited window.

## Release packet and local preparation

The lead supplies exact integrated source and manifest SHAs, architect PASS
for every relevant head, locally built immutable AM/EG/runtime image digests,
build provenance, additive schema/role receipts, retained-runtime image/protocol
and exact challenge/lease-duration compatibility floor,
all local correctness/performance/fault evidence, approved max-owned-Agent value,
capacity receipts, chosen HPA cadence and per-stage/per-step approvals. No moving
tag or invented digest is acceptable. An unresolved item is HOLD.

From the exact approved checkout, use Bash, scoped variables and an owned receipt
directory. All git/gh commands require active `chronoai-shining`; do not switch.
The patch generator makes no cluster calls and retains image/configuration fields.

```bash
set -euo pipefail
gh auth status --active
# Stop if the active account is not chronoai-shining.
: "${manifest_sha:?exact reviewed manifest commit required}"
: "${release_sha:?exact integrated source commit required}"
: "${am_image:?reviewed immutable AM reference required}"
: "${eg_image:?reviewed immutable EG reference required}"
: "${owned_agents:?reviewed positive per-AM capacity required}"
: "${scale_receipts:?owned absolute receipt directory required}"
test "$(git rev-parse HEAD)" = "$manifest_sha"
python3 - "$release_sha" "$am_image" "$eg_image" "$owned_agents" <<'PY'
import re, sys
assert re.fullmatch(r'[0-9a-f]{40}', sys.argv[1]), 'source SHA required'
assert all(re.fullmatch(r'[^\s]+@sha256:[0-9a-f]{64}', v) for v in sys.argv[2:4]), 'immutable images required'
assert sys.argv[4].isdigit() and int(sys.argv[4]) > 0, 'owned Agent bound required'
PY
mkdir -p "$scale_receipts"
scale_context=gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke
scale_namespace=chrono-bot
k() { kubectl --context "$scale_context" -n "$scale_namespace" "$@"; }
outside_blackout() { test "$(TZ=Asia/Singapore date +%H)" != 02; }
fence_patch() {
  local service="$1" file="$2" version
  version="$(k get deployment "$service" -o jsonpath='{.metadata.resourceVersion}')"
  test -n "$version"
  python3 - "$file" "$version" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
patch = json.loads(path.read_text())
patch['metadata'] = {'resourceVersion': sys.argv[2]}
path.write_text(json.dumps(patch, indent=2) + '\n')
PY
}
ruby .github/scripts/cma-scale-manifest-contract.rb
ruby .github/scripts/cma-scale-manifest-tests.rb
ruby .github/scripts/cma-scale-manifest-contract.rb --patch api 1 > "$scale_receipts/am-stage.json"
ruby .github/scripts/cma-scale-manifest-contract.rb --patch cmaeg 1 > "$scale_receipts/eg-stage.json"
```

Construct the image/switch fields locally, leaving production ConfigMaps intact:

```bash
python3 - "$scale_receipts" "$am_image" "$eg_image" "$owned_agents" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
for role, image in [('am', sys.argv[2]), ('eg', sys.argv[3])]:
    path = root / (role + '-stage.json')
    patch = json.loads(path.read_text())
    c = patch['spec']['template']['spec']['containers'][0]
    c['image'] = image
    if role == 'am':
        c['env'] = [
            {'name': 'POD_IP', 'valueFrom': {'fieldRef': {'fieldPath': 'status.podIP'}}},
            {'name': 'CHRONO_AGENT_MANAGER_RELAY_ADDRESS', 'value': 'http://$(POD_IP):8080'},
            {'name': 'CHRONO_AGENT_MANAGER_REPLICAS_ENABLED', 'value': 'true'},
            {'name': 'CHRONO_AGENT_MANAGER_MAX_OWNED_AGENTS', 'value': sys.argv[4]}]
    else:
        c['env'] = [{'name': 'CMAEG_REPLICAS_ENABLED', 'value': 'true'}]
    path.write_text(json.dumps(patch, indent=2) + '\n')
PY
```

Existing named env entries must have the expected source/type; if a valueFrom
already exists for an overridden literal, add a reviewed removal in the patch.
Compare live topology first: production had none at initial inspection. If newer
constraints exist, merge by topologyKey/whenUnsatisfiable under review instead
of discarding them. Add the fresh Deployment `metadata.resourceVersion` to each
final patch after the zero-pod boundary; failure on concurrent mutation means
reinspect, not retry with a new version silently. Read back each mutation.

Before any write, capture only images, replica/status counts, strategy, resources,
probes, grace/hooks, topology, named nonsecret replica controls, resourceVersion
and existing HPA/PDB specs into private receipts. Hash pod identities and record
zone/counts; no Secret reads, raw ConfigMap/Deployment exports, pod exec or logs
with request/user payloads. Keep source/image/config changes as separate strata.
Do not restore an old whole object from this capture.

## Common stage gates and verification

Require integrated local fmt/clippy/tests and real Mongo independent-instance
proof. **Before zeroing legacy AM**, inventory every retained traffic-capable
native/hosted runtime and verify its exact image digest, protocol and ownership
challenge/lease-duration compatibility with the final candidate. Protocolv1
presence alone is insufficient: old #1370 receivers use20s while the integrated
target is Mongo15s/renew3s and receiver10s; the receiver challenge `valid_for_ms`
must exactly equal the manager's tested expected value (proposed10000ms).
Pin the final durations and compatibility matrix in the release packet.
Require positive same-floor evidence and negative tests rejecting an old20s
receiver that still advertises v1, wrong challenge duration, stale/expired
challenge and incompatible image/reader combinations. Those integration tests
are mandatory evidence, **not executed by the manifest validator**. Any missing
runtime, stale inventory or failed/missing challenge proof blocks S1 before
the first scale-to-zero. Updating only the default runtime image does not qualify
retained runtimes or replace their upgrade/continuity proof. Then require:
shared auth/tickets/desktop/quota/cache and every background effect is safe at N;
AM101 sudden-loss trials, EG10 process/node losses and two worker saturation/panic
tests meet <=30s sustained service recovery. Required provider24h/1000-event gates
and Unknown/Partial receipt reconciliation remain required. Preserve user homes,
accepted work, epoch watermarks and original operation IDs. No sticky affinity.

Require matched same-candidate N1/N2/2→3→5→2 performance and the frozen batch2/3
route catalog, plus final-candidate N1 versus the latency owner's baseline.
Agent create/tab-open/send/interrupt <=1s, usage <=2s, other route p95s as frozen.
Use at least three baseline repeats, preregister measured noise and confidence
criteria before N2; report N, errors, missing completions, p50/p95 and p99 only
when N>=1000. Missing/unstable data is INCONCLUSIVE. Do not generate production
canary traffic or faults without separately scoped authorization.

Resource dry-run/read-only commands, within the permitted release route:

```bash
k get deployment cma-agent-manager cma-event-gateway \
  -o custom-columns=SERVICE:.metadata.name,DESIRED:.spec.replicas,READY:.status.readyReplicas,AVAILABLE:.status.availableReplicas
k get hpa,pdb,resourcequota,limitrange
kubectl --context "$scale_context" get apiservice v1beta1.metrics.k8s.io
k top pod -l 'app.kubernetes.io/name in (cma-agent-manager,cma-event-gateway)' --containers
k patch deployment cma-agent-manager --type=strategic --patch-file "$scale_receipts/am-stage.json" --dry-run=server -o name
k patch deployment cma-event-gateway --type=strategic --patch-file "$scale_receipts/eg-stage.json" --dry-run=server -o name
k apply --dry-run=server -f infra/api/pdb.yaml -o name
k apply --dry-run=server -f infra/cmaeg/pdb.yaml -o name
```

These dry-runs do not execute the CLI or establish membership/capacity. Prove both
image-contained hooks under non-root/read-only filesystem in local image gates:
AM `/usr/local/bin/cma --agent-manager-drain`, EG
`/usr/local/bin/cmaeg --replica-drain`; authenticated loopback, idempotent, <=25s,
early irreversible readiness withdrawal, close-before-exact-lease-release. Shell
hooks always wait30s, then return the CLI status so failure is observable. Failed
preStop does not prevent kubelet termination: SIGTERM must repeat the safe drain
and failed close must retain authority until safe expiry. Neither grace nor a
healthy probe proves failover performance. Budget AM25+30+660+30+15=760;
EG25+30+660+45=760; prove actual sequential/concurrent shutdown fits these sums.

## S1: AM no-overlap activation, then fixed two

EG remains at its proven state; no AM HPA may exist. Confirm the complete retained
receiver image/challenge/duration matrix and its negative tests before the first
write; a protocolv1-only inventory is a failed preflight. Initial switch-off/old
managers must not overlap participating managers. Obtain the explicit outage
window and all-writers/receiver proof; PDB does not preserve availability at zero.
The following are individually approved payloads, not a batch to run unattended:

```bash
outside_blackout
k scale deployment/cma-agent-manager --current-replicas=1 --replicas=0
k wait --for=delete pod -l app.kubernetes.io/name=cma-agent-manager --timeout=20m
# Verify zero live old processes/receiver authority, then bind the reviewed patch.
fence_patch cma-agent-manager "$scale_receipts/am-stage.json"
k patch deployment cma-agent-manager --type=strategic --patch-file "$scale_receipts/am-stage.json" --dry-run=server -o name
outside_blackout
k patch deployment cma-agent-manager --type=strategic --patch-file "$scale_receipts/am-stage.json" -o name
k rollout status deployment/cma-agent-manager --timeout=35m
# HOLD until protocol, membership, /ready, endpoints/NEG and approved canaries pass at ON1.
outside_blackout
k scale deployment/cma-agent-manager --current-replicas=1 --replicas=2
k rollout status deployment/cma-agent-manager --timeout=35m
outside_blackout
k apply -f infra/api/pdb.yaml -o name
k get pdb cma-agent-manager
```

Require two Ready/Available, healthy canonical endpoints/NEGs, distinct hosts,
per-pod traffic and ownership proof. `/ready` must be false before joined/renewed
membership and throughout draining; liveness remains separate. After all old
processes finish, run the shared30-minute stable soak below. Lead records S1 PASS
and separately authorizes S2. Timeout/HOLD does not permit shortening gates.

## S2: EG no-overlap activation, then fixed two

Require S1 PASS, all enabled provider/partition/console evidence and no EG HPA.
The live console and legacy-console routes are reachable; require shared session,
CSRF/refresh/logout proof. No-overlap is the default absent reviewed mixed-mode
proof. Preserve receipts for uncertain external outcomes; never blindly resend.

```bash
outside_blackout
k scale deployment/cma-event-gateway --current-replicas=1 --replicas=0
k wait --for=delete pod -l app.kubernetes.io/name=cma-event-gateway --timeout=20m
# Prove old sockets closed/authority expired, then bind the reviewed patch.
fence_patch cma-event-gateway "$scale_receipts/eg-stage.json"
k patch deployment cma-event-gateway --type=strategic --patch-file "$scale_receipts/eg-stage.json" --dry-run=server -o name
outside_blackout
k patch deployment cma-event-gateway --type=strategic --patch-file "$scale_receipts/eg-stage.json" -o name
k rollout status deployment/cma-event-gateway --timeout=35m
# HOLD until /readyz participation, source leases, partitions and receipt recovery pass at ON1.
outside_blackout
k scale deployment/cma-event-gateway --current-replicas=1 --replicas=2
k rollout status deployment/cma-event-gateway --timeout=35m
outside_blackout
k apply -f infra/cmaeg/pdb.yaml -o name
k get pdb cma-event-gateway
```

Both instances must receive work while each bot/partition has one authorized
generation. An idle member may be ready; forced equal ownership is not required.
Require irreversible readiness withdrawal and close-before-release, then the
shared soak and lead S2 PASS. Sleep-only or unimplemented drain code blocks S2.

## S3: HPA activation and measured automatic up/down

Require S1/S2 PASS, actual Atlas>=30% headroom, the full quota/node/provider bound
and CPU qualification in the capacity document. The selected cadence and max5
must match reviewed tests. No rollout overlaps HPA qualification. Existing HPA
must be absent; otherwise capture/review its exact ownership before proceeding.

```bash
k apply --dry-run=server -f infra/api/hpa.yaml -o name
k apply --dry-run=server -f infra/cmaeg/hpa.yaml -o name
outside_blackout
k apply -f infra/api/hpa.yaml -o name
k get hpa cma-agent-manager -o jsonpath='{.spec}{"\n"}{.status}{"\n"}'
# Require usable fresh metrics/conditions and stable idle behavior before EG.
outside_blackout
k apply -f infra/cmaeg/hpa.yaml -o name
k get hpa cma-event-gateway -o jsonpath='{.spec}{"\n"}{.status}{"\n"}'
```

Using separately approved load, observe real metric-triggered2→3→5 for each,
then together after capacity proof. A manual replicas patch does not prove HPA.
Reduce only owned load; observe the full600s down stabilization and -1/120s rate,
all Ready/terminating/live-client counts, successful early drain and handoff,
until stable two. Demonstrate no missing-metric downscale in isolated tests.
Start the final30-minute soak only after actual terminating processes are gone.
Do not reapply Deployment `replicas:2` while an HPA owns its scale subresource.

## Soak, abort, rollback and receipts

Every stage needs30 stable minutes after its intended replica count is Ready,
healthy in endpoints/NEG, ownership stable, and old processes finished. Record
per-minute desired/Ready/Available/terminating, backend health, owner/epoch/stale
effect counters, stream gaps, oldest accepted-unfinished age, terminal/receipt
reconciliation, CPU/throttling/memory/OOM, Mongo occupancy/checkout/timeouts,
worker lag/completion, provider limits/Unknowns and HPA conditions/recommendations.
Include at least two five-minute worker/metering cycles and every route comparison.
Restart stable-soak timing on any image/config/replica change; retain fault/ramp
windows in the results. Correctly rejected stale requests are not committed
stale effects, but sustained refusals still fail availability.

Immediate ABORT: wrong owner/stale committed effect, duplicate external action,
accepted work/terminal lost, two desktop owners, auth bypass, epoch/cursor damage,
zero healthy backends, >30s loss of sustained service, incompatible receiver,
OOM/repeated crash, breached process/provider bounds or Atlas headroom<30%.
Performance regression beyond preregistered noise/absolute limits aborts after
two consecutive five-minute windows; widespread failures abort immediately.
Missing evidence, unknown provider outcome or low samples HOLD promotion.

**S3 rollback first freezes HPA, then removes only S3-created objects:**

```bash
outside_blackout
k patch hpa cma-agent-manager --type=merge -p '{"spec":{"behavior":{"scaleUp":{"selectPolicy":"Disabled"},"scaleDown":{"selectPolicy":"Disabled"}}}}'
outside_blackout
k patch hpa cma-event-gateway --type=merge -p '{"spec":{"behavior":{"scaleUp":{"selectPolicy":"Disabled"},"scaleDown":{"selectPolicy":"Disabled"}}}}'
# Read back both Disabled policies; each delete below needs its own approval.
outside_blackout
k delete hpa cma-agent-manager --wait=true
outside_blackout
k delete hpa cma-event-gateway --wait=true
# Verify absence and stable desired counts; an in-flight controller write may settle late.
```

Keep a safe serving count for current demand. Descend **one at a time**, separately
approved, with expected current replicas to detect a race:

```bash
: "${rollback_service:?canonical AM or EG deployment required}"
: "${current_replicas:?fresh observed serving count required}"
case "$rollback_service" in cma-agent-manager|cma-event-gateway) ;; *) exit 1 ;; esac
test "$current_replicas" -gt 2
next_replicas=$((current_replicas - 1))
outside_blackout
k scale "deployment/$rollback_service" --current-replicas="$current_replicas" --replicas="$next_replicas"
k rollout status "deployment/$rollback_service" --timeout=35m
# Independently wait until ALL departing processes finish; rollout status alone is insufficient.
# Verify survivor traffic/ownership and fresh capacity before the next approved decrement.
```

Stop at fixed two with S1/S2 switches/images/PDBs retained, or retain a higher safe
count if demand requires it. Observe30 minutes of recovered performance. For S1
or S2 failure at two, the approved safe rollback is `scale --current-replicas=2
--replicas=1` for that service after HPA absence is proven, with actual drain and
survivor proof; PDB minAvailable1 remains. Keep AM ownership ON and a tested
protocol-compatible image. EG image recovery is **forward-fix-only** unless an
exact older digest has explicit retained-record compatibility proof/approval.

Never use generic `rollout undo`, flip modes OFF, erase epochs, replace user homes
or restore a captured legacy image automatically. AM mode-off needs a separate
full stop, exact-token runtime legacy resets with watermark preservation, proof
for every receiver, then exactly one compatible legacy writer. EG mode-off needs
its own all-writer/retained-record proof. If unsafe, quiesce new admission under
approved controls, preserve fenced custody and ask the lead for recovery.

Receipt per stage: exact source/manifest SHAs, image digests, config/switch digest,
approval IDs/route, time windows, schema/receiver image and exact-duration floor,
retained-runtime inventory hash and positive/negative challenge test artifacts,
all comparisons and failed/NOT_RUN/INCONCLUSIVE cells, resource/HPA/PDB snapshots,
peak live clients/processes, hashes of operator-owned canary receipts, rollback
outcome and remaining gates. Retain sanitized evidence under the engineer's owned
path; remove temporary download/build/dry-run files. Issue closure for #1956
covers delivery of manifests/runbook only; production verification stays on
#1356/#1541/#1530 and the lead's S1/S2/S3 receipts.

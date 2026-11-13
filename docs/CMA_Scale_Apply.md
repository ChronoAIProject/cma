# CMA scale: staged apply and rollback

Operational handoff for **deployment-engineer-01/02**, issue #1956. This PR targets
develop **after #1370**. It is an inert, conditional manifest delivery; neither
merge nor this document authorizes production writes. Application switches remain
default-off in the integration. Follow architect A2
(`b238fbbc032f2f6d8bdfd96d8bbc669a985833a2a1cd1ca153f46d5c9ec7c717`),
updated contract (`79b2de77e6b4b68a1c919ba68b355487274787ad702ef111766313c22840b11f`)
and stages (`229cc0388034fed1d102da641802a0789e469650ae0d1bd20cf1297ce4cf4909`).
Read [capacity qualification](CMA_Scale_Capacity.md) before any stage.
The canonical-controller preparation described below is **execution HOLD** until
its exact tooling and isolated actual-controller proof receive architect PASS.

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
Proposed outage windows S1 03:15 and S2 04:15 still need owner approval.
Reserve S1~03:15–04:15, S2~04:15–05:30, S3~05:30–09:30, review09:30–12:00,
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
# The reviewed execution controller also bounds transport/request duration.
# Reserve that duration before02:00; checking the current hour alone is unsafe.
outside_blackout() {
  : "${max_write_seconds:?reviewed dispatch/request bound required}"
  python3 - "$max_write_seconds" <<'PYTIME'
import datetime, sys, zoneinfo
bound = int(sys.argv[1])
assert 0 < bound < 86400
now = datetime.datetime.now(zoneinfo.ZoneInfo('Asia/Singapore'))
end = now + datetime.timedelta(seconds=bound)
for day in (now.date(), end.date()):
    start = datetime.datetime.combine(day, datetime.time(2), now.tzinfo)
    stop = start + datetime.timedelta(hours=1)
    assert not (now < stop and end >= start), 'write may overlap02:00-02:59'
PYTIME
}
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
ruby .github/scripts/cma-scale-manifest-contract.rb --patch api 0 > "$scale_receipts/am-stage.json"
ruby .github/scripts/cma-scale-manifest-contract.rb --patch cmaeg 0 > "$scale_receipts/eg-stage.json"
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
        c['env'] = [
            {'name': 'POD_IP', 'valueFrom': {'fieldRef': {'fieldPath': 'status.podIP'}}},
            {'name': 'CMAEG_REPLICA_ADDRESS', 'value': 'http://$(POD_IP):8090'},
            {'name': 'CMAEG_REPLICAS_ENABLED', 'value': 'true'}]
    path.write_text(json.dumps(patch, indent=2) + '\n')
PY
```

EG address and switch names are developer-15's source contract pending exact
integrated-SHA review. Confirm the existing EG listen port8090 (`CMAEG_LISTEN` and named `http`
container port), direct Mongo mode,
additive `--migrate-replicas` marker/index and durable routed admission/recovery
before S2. The advertised address must be a literal Pod IP/direct origin, never
a Service address. Neither this patch nor drain204 proves sockets/leases handed
off. Reconcile any final reviewed binary contract change before sealing the packet.

Existing named env entries must have the expected source/type; if a valueFrom
already exists for an overridden literal, add a reviewed removal in the patch.
Compare live topology first: production had none at initial inspection. If newer
constraints exist, merge by topologyKey/whenUnsatisfiable under review instead
of discarding them. Add the fresh Deployment `metadata.resourceVersion` to each
final patch immediately before its approved preparation step; failure on concurrent mutation means
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
tests meet <=30s sustained service recovery. Local kill plus cordon/rescheduling
node-loss equivalent is required; a production drill is optional after S3 with
separate approval. Supported provider idempotency must be used; without it never
automatically resend Unknown/Partial. Preserve flagged intent/custody, stop
promotion and page the lead. Every accepted S2 canary must reach terminal or
reconciled state; unresolved flags are not PASS. Reconciled failures stay failures.
A2 replaces pre-release24h/1000 events with provider-specific S2 canary soak and
assigned24h post-go-live monitoring, including owners, approved mechanism and
abort/recovery triggers in the packet. Preserve user homes,
accepted work, epoch watermarks and original operation IDs. No sticky affinity.

Require matched same-candidate N1/N2/2→3→5→2 performance and the frozen batch2/3
route catalog, plus final-candidate N1 versus the latency owner's baseline.
Frozen `latency-route-catalog-a2.json` SHA256
`a47475609633f3f736147dbded6dace5864d174b0cf58283425e639aba3ad951` has43 criterion
rows,37 route memberships/34 distinct routes and six client boundaries. Preserve
every repeated/conditional/alternate row; missing routes remain INCONCLUSIVE.
Plain/desktop Agent create <=5s END TO END; tab-open/send/interrupt <=1s, usage
<=2s. Header timing cannot replace any completed client journey.
Use at least three baseline repeats, preregister measured noise and confidence
criteria before N2; report N, errors, missing completions, p50/p95 and p99 only
when N>=1000. Missing/unstable data is INCONCLUSIVE. Do not generate production
canary traffic or faults without separately scoped authorization. Analyst02's
three final-candidate production N1 windows require stable matching service by
01:30 (windows01:30–03:00, read-only during blackout). This cutover does not add
an ON1 stage to satisfy that schedule. If unavailable, obtain authorized matching
ON1/ON2 fixture proof from the verifier or HOLD; do not compress windows or use
batch4 noise. N1 modeOFF versus N2 modeON is a configuration confounder.

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

## Controller and route packet: prepare before the outage

Retain each canonical Deployment UID/selector, four Service aliases, Gateway,
HTTPRoute, NEG identity and backend policies. No routing, affinity, HPA target,
backend drain or health-check change is proposed. Include exact Route parents,
backendRefs/ports, Service selectors/targetPorts, endpoint/NEG membership and
backend health source in the lead-approved packet. Console/legacy-console are
reachable EG routes. GCP backendServices list permission was denied to the
engineer: obtain an authorized observer get-health receipt, not a permission bypass.

Architect-proposed supported-controller variant, **pending lead approval of a
change to A2's literal pre-created-RS0 order**: no HPA/concurrent release; pause
canonical Deployment at1 and verify stable old1; install the final template at
canonical desired1 with replica mode confined to new pod-template env. Pre-pull
immutable images and qualify nodes before the outage. Then old1->0; prove every
legacy effect stopped; unpause at desired0 so the Deployment controller creates
the exact owned newRS0; verify it before scaling canonical desired2. RS0 therefore
appears **after** old shutdown. Prepared template/cache is not prepared RS0 or
a running warm application. Do not claim the literal earlier order is satisfied.

Do not manufacture ReplicaSets, pod-template-hash, ownerReferences or revisions.
The actual Deployment controller must create/own/select the new RS; verify exact
parent UID, selector, template, image/configuration and desired0 with no legacy
restart or orphan adoption. Pausing prevents template rollout, not all scaling.
A separate candidate Deployment changes routing/HPA ownership and needs its own
review. If literal pre-existing RS0 remains mandatory, HOLD for separately
reviewed controller tooling/proof; none is supplied here.

Before production preparation, isolated actual-controller tests must prove
pause/template reconciliation at old1, stale UID/RV/concurrent-writer refusal,
controller restart, old1->0, unpause-at0/newRS0 and new0->2. Prove no legacy pod
can reappear and no candidate starts before the no-legacy-effects gate. API
server dry-run alone cannot prove this. Capture only narrow nonsecret fields;
never persist a whole raw Deployment/Secret to construct a template.

The following are **proposed payloads**, pending lead route/order approval and
exact-version architect review with actual-controller evidence. The execution
controller must bind UID/resourceVersion/expected spec, stage grants, blackout
duration and rollback at every write. They are not an approved unattended script.

```bash
: "${stage_service:?cma-agent-manager or cma-event-gateway}"
: "${stage_patch:?reviewed am-stage.json or eg-stage.json}"
case "$stage_service" in cma-agent-manager|cma-event-gateway) ;; *) exit 1 ;; esac
# HPA absent, legacy desired1/Ready1; capture exact rollback and parent UID first.
outside_blackout
k patch deployment "$stage_service" --type=merge -p '{"spec":{"paused":true}}'
# Use a preparation copy: keep replicas1 while installing the future template.
python3 - "$stage_patch" "$scale_receipts/prepare.json" <<'PYPREP'
import json, pathlib, sys
patch = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert patch['spec']['replicas'] == 0
patch['spec']['replicas'] = 1
pathlib.Path(sys.argv[2]).write_text(json.dumps(patch) + '\n')
PYPREP
fence_patch "$stage_service" "$scale_receipts/prepare.json"
k patch deployment "$stage_service" --type=strategic --patch-file "$scale_receipts/prepare.json" --dry-run=server -o name
outside_blackout
k patch deployment "$stage_service" --type=strategic --patch-file "$scale_receipts/prepare.json" -o name
# STOP: prove paused controller holds only old1; no new pod or shared mode change.
# Final pod-template/cache/node/rollback receipts ready; no prepared RS0 claim.
```

## S1/S2: proposed controller-owned old1->0, new0->2

S1 requires the complete retained-receiver exact image/challenge-duration floor
and negative tests **before** zeroing AM; EG stays at its proven state. S2 needs
S1 PASS plus every enabled provider/partition/console path's custody, shared
session/CSRF/refresh/logout and close-before-release proof. Both require explicit
owner outage approval (proposed03:15/04:15), source/image/cache/node readiness and
an exact compatible recovery branch. The pending order exception above must be
approved first; a zero-sized RS contains no running warm application.

```bash
# Individually approved payloads, with fresh controller identity/spec fences.
outside_blackout
k scale "deployment/$stage_service" --current-replicas=1 --replicas=0
# Observe old RS0 and all old processes/sockets stopped or safely revoked/expired.
# Pod deletion alone is not proof of no legacy effects; never force-delete.
# Only the qualified no-legacy-effects receipt permits the next operation.
outside_blackout
k patch deployment "$stage_service" --type=merge -p '{"spec":{"paused":false}}'
# Let the Deployment controller create newRS0, then verify exact owned template
# and both old/new desired0; no legacy ReplicaSet may restart.
outside_blackout
k scale "deployment/$stage_service" --current-replicas=0 --replicas=2
# Observe both exact-image pods independently serving, membership, Ready,
# canonical endpoints/NEG/backend health, distinct hosts and owner fencing.
```

Measure from loss of the last healthy legacy backend to restored successful
user service; record both-new-pods-ready separately. Target **<120s outage**.
At120s without restored service, abort promotion into the preapproved compatible
recovery branch. Do not wait35m for rollout then discover the missed outage gate.
An old process that cannot safely stop in time is a preflight failure; deadlines
never shorten leases/grace. Once new authority has been used, normal recovery
retains modeON and compatible image/receiver floor. No automatic legacy restart.

After **both** new pods/backends qualify, apply only that stage's PDB:

```bash
: "${stage_pdb:?infra/api/pdb.yaml or infra/cmaeg/pdb.yaml}"
k apply --dry-run=server -f "$stage_pdb" -o name
outside_blackout
k apply -f "$stage_pdb" -o name
k get deployment "$stage_service" -o custom-columns=DESIRED:.spec.replicas,READY:.status.readyReplicas,AVAILABLE:.status.availableReplicas
k get pdb "$stage_service"
```

Require readiness false before joined/renewed membership and immediately on drain;
liveness is separate. Each replica must independently serve; an idle EG member
may be ready without forced equal ownership. AM/EG CLI success is not proof of
all-owner handoff. Run the shared30-minute stable soak, then obtain lead S1/S2
PASS. S2 also requires provider canaries: every accepted event terminal/reconciled,
no unsupported blind replay, and the assigned24h observation handoff. No ON1
intermediate promotion gate is part of this cutover.

## S3: HPA activation and measured automatic up/down

Require S1/S2 PASS, verified Atlas>=30% headroom, the full quota/node/provider bound
and CPU qualification in the capacity document. The selected cadence and max5
must match reviewed tests. No rollout overlaps HPA qualification. Existing HPA
must be absent; otherwise capture/review its exact ownership before proceeding.

The release packet binds complete uncensored successful cohorts and the largest
qualified p95 from drain initiation through all-owner handoff AND successor
service. Missing timing HOLDs S3; p95 is never the hard client lifetime bound.
Use period120 if p95<=90s, else ceil(p95+30); values>1800 HOLD, never clamp.
Render and review both exact candidate specs, even when the initial120s remains:

```bash
: "${qualified_handoff_p95_seconds:?reviewed complete timing cohort required}"
ruby .github/scripts/cma-scale-manifest-contract.rb --hpa api "$qualified_handoff_p95_seconds" > "$scale_receipts/am-hpa.json"
ruby .github/scripts/cma-scale-manifest-contract.rb --hpa cmaeg "$qualified_handoff_p95_seconds" > "$scale_receipts/eg-hpa.json"
k apply --dry-run=server -f "$scale_receipts/am-hpa.json" -o name
k apply --dry-run=server -f "$scale_receipts/eg-hpa.json" -o name
outside_blackout
k apply -f "$scale_receipts/am-hpa.json" -o name
k get hpa cma-agent-manager -o jsonpath='{.spec}{"\n"}{.status}{"\n"}'
# Require usable fresh metrics/conditions and stable idle behavior before EG.
outside_blackout
k apply -f "$scale_receipts/eg-hpa.json" -o name
k get hpa cma-event-gateway -o jsonpath='{.spec}{"\n"}{.status}{"\n"}'
```

Using separately approved load, observe real metric-triggered2→4→5 for each
(Max of+2/+100% per60s, up stabilization0),
then together after capacity proof. A manual replicas patch does not prove HPA.
Independent correctness also covers N3. Reduce only owned load; observe the
full300s down stabilization and -1 per qualified period (initial120s),
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
zero healthy backends outside the approved cutover, >30s sudden-loss service gap,
no restored service by120s inside the approved cutover, incompatible receiver,
OOM/repeated crash, breached process/provider bounds or Atlas headroom<30%.
Performance regression beyond preregistered noise/absolute limits aborts after
two consecutive five-minute windows; widespread failures abort immediately.
Missing evidence or low samples HOLD promotion. Unknown/Partial stops promotion
and pages the lead; reconcile within the approved observation window or invoke
compatible recovery. Continue assigned24h post-go-live monitoring with these
triggers even beyond15Nov13:00.

**S3 rollback first freezes HPA, then removes only S3-created objects:**

```bash
outside_blackout
k patch hpa cma-agent-manager --type=merge -p '{"spec":{"behavior":{"scaleUp":{"selectPolicy":"Disabled"},"scaleDown":{"selectPolicy":"Disabled"}}}}'
outside_blackout
k patch hpa cma-event-gateway --type=merge -p '{"spec":{"behavior":{"scaleUp":{"selectPolicy":"Disabled"},"scaleDown":{"selectPolicy":"Disabled"}}}}'
# Read back both Disabled policies; each delete needs its own approval.
# Reviewed tool supplies canonical URI and DeleteOptions with UID/resourceVersion
# of the exact S3-created HPA, after a server dryRun delete. Name alone is unsafe.
: "${am_hpa_delete_uri:?reviewed canonical URI required}"
: "${am_hpa_delete_options:?reviewed UID/RV DeleteOptions required}"
: "${eg_hpa_delete_uri:?reviewed canonical URI required}"
: "${eg_hpa_delete_options:?reviewed UID/RV DeleteOptions required}"
outside_blackout
k delete --raw "$am_hpa_delete_uri" -f "$am_hpa_delete_options"
outside_blackout
k delete --raw "$eg_hpa_delete_uri" -f "$eg_hpa_delete_options"
# Verify absence and stable desired counts; an in-flight controller write may settle late.
```

Keep a safe serving count for current demand. Descend **one at a time**, separately
approved, with expected current replicas to detect a race:

```bash
: "${rollback_service:?canonical AM or EG deployment required}"
: "${current_replicas:?fresh observed serving count required}"
case "$rollback_service" in cma-agent-manager|cma-event-gateway) ;; *) exit 1 ;; esac
test "$current_replicas" -gt 2
# Reviewed controller MUST freshly recheck Ready/Available/observedGeneration,
# zero previous terminations and survivor capacity immediately before dispatch.
# A Deployment spec/replica fence alone does not fence pod/status changes.
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

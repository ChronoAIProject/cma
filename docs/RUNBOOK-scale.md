# Local cma-scale environment

Owner: local-env-manager-02. Workspace: `/Users/shining/code/work/tech-lead.staff/local-env-manager-02`.
Track: ChronoAIProject/cma #1356, #1360, #1541; acceptance suite #1957; HPA/PDB handoff #1956.
Environment delivery: #1970. Repository scripts live under
`.github/scripts/cma-scale-local/`; the committed runbook is `docs/RUNBOOK-scale.md`.

**Status: local AM×2, EG×2 and Mongo×3 are running; HPA and candidate acceptance gates are pending.**
The initial integration checkout is `d6fc953a6cb43f8d9c90337b4e4ce17ecb291fbc`.
Do not label measurements from it as verification of developer-13's forthcoming refreshed candidate.
Read `evidence/deployment-ledger.jsonl` and `evidence/standing-duty-state.json` for live
source, OCI revision, Docker config ID, running CRI image digest and readiness.
The Docker config ID and CRI digest are different identities; do not equate them.
At 2026-11-14 00:30 SGT, a rollout initiated outside this turn had replaced both
images with separate amd64 builds from the same source. AM/EG remain 2/2 with
zero restarts, and three TLS readiness probes per service returned 200. The
00:22 native-arm64 observation remains historical in the ledger. Mongo is 3/3;
metrics-server was verified 1/1. No authenticated journey baseline is active.

| Service | Current image | Running CRI image digest |
|---|---|---|
| AM | `cma-scale-api:amd64-d6fc953a6cb4` | `sha256:9036f557b2bf9f100d1c2b9ca6fccdadc2c9b5c3015476a9ae750fdc33874f9f` |
| EG | `cma-scale-eg:amd64-d6fc953a6cb4` | `sha256:4621e1eebcf404fa409ad58aea89a3eb2d6d6ff763d9952da20fd6aa4a97c1d0` |

Placement at 00:30 is degraded: both AM pods share one worker, and both EG pods
share the other. Existing service-wide spread constraints do not rebalance after
old rollout pods disappear. Earlier placement and native-arm64 measurements do
not qualify these images or this topology. A proposed renderer correction adds
`matchLabelKeys: [pod-template-hash]` to both host-spread constraints so every
rollout revision is spread separately. The generated constraints passed Kubernetes
server dry-run against both current Deployments at 00:37 SGT. See
`evidence/placement-server-dry-run.json`; the exact current-image projection is
`private/placement-proposed.json`. This proposal is not installed. It needs the
exact-source reviewer verdict, a fresh unchanged-template check and a controlled
rollout with post-rollout worker-hash verification before placement can pass.
The active generated `manifests/apps.json` is deliberately not overwritten by
this code change, because the peer helper uses that file during build/load.

## Current deadline and controlling gate

Latest performance ruling: architect A3 supersedes universal noise-only tolerance for explicitly classified noncritical operations. CRITICAL create/open/send/interrupt/usage-first-render and their awaited dependencies stay noise-only. UX-AFFECTING permits +10% p95 maximum (aim≤5%); NON-SENSITIVE permits +30ms p95. Keep the upper-confidence-bound method and do not add noise to those caps. All absolute owner targets remain. Activity API p95<1s is distinct from client page completion, whose absolute ceiling is unspecified. Bind class/context before comparison; no blanket allowance or result-driven relabeling. Complete rule/table bindings are in `evidence/deadline-followup-20261114/receipt.json`; final class JSON SHA256 `effce4365936c4029b2e9b2c27edeec05c396d918d3e36ea806e17e40c1823f9`, addendum SHA256 `aae62b1a3cd97c94aac11db29a474b2301af1815c57d145d8af0a8984ed7953b`.

Developer17 source now implements `--scenario baseline-n1`; the selector-availability blocker is resolved. A working reviewed adapter with safe credential loading, actual runtime/browser/provider/oracle fixtures and exact candidate/configuration bindings is still required. The owner inspected source only; no suite was executed and no cohort was reserved.

The owner-set final deadline is **15 November 2026 at 18:00 SGT**, with all performance criteria, multi-replica behavior and autoscaling verified in production. The lead plan is `/Users/shining/code/work/tech-lead/notes/2026-11-14-hard-deadline-plan.md`; the controlling 25-gate document is `/Users/shining/code/work/tech-lead.staff/solution-architect-04/essentials.md`. The full matrix and old mandatory repetition counts are best-effort and do not independently block. Existing relevant failures still require closure.

Environment owner local-env-manager-02 supports M2 integration by **14 Nov 22:00** and M3 local essentials by **15 Nov 01:30**: current source/image provenance, healthy isolated services, reviewed placement/PDB, safe resources and coordinated N1/N2/2→4→2/owner-kill execution support. The local bounded scale sequence replaces the old mandatory local max5 sequence; production still requires both HPAs min2/max5 and verified automatic up/down. Local survivor, terminating-process, runtime and Mongo capacity still need evidence.

Production milestones belong to the deployment/verification owners: batch9 N1/OFF03:00–05:00, S1AM2 at06:00, S2EG2 at08:30, S3HPA at10:30, all performance verification13:00–17:00, final18:00. No production writes02:00–02:59. This local owner has no production authority. Every owner target holds and any regression stays within preregistered measured noise; create≤5s, tab/send/interrupt≤1s, usage≤2s, plus Activity usability and its existing performance boundary. No new Activity ceiling is invented.

At14Nov15:27SGT the live cluster still ran `d6fc953a6cb43f8d9c90337b4e4ce17ecb291fbc`: AM/EG2/2 Ready with zero restarts, Mongo3/3, no application HPA/PDB, and per-service co-location. The owned checkout was fast-forwarded to pushed integration `0ac76c70f7a5c6f18d7c38f0b8266adece125b8f`. This is prepared source, not a deployed candidate.

The current native helper SHA256 is `477fd97f90fe103e042e732c8ae1c3b6c258622d978c5eca09c66acc8353454c`; previous e9235518 references below are historical. Its complete current script was read. Exact helper clearance and the working local zig/cargo-zigbuild prerequisite/PATH handoff remain absent. The env01 directed request was denied at15:27; report BLOCKED: URGENT to the lead instead of bypassing grants or treating fixed-mode clearance as helper authority.

Developer16's manifest source `9cbc36204bf052de5aa92efc2bf42e533c8427e3` received conditional delivery PASS and was merged at `d38966aab5c77fa2262da2d14ffba0da84cd958c`; the four HPA/PDB files are byte-identical in current integration. This supersedes earlier c011/840 source-review states below. The local namespace-only projection preserves max5 and A2 policy and passed server dry-run. Fresh placement patches also passed server dry-run with resourceVersion preconditions and retained current images. None was applied; source/merge approval does not certify the local operation, actual placement or HPA capacity.

Concrete preparation is retained under `evidence/deadline-local-preparation-0ac76c70/`: source/review bindings, namespace projection, fresh placement and HPA/PDB dry-run receipts, capacity arithmetic, helper preflight and owner milestones. At four processes/service, app+Mongo+routing/toxiproxy declarations request2.35CPU/3.1875GiB and limit5.8CPU/6GiB before runtime/Jobs/system overhead. This is not a survivor or performance qualification. EG's200%×250m target equals its500m limit; a750m limit is a review proposal to preserve the request/target while adding headroom, not an applied change. A reviewed capacity revision is still needed for repeated autoscaling and terminating overlap.

No measurement slot is reserved until concrete approved adapters, independent outcomes and the reviewed N1-only driver are supplied. The ordinary-work recovery slice with developer02/debugger03 remains explicitly assigned, but its local fixture/control binding is still required. Continue independent source/manifest/preparation work while blocked and report the exact critical input as BLOCKED: URGENT. The lead owns monitoring and mutation precedence.

## Isolation and capacity

Use **kind v0.30.0 / Kubernetes v1.34.0 with arm64 nodes**. Docker Desktop's VM is arm64,
8 CPUs / 16 GiB; the Mac has 18 CPUs / 128 GiB and approximately 1.5 TiB free.
Kind gives this task a disposable cluster with its own kubeconfig, independently
bounded node containers, and two worker scheduling domains without enabling or
reconfiguring Docker Desktop Kubernetes. The initial application images were
native arm64. The current amd64 images run through the Docker Desktop host
translation path; qualify new matched baselines and record architecture. Native
compilation does not imply native execution on these nodes.

| Component | Count | CPU request / limit each | Memory request / limit each |
|---|---:|---:|---:|
| Kind control plane | 1 | Docker cap 2 CPUs | Docker cap 3 GiB |
| Kind worker | 2 | Docker cap 2 CPUs | Docker cap 3.5 GiB |
| AM | 2; A2 requires qualification through 5 | 250m / 500m | 256 MiB / 512 MiB |
| EG | 2; A2 requires qualification through 5 | 250m / 500m | 256 MiB / 384 MiB |
| Mongo | 3 | 100m / 500m | 384 MiB / 768 MiB |
| Nginx / Toxiproxy | 1 each | 25m / 200m; 25m / 100m | 32 MiB / 128 MiB each |
| Metrics server | 1 | 100m / 200m | 200 MiB / 256 MiB |
| Sandbox controller | 1 | 50m / 250m | 64 MiB / 256 MiB |
| k6 | on demand | Docker cap 0.5 CPU | Docker cap 256 MiB |
| Rust compilation | on demand | Docker cap 2 CPUs, 2 jobs | Docker cap 5 GiB |

The old max 4 projection does not qualify the A2 max 5 requirement. The namespace quota bounds requests to 4 CPUs/6 GiB, limits to
9 CPUs/9 GiB, storage requests to 12 GiB and pods to 24. The node containers are a
second, physical 6 CPU/10 GiB ceiling. Kind reports the Docker VM's nominal
8 CPU/16 GiB capacity on each node despite these cgroup limits; budget against
the Docker caps and namespace quota, not the nominal node percentages. Do not run load tests during builds or other
teammates' resource-intensive gates. Inspect actual load before each cohort.
The two workers share one Mac and one Docker VM: this tests pod/node-container
loss, not independent physical-machine or zone failure.

```sh
cd /Users/shining/code/work/tech-lead.staff/local-env-manager-02
docker stats --no-stream --format '{{.Name}} {{.CPUPerc}} {{.MemUsage}}'
scripts/k top pods
scripts/k get resourcequota local-budget
```

All Kubernetes commands use `private/kubeconfig`, context `kind-cma-scale` and
namespace `cma-scale`. `scripts/k` rejects context/server overrides, pod exec,
attach, copy, arbitrary raw URLs and unrelated kubectl commands. The user's
default kubeconfig is untouched and currently names production; **never use a
bare mutating kubectl command**. No production access, Secret reads or pod exec
are needed. Credentials are generated locally, kept under mode-0700 `private/`
with mode-0600 secret files (the public TLS certificate is mode 0644), and injected without printing values. Do not send that
directory, whole pod specs, environment dumps or raw application logs to peers.

## Ports and routing

| Loopback endpoint | Purpose |
|---|---|
| `127.0.0.1:16443` | Isolated Kubernetes API; client credentials stay private |
| `127.0.0.1:18080` | Nginx TLS NodePort load balancer |
| `127.0.0.1:18474` | Temporary Toxiproxy port-forward, only during `fault.sh delay` |

No `/etc/hosts` modification is needed. Use these HTTP Host headers:

- `am.cma-scale.test`: all paths go to AM, analogous to the API hostname.
- `eg.cma-scale.test`: all paths go to EG, analogous to the gateway hostname.
- `cma-scale.test`: AM `/api`, `/auth`, discovery routes, `/api/agent-manager` →
  `/api`; EG `/event-gateway/{v1,mcp,openapi.json,docs}` strips the
  `/event-gateway` prefix; connect/legacy-console paths are preserved.

Production routing was checked **in the integration source**, at
`infra/{api,cmaeg}/{service,httproute,gcpbackendpolicy}.yaml`. Neither Services nor
GCP backend policies enable affinity. The local load balancer uses round-robin
over pod DNS records, no cookies/stick tables and Services with `sessionAffinity:
None`. WebSocket upgrade and unbuffered streaming are enabled; upstream timeouts
are 660 seconds. Automatic upstream retries are disabled so a failure remains
visible to the acceptance suite. The `X-Scale-Upstream` response header identifies
the selected local backend; hash it before retaining or reporting provenance.

Differences: local self-signed TLS, no GFE/Cloudflare, no frontend or Image Manager,
5-second DNS refresh rather than GKE NEG propagation, smaller CPU/memory limits.
Unknown routes return 404. Do not infer live production affinity from repository
configuration alone; the deployment owner must attest any production drift.

```sh
curl --cacert private/tls.crt --fail https://127.0.0.1:18080/lb-health
curl --cacert private/tls.crt --fail -H 'Host: am.cma-scale.test' https://127.0.0.1:18080/ready
curl --cacert private/tls.crt --fail -H 'Host: eg.cma-scale.test' https://127.0.0.1:18080/readyz
curl --cacert private/tls.crt --fail -H 'Host: cma-scale.test' https://127.0.0.1:18080/event-gateway/openapi.json -o /dev/null
```

## Reproduce infrastructure

Use the existing `source/` checkout; do not make a second worktree. GitHub/git must
use active account `chronoai-shining`. If another account is active, stop and
report it without switching. The lead owns worktree cleanup and candidate
integration. Do not rebase, force-push, poll CI or modify production.

The delivered bundle is already installed in the workspace above. To refresh it
from a lead-integrated checkout, copy `.github/scripts/cma-scale-local/scripts/`
and `manifests/` into this workspace, and copy `docs/RUNBOOK-scale.md` here. Keep
`source/` on `tl/cma-am-replicas/integration`; the build script enforces that branch.
The bootstrap commands generate infrastructure/private state, fetch checksum-pinned
add-ons and never require global package installation.

```sh
gh auth status --active
mkdir -p bin cache manifests evidence private
chmod 700 private
curl -fsSL https://github.com/kubernetes-sigs/kind/releases/download/v0.30.0/kind-darwin-arm64 -o bin/kind
curl -fsSL https://github.com/kubernetes-sigs/kind/releases/download/v0.30.0/kind-darwin-arm64.sha256sum -o cache/kind.sha256sum
python3 - <<'PY'
from pathlib import Path
import hashlib
assert hashlib.sha256(Path('bin/kind').read_bytes()).hexdigest() == Path('cache/kind.sha256sum').read_text().split()[0]
PY
chmod +x bin/kind scripts/k scripts/*.sh
scripts/cluster-up.sh
python3 scripts/render-infra.py
scripts/k apply -f manifests/infra.json
scripts/k apply -f private/secrets.json
scripts/k rollout status statefulset/mongo --timeout=180s
python3 scripts/check-mongo.py
scripts/install-addons.sh
scripts/tls.sh
python3 scripts/render-routing.py
scripts/k apply -f manifests/routing.json
scripts/k rollout status deployment/routing --timeout=120s
scripts/k rollout status deployment/toxiproxy --timeout=120s
```

MongoDB 8.0.13 uses a three-member `rs0`, keyfile membership authentication,
separate application accounts restricted to `cma_scale`, a `clusterMonitor`
probe account, 0.25 GiB WiredTiger cache/member, 128 MiB oplog, and 3 GiB PVC/member.
The bootstrap script uses the localhost exception only inside mongo-0 and then
enables authenticated user setup. Applications use all three hosts and a pool
cap of 10 per server. `check-mongo.py` uses a disposable Job; its output contains
only primary/secondary/healthy counts. A local PDB requires two Mongo members.
Volumes survive pod restarts and disappear with the disposable cluster.

Metrics server v0.8.0 uses `--kubelet-insecure-tls` only in this isolated kind
cluster because kind's kubelet certificates are not production certificates.
The upstream sandbox controller v1.0.2 is installed for the real API required by
AM startup. It is not the patched production controller, and no runtime pools
or production resources are imported.

## Build and deploy the application candidate

The commands in this section describe the initial successful Docker-native build.
For subsequent integration pushes the lead requires local-env-manager-01's
build-and-load helper; follow the standing-duty handoff below after its exact
review and prerequisites are supplied. Do not silently substitute a build path.

Obtain developer-13's ready SHA and the complete applicable review verdict first.
Refresh the existing integration checkout only when no build is reading it; the
lead/integrator merges develop into that branch. The local manager does not
resolve integration conflicts or change its history.

```sh
gh auth status --active
git -C source fetch origin tl/cma-am-replicas/integration
git -C source merge --ff-only FETCH_HEAD
git -C source rev-parse HEAD
scripts/build-arm64.sh > evidence/build-arm64.log 2>&1
scripts/package-arm64.sh > evidence/package-arm64.log 2>&1
scripts/k get deployments
scripts/k get endpointslice
```

Fresh EG storage is initialized by `scripts/migrate-eg.py` through the same
application CLI in a disposable Job; this is local-only database setup.

The build uses pinned Rust 1.96.1, `--release --locked`, native Linux arm64,
unchanged application source, exact source/build identity and recorded binary
SHA-256s. Packaging reuses the source's pinned Codex/Gitleaks build stages and a
task-owned BuildKit worker with 2 CPU/3 GiB limits. Both binaries share one local
image, loaded directly into kind. Nothing is pushed to a registry. EG's media
transformer is amd64-only; it is disabled here, and media-transformer acceptance
is outside the evidence provided by this arm64 environment.

AM ownership mode is enabled only for this disposable local scope. N=1 baseline
keeps the same ownership protocol enabled, source, image, configuration and
database, avoiding a mixed-mode transition. No production feature activation is
authorized. AM relay addresses come from each pod's IP. AM uses a namespace-only
service account, without Secret or pod-exec grants; EG has no service-account
token. Containers are non-root with read-only root filesystems and dropped
capabilities. Production-like drain hooks/grace periods are retained.

**Application fixture gap:** AM and EG retain provider authentication with separate generated synthetic
client credentials; identity/provider origins point to `.invalid`, EG has no real channel providers,
and runtime pools are zero. The native home policy uses kind's `standard` storage
class with a 1 GiB per-home bound; it creates no homes during bootstrap. The source's pinned runtime reference is retained as
configuration identity but is not pulled or activated. This permits control-plane
bootstrap and health/routing checks; it does not constitute a usable browser,
agent execution or provider-delivery fixture. Developer-17/local-verifier-02 must
provide the approved local fixture adapters and runtime image before those gates
run. No production credentials may be substituted.

## Baseline, replicas and autoscaling

The architect's corrected-control verdict at PR #1972 commit `d03931a4b1e59fdf2de73f6e9228d515d6fb705f` is GO only for owner-operated fixed-1/fixed-2 in this existing environment under a coordinated stable window and normal build-slot/resource conditions. The complete verdict SHA256 is `bf0e6440b3bc196b207318eb7d2664c4045a47cc7cf3f240817d703620eda103`. On 2026-11-14 at 01:40 SGT, the owner installed and hash-verified the exact approved `k` and `mode.sh` blobs. The actual before/after/source receipt is `evidence/control-installations/d03931a-20261113T174055.229737Z/installation.json`. This scoped GO supersedes the prior P1/P2 source NO-GO; no live control or baseline was run during installation. Fixture/driver prerequisites remain missing and no N=1 measurement slot is reserved. Other operations and whole-PR merge readiness remain outside this verdict.

The installed reviewed wrapper permits only the short output/file/selector forms used by this bundle (`-o`, `-f`, `-l`, including `=value`) and the standalone stdin marker `-`. Other short or combined flags fail before kubectl starts, including `-nreview-other`, `-shttps://review.invalid:443`, `-A=true` and `-wnreview-other`. Long destination/namespace/authentication overrides remain rejected. This confines arguments; it does not authorize Secrets, logs, cluster-scoped objects, unrelated resources or arbitrary accepted commands. Requested execution scope remains owner-operated fixed-1/fixed-2 in the existing local namespace during an agreed stable window.

Each corrected fixed-mode attempt creates a separate private directory under `evidence/mode-transitions/fixed-N-<unique suffix>/`. Its files are:

- `intent.json`: UTC start, context, namespace, loopback endpoint, mode/command and exact control hashes.
- `hpa-before.json` and `deployments-before.json`: scoped checkpoints written once before any mutation, including hashed resource UIDs and resource versions. HPA metadata annotations are omitted; this evidence is not a restore/apply manifest. Only HPAs targeting the two named Deployments are selected.
- `events.jsonl`: append-only command start/outcome journal with exit codes and stdout/stderr hashes. Unrestricted output and error payloads are not retained.
- `pods-after.json`: written only after exactly N nonterminating Ready pods are observed per service; retains hashed pod/node identities and runtime image IDs. This does not establish placement or user-journey correctness.
- `result.json`: completion or failure phase and exit code. A killed process may leave an incomplete attempt without this file; that is never success.

A failed initial snapshot aborts before mutation. HPA deletions and each Deployment scale run separately, stopping on the first failure while preserving earlier successful effects in the journal. No automatic rollback or HPA restoration occurs. A retry creates another directory and cannot replace a previous attempt or the legacy `evidence/hpa-before-fixed.json`. Keep all failed and incomplete attempts when handing off results.

Offline validation from the repository checkout uses only a synthetic kubectl and synthetic kubeconfig in disposable directories under the bundle:

```sh
python3 .github/scripts/cma-scale-local/tests/test_controls.py -v
```

The tests cover override rejection, the pinned destination, checkpoint preservation, snapshot errors, both HPA deletion failures, either Deployment scale failure, rollout failure, retry preservation and termination exclusion. They make no real Kubernetes/network calls, install no tooling and clean their fixture directories. Their source-bound receipts support the focused review; they do not approve live control use. The `auto` branch, HPA installation, faults, native helper and production operations remain outside this control review.

```sh
scripts/mode.sh fixed-1
scripts/load.sh n1-health
scripts/mode.sh fixed-2
scripts/load.sh n2-health
scripts/mode.sh auto
scripts/k get hpa,pdb,deployments
```

`fixed-1`/`fixed-2` remove only HPAs targeting these two deployments before scaling
and wait for terminating pods to disappear. Ownership mode remains enabled.
Only owner-operated `fixed-1`/`fixed-2` are covered by the installed corrected-control verdict above: mode SHA256 `6f7492597b13857874972de56b1318a7b5697fe8a38c101a12f725deac35c103`, wrapper SHA256 `827aa3a61c717fb1076bd8f5456778e82420eaa96b6c8536483740e3c9527082`. Coordinate a stable window after the real driver and fixtures are ready. The displayed load, auto/HPA and general-wrapper commands retain their separate execution gates. Historical runs do not grant those scopes; do not use the old 27ee2b7b/f840846e controls.

The joint developer-17/local-verifier-02 N=1 request is queued, not active. Do not reserve a slot until real adapters, independent outcomes and the reviewed N1-only driver are available. After those prerequisites and stable-window coordination, record the actual cohort start/end and source/configuration hashes; the environment owner runs `scripts/mode.sh fixed-1`, waits for exactly one Ready pod per service and no terminating pods, and hands off that observation. Do not change the candidate, configuration, resources or ownership-enabled flags during matched cohorts. The lead resolves a conflict between an active cohort and a newly pushed candidate.

`auto` fails until `manifests/hpa-pdb-reviewed.json` exists. The current code also rejects maxReplicas above four; **that guard must be revised and reviewed with a sufficient resource envelope before A2 activation**. Do not set an `approved` annotation yourself to bypass a missing verdict. The older `manifests/hpa-pdb-proposed.json` is an unapplied, superseded A1 projection and must not be used for A2 acceptance.

Developer-16's corrected manifest source is `9cbc36204bf052de5aa92efc2bf42e533c8427e3`, merged at `d38966aab5c77fa2262da2d14ffba0da84cd958c`. The complete architect verdict retained in `evidence/deadline-local-preparation-0ac76c70/manifest-source-verdict.md` (SHA256 `dcd51a2d640b6a7fda7bc4ff95ac69f730b90e51a2c0f89169d71e48086684f3`) gives conditional source/preparation PASS. All four current integration HPA/PDB files match that reviewed source. Local execution and capacity gates remain HOLD. This supersedes the c011/840 source findings. Metrics-server is ready; application HPA/PDB remains absent. The current policy requires:

- Both services min 2 / max 5; up stabilization 0s, Max(2 pods, 100%)/60s.
- Down stabilization 300s, one pod/120s with Min selection initially. If qualified drain/handoff p95 exceeds 90s, extend the period to ceil(p95+30s). Missing timing remains a hold; p95 is not a hard process lifetime.
- Preserve reviewed CPU targets, application PDB minAvailable 1 and AlwaysAllow, and record every namespace/resource override. No HPA/rollout overlap; wait for previous terminations before a rollout.

The present local resource envelope cannot qualify the conservative A2 process bound. If an effective lifetime G = 760s is proven with P = 120s, ceil(G/P) = 7 terminating plus 5 active means 12 processes/service. Application pods alone then request 6 CPU / 6 GiB, limit 12 CPU / 10.5 GiB and consume 24 pods, exceeding current quota before Mongo/routing/runtime/daemon overhead. The two workers are capped at 4 CPU / 7 GiB total. A reviewed capacity revision is required; lowering max 5 to four does not satisfy the requirement. The assumption G = 760s itself needs evidence.

The provisional EG target 200% of its 250m request equals the local 500m limit, so reliable scale-up cannot trigger. The current review proposal preserves the 250m request/200% target and raises only the EG CPU limit to 750m; the older 125m-request proposal is withdrawn. The 750m limit has not been applied. Any revised resources must be identical in all matched cohorts, followed by new baselines. Preserve original source SHA and file hashes for the final localized manifests.

For the current M3 gate, qualify local 2→4→2 behavior for each service; production K25 separately requires min2/max5 and automatic growth/return to two. Earlier local max5 trial requirements are superseded by the hard-deadline ruling. Manual `kubectl scale` is not autoscaling evidence. Include the reviewed 300s stabilization, measured drain/handoff period and actual termination horizon in the observation window. Watch Mongo sockets, worker requests/limits, throttling, OOM, quota and Pending pods. Stop a load run on pressure; do not lift quotas without a revised budget. PDBs govern eviction, not forced deletion or Deployment scale-down.

## Load and failure tools

`scripts/load.sh` runs bounded k6 v1.3.0 and writes only aggregate measurements.
The supplied `smoke.js` measures readiness round trips at low rate. Its p95s are
**not** agent-create, tab-open, send, interrupt, usage, or batch-2/3 latency gates.
Use the reviewed #1957 suite for those operations and its fixture reconciliation.

```sh
DURATION=60s scripts/load.sh steady-health
# In a second own terminal, while the real fixture/load suite is running:
scripts/fault.sh kill am
scripts/fault.sh kill eg
# Adds a 150 ms TCP downstream delay for 30 seconds, then removes it on exit.
scripts/fault.sh delay am 150 30
# While that command is active, opt into the delayed path:
FAULT=delay DURATION=20s scripts/load.sh delayed-health
```

Pod loss deletes one running application pod with zero grace only when at least
two replicas are Ready. It deliberately bypasses graceful drain. Retain failures
from the fault window and independently reconcile durable outcomes; successful
rollout replacement alone proves neither RTO nor zero lost/duplicate work.

Network delay uses unprivileged Toxiproxy, with a temporary loopback admin port.
Only requests with `X-Scale-Fault: delay` take that path. Baseline traffic remains
on the direct load-balancer path. The experiment currently delays LB→AM or LB→EG;
it does not partition Mongo, delay AM peer relay, or simulate lost network packets.
For numeric delay comparison use the same proxy path with zero delay as the
control so the extra proxy hop is not confused with the injected latency.

## Evidence and handoff gates

Record source SHA, image/binary digests, UTC/SGT window, local resource configuration,
fixture hash, workload hash, flag values and sample counts for every cohort.
Hash replica/receipt/operation identifiers; retain no raw identities, credentials
or request/response bodies. Repeat N=1 cohorts to estimate noise, then compare
matched N=2 and scaling cohorts. Report p50/p95, failures and absolute/relative
deltas; only report p99 when the relevant sample count is at least 1000. Required
user gates remain create/open/send/interrupt ≤1 second, usage ≤2 seconds, plus the
specified batch-2/3 p95s, and no regression beyond measured baseline noise.

Current environment observations (these are not user-journey acceptance):

| Observation | N=1 | N=2 / fault |
|---|---:|---:|
| Health checks passing | 60/60 | 60/60 |
| AM readiness p95 | 0.826 ms | 0.893 ms |
| EG readiness p95 | 0.790 ms | 0.618 ms |
| Abrupt AM/EG loss health checks | — | 126/128; zero-error threshold FAIL |
| Network delay same-path mean | 5.82 ms control | 156.71 ms with 150 ms injection; 5.00 ms after cleanup |

The 30-second health cohorts have only 30 observations per service and do not
estimate measurement noise or prove no regression. No durable-work recovery or
user-level RTO was measured. Both deployments and all Mongo members recovered
healthy after the drills. Application HPA/PDB remains unapplied.

Current evidence:

- `evidence/kind-checksum.txt`: verified upstream CLI checksum.
- `evidence/mongo-health.json`: three members, one primary, two secondaries, all healthy.
- `evidence/build-arm64.log`: successful native release build (23m 59s); not acceptance evidence.
- `evidence/routing.json`: 20/20 HTTP 200 per service, 10 requests to each of two hashed backends; both discovery routes also returned 200.
- `evidence/eg-migrations.json`: local schema initialization passed.
- `evidence/{n1-health,n2-health,sudden-loss-health}/summary.json`: k6 aggregate outcomes, including the failed fault threshold.
- `evidence/network-delay.json`: same-path control, injected and restored latency.
- Five wrapper-negative checks rejected context/server overrides, raw URLs and pod exec.
- Nginx TLS `/lb-health` returned 200 with the generated certificate verified; Toxiproxy's timed toxicity create/delete succeeded.

Outstanding before full-task DONE: refreshed integration candidate,
developer-16's exact HPA/PDB handoff and verdict, real fixture adapter handoff,
observed N=1/N=2 and HPA/fault gates, and delivery to all requested peers.
The original runbook reached local-verifier-02 at 00:07 SGT on 2026-11-14.
The 00:24 SGT baseline follow-up failed or is uncertain because that recipient
was WORKING; it was not automatically resent. A new material rollout notice at
00:32 SGT was delivered using the guarded command's authorized `--force` queue.
Developer-17 and debugger-03 directed routes were denied at 00:33 SGT. The other original recipient routes also remain unresolved; retain
exact delivery receipts and ask the lead to relay or repair grants.
Missing prerequisites are BLOCKED, never PASS.

## Standing-duty build and fixture handoff

The lead owns the monitoring loop. This workspace has no background watcher and does not independently monitor/poll GitHub CI. At each lead tick or pushed-head handoff, compare `git -C source ls-remote origin refs/heads/tl/cma-am-replicas/integration` with the live ledger after `gh auth status --active` confirms `chronoai-shining`. Build only the integrator's exact pushed SHA, including EG topic changes when integrated. Never switch the active GitHub account, rebase or force-push.

Required helper: `/Users/shining/code/work/tech-lead.staff/local-env-manager-01/build-cma-scale-native-load.sh`, current SHA256 `477fd97f90fe103e042e732c8ae1c3b6c258622d978c5eca09c66acc8353454c`. Earlier e9235518 observations are historical. Its complete exact-version review verdict has not been supplied. It requires `cargo-zigbuild`, `zig`, the matching musl Rust target and the shared build-slot wrapper. The complete lead ruling in `/Users/shining/code/work/tech-lead/team/briefs/developer-08/20261114-001147-send.md` was read and its installed SHA256 `ba6faed78c26f961ec642667942d090e2425669650c6ec0dd2402dd62e659940` matched. That ruling clears this exact build-slot version for all cma-scale work; it does not clear the native helper, mode script or kubectl wrapper. Normal resource checks remain required and no priority lane was granted. In this shell the first two are absent and only `aarch64-apple-darwin` is installed. The peer's arm64 attempt failed with `E0463: can't find crate for core` for `aarch64-unknown-linux-musl`; its separate amd64 build subsequently loaded and rolled out successfully around 00:25 SGT. The manager-02 shell still needs the approved prerequisite/PATH handoff to reproduce that build. Do not perform a global toolchain install to bypass it.

Before the next deploy, obtain a reviewed prerequisite/PATH setup, isolate the helper's temp/cache output under this workspace (it creates temp files beside itself), use a bounded task-owned BuildKit worker, and use the explicitly cleared shared build slot. Pin a local copy to the approved helper hash. Supply `CHRONO_BUILD_SHA` and `CHRONO_BUILD` to native compilation, not only Docker packaging. The helper currently applies a full generated apps manifest and waits for exactly two replicas; reconcile current configuration/fixtures first, exclude HPA/rollout overlap and a running baseline, and do not overwrite newer environment state. The helper also needs a verified termination-aware rollout timeout; the current 300s wait can be shorter than the 760s AM grace. These are review inputs, not a claim that the helper is ready.

After a successful new rollout, record each service's source SHA, OCI revision, binary hashes, Docker config ID, actual pod `status.containerStatuses[].imageID`, configuration hash, replica readiness and timestamps in `evidence/deployment-ledger.jsonl`. Confirm both TLS readiness endpoints, Mongo quorum and metrics health, preserving failed checks. Only then notify local-verifier-02, developer-17 and debugger-03 with that exact identity via granted `team.sh peer-send`. A message delivery failure is a handoff blocker; it does not turn a rollout into a failed build. Do not automatically retry a delivery reported as uncertain.

No operational user/browser fixture authentication is available yet. Generated local service client secrets are not user login credentials. Supply approved local identity/provider adapters, runtime/frontend artifacts and fixture claims/grants before authenticated journeys. The reviewed harness should consume short-lived auth through an ignored mode 0600 file or mounted secret, with no credential values in argv, logs, evidence or peer messages. Do not disable authentication or import production credentials. `handoffs/baseline-standing-duty.txt` records the exact current gap and tool hashes; it is a handoff, not a completed baseline reservation.

## Repository review and current handoff

The 15:49 SGT read-only candidate/configuration snapshot is `evidence/deadline-followup-20261114/current-candidate-inventory.json`; it confirms the same running digests, AM/EG 2/2 and namespace metrics coverage 9/9. The subsequent `capacity-inventory.json` adds all 24 local-cluster pods/metrics, node allocatable, init policies, quota and current HPA/PDB state. Kubernetes advertises 8 CPU/about 15.6 GiB **per kind node**, while Docker enforces only 2 CPU/3.5 GiB per worker (2 CPU/3 GiB control plane). Use the enforced limits and system overhead for capacity review; the advertised allocatable values overstate available per-node resources. These idle snapshots do not establish survivor, runtime/provider or Mongo headroom. Collector source and exact read-only query exits are retained with the evidence.


The environment is tracked by #1970. The scoped source bundle and operational
runbook are in draft PR #1972 into `tl/cma-am-replicas/integration`, so the
lead can review/integrate them with #1370. This draft is not MERGE READY: HPA/PDB
approval, the refreshed application candidate, live acceptance fixtures and
guarded peer delivery remain outstanding. Local syntax, server dry-run, secret
scan and the running-environment checks above are the evidence; CI is not polled.

## Cleanup and ownership

Keep the running cluster and private credentials while verifier/QA use the
environment; those are active task state, not stale artifacts. After the lead
confirms the environment is no longer needed, remove only owned resources:

```sh
bin/kind delete cluster --name cma-scale
BUILDX_CONFIG="$PWD/private/buildx" docker buildx rm cma-scale-package
```

Then remove this task's `cache/`, Python `__pycache__`, unused local image tags,
`bin/` and `private/`. Preserve this runbook, `scripts/`, `manifests/` and sanitized
evidence until the lead integrates the handoff. Do not run broad Docker prune,
delete another teammate's containers/caches, or remove `source/`/worktrees yourself;
the lead owns checkout cleanup. Any final cleanup must list exact retained paths.

Handoff via the granted command only:

```sh
TL_HOME='/Users/shining/code/work/tech-lead' /Users/shining/.claude/skills/shining-tech-lead/scripts/team.sh peer-send developer-17 RUNBOOK-scale.md
# Repeat for local-verifier-02, developer-13, developer-15 and developer-16.
```

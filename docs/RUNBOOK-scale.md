# Local cma-scale environment

Owner: local-env-manager-02. Workspace: `/Users/shining/code/work/tech-lead.staff/local-env-manager-02`.
Track: ChronoAIProject/cma #1356, #1360, #1541; acceptance suite #1957; HPA/PDB handoff #1956.
Environment delivery: #1970. Repository scripts live under
`.github/scripts/cma-scale-local/`; the committed runbook is `docs/RUNBOOK-scale.md`.

**Status: local AM×2, EG×2 and Mongo×3 are running; HPA and candidate acceptance gates are pending.**
The initial integration checkout is `d6fc953a6cb43f8d9c90337b4e4ce17ecb291fbc`.
Do not label measurements from it as verification of developer-13's forthcoming refreshed candidate.
Read `evidence/binary-source-sha.txt` and `evidence/image-identity.txt` for the binaries actually installed.

## Isolation and capacity

Use **kind v0.30.0 / Kubernetes v1.34.0, native arm64**. Docker Desktop's VM is arm64,
8 CPUs / 16 GiB; the Mac has 18 CPUs / 128 GiB and approximately 1.5 TiB free.
Kind gives this task a disposable cluster with its own kubeconfig, independently
bounded node containers, and two worker scheduling domains without enabling or
reconfiguring Docker Desktop Kubernetes. There is no amd64 emulation.

| Component | Count | CPU request / limit each | Memory request / limit each |
|---|---:|---:|---:|
| Kind control plane | 1 | Docker cap 2 CPUs | Docker cap 3 GiB |
| Kind worker | 2 | Docker cap 2 CPUs | Docker cap 3.5 GiB |
| AM | 2; planned local HPA ceiling 4 | 250m / 500m | 256 MiB / 512 MiB |
| EG | 2; planned local HPA ceiling 4 | 250m / 500m | 256 MiB / 384 MiB |
| Mongo | 3 | 100m / 500m | 384 MiB / 768 MiB |
| Nginx / Toxiproxy | 1 each | 25m / 200m; 25m / 100m | 32 MiB / 128 MiB each |
| Metrics server | 1 | 100m / 200m | 200 MiB / 256 MiB |
| Sandbox controller | 1 | 50m / 250m | 64 MiB / 256 MiB |
| k6 | on demand | Docker cap 0.5 CPU | Docker cap 256 MiB |
| Rust compilation | on demand | Docker cap 2 CPUs, 2 jobs | Docker cap 5 GiB |

At AM=4/EG=4 the app and database memory limits total 5.75 GiB, plus routing and
system overhead. The namespace quota bounds requests to 4 CPUs/6 GiB, limits to
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
`auto` deliberately fails until `manifests/hpa-pdb-reviewed.json` exists from
developer-16's exact reviewed handoff. Preserve HPA CPU targets and stabilization
policy; localize the namespace and cap maxReplicas at four for this machine.
Do not silently replace the reviewed policy with a newly invented HPA.
Record original SHA/file hashes and every local resource/namespace/ceiling override.
The currently published draft is PR #1968 at
`594fe05074fb090584a71dbd78174699f4059c29`; its complete PR description says it is
not merge-ready, and there is no review verdict on GitHub.
`manifests/hpa-pdb-proposed.json` is a server-dry-run-valid projection only,
with namespace `cma-scale` and maxReplicas=4. It is NOT installed.
The draft specifies AM60% / EG200% CPU, +1/60s, 600-second downscale stabilization
and -1/120s. The lead is resolving that cadence. For EG, 200% of the current
250m request equals its 500m local limit: reliable scale-up cannot trigger.
Proposed local adjustment for review is EG request=125m with the 500m limit,
preserving the draft target while keeping headroom. Apply it to all matched
cohorts only after the handoff, and rerun the baseline. The installer requires
`cma.chronoai.dev/review-state: approved` on each exact reviewed object.
Keep PDB minAvailable=1 for each application; PDBs govern eviction, not forced pod
deletion or Deployment scale-down.

The acceptance suite must observe actual HPA growth to at least four, then return
to two after load removal. Manual `kubectl scale` is not autoscaling evidence.
The current draft's 600-second downscale stabilization and drain horizon must
be included in the observation window. Observe aggregate Mongo pool demand,
worker requests/limits, throttling, OOM and Pending pods at every stage. Stop a
load run on resource pressure rather than lifting quotas without a new budget.

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
The guarded peer-send calls to developer-13, developer-16 and local-env-manager-01
and all five requested runbook recipients were denied on 2026-11-13; the lead must grant the routes or relay the handoffs.
Missing prerequisites are BLOCKED, never PASS.

## Repository review and current handoff

The environment is tracked by #1970. The scoped source bundle and operational
runbook are prepared for a draft PR into `tl/cma-am-replicas/integration`, so the
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

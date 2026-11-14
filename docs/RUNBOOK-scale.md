# Local cma-scale runbook

Owner: local-env-manager-02. Workspace: `/Users/shining/code/work/tech-lead.staff/local-env-manager-02`.
Source delivery: #1970 / draft PR #1972, `.github/scripts/cma-scale-local/` and `docs/RUNBOOK-scale.md`.
Run commands from the workspace root unless otherwise stated.

## Current contract

The lead-selected application is `f6d77bbd30361e75c304690229e7d17e4540472a`, including the cross-ingress relay and startup correction. Check `evidence/latest-deployment.json` for the actual running source/images and observation time; a source announcement is not deployment. Native build output is `evidence/native-f6d77bbd3-build.log`. Subsequent builds use the exact published commit selected by the lead/integrator. No fixture/tool review or checksum handoff chain is needed under the lead's speed directive.

The normal topology is AM2, EG2, Mongo3, metrics-server, TLS Nginx routing, Toxiproxy and the sandbox controller. Each application spans two workers using revision-aware topology spread (`matchLabelKeys: [pod-template-hash]`). Both application PDBs are installed with minAvailable1 and AlwaysAllow; Mongo's PDB requires two members. HPA is deliberately absent during fixed OFF/ON N1/N2. The later automatic local phase uses min2/max4; production policy remains min2/max5.

M3 is due 15 November 2026 01:30 SGT: the controlling essentials gate has at most25 rows, with local N1/N2, automatic2→4→2 and actual-owner loss. The owner deadline is production verification by15 November18:00 SGT. This workspace performs local operations only. Production approvals, rollback triggers and no02:00–02:59SGT writes remain with deployment owners.

`a160` readiness does not qualify the later cross-ingress fix. Healthy forwarding in `f6` does not close EG-SSE-001: terminal-hop failure before enqueue may leave the original live waiter open. Developer08 owns that follow-up. The admission fixture's Telegram callback JSON is separate from the original-client Custom SSE test. Report each case's result and failed row, not a whole M3 PASS from readiness or a component test.

## Isolation, resources and routing

We use **kind v0.30.0 / Kubernetes v1.34.0**, rather than enabling Docker Desktop Kubernetes. Kind provides a dedicated kubeconfig, disposable node containers and two worker scheduling domains while keeping the host's default cluster untouched. The default kubeconfig names production: every command below pins the local configuration. Never use a bare mutating kubectl command.

- Cluster/context: `cma-scale` / `kind-cma-scale`; namespace: `cma-scale`.
- API: `https://127.0.0.1:16443`; private kubeconfig: `private/kubeconfig`.
- TLS load balancer: `https://127.0.0.1:18080`; CA: `private/tls.crt`.
- Host `am.cma-scale.test` routes to AM; `eg.cma-scale.test` routes to EG.
- Host `cma-scale.test` routes `/api`, `/auth` and discovery to AM; `/event-gateway/v1`, `/mcp`, `/openapi.json` and `/docs` strip the gateway prefix. Existing connect/legacy-console paths are preserved.
- Streaming is unbuffered; WebSocket upgrades are enabled. Timeouts are660s and automatic upstream retries are disabled.

Production routing was inspected in integration `infra/{api,cmaeg}/{service,httproute,gcpbackendpolicy}.yaml`: no affinity is configured there. Local routing uses round-robin pod DNS and Services with `sessionAffinity: None`; there is no sticky cookie. This checks repository configuration, not production drift. Local TLS, five-second DNS refresh and the absence of GFE/Cloudflare differ from production.

| Component | Count | CPU request/limit each | Memory request/limit each |
|---|---:|---:|---:|
| AM | 2 default | 250m/500m | 256Mi/512Mi |
| EG | 2 default | 250m/750m | 256Mi/384Mi |
| Mongo | 3 | 100m/500m | 384Mi/768Mi |
| Fixture proxy when selected | per EG Pod | 5m/50m | 16Mi/32Mi |
| Kind worker | 2 | Docker cap2CPU | Docker cap3584Mi |
| Kind control plane | 1 | Docker cap2CPU | Docker cap3Gi |
| Packaging builder | on demand | Docker cap2CPU | Docker cap3Gi |
| k6 | on demand | Docker cap0.5CPU | Docker cap256Mi |

Docker Desktop VM has8CPU/~16GiB. Kubernetes advertises the VM's capacity on **each** kind node; use the enforced Docker limits when assessing headroom. Namespace quota is requests4CPU/6GiB, limits9CPU/9GiB,24Pods and12GiB storage. Four replicas per application before fixture/runtime/system overhead request2.35CPU/3264Mi and limit6.8CPU/6144Mi with EG750m. This declaration is not survivor or sustained-load qualification. Include proxies, terminating processes, runtime Jobs and Mongo connections in the live budget. Stop on OOM, pressure, Pending or quota failures; do not raise caps to hide them. Both workers share one physical Mac/VM.

```sh
docker stats --no-stream --format '{{.Name}} {{.CPUPerc}} {{.MemUsage}}'
scripts/k top pods
scripts/k get resourcequota local-budget
scripts/k get deployments,statefulsets,pdb,hpa
curl --cacert private/tls.crt --fail -H 'Host: am.cma-scale.test' https://127.0.0.1:18080/ready
curl --cacert private/tls.crt --fail -H 'Host: eg.cma-scale.test' https://127.0.0.1:18080/readyz
```

The `scripts/k` wrapper rejects destination/authentication overrides, pod exec, attach, copy and raw URL calls. General command acceptance does not authorize unrelated resources. No Secret API reads or pod exec are needed. Credentials stay in0700 `private/`, with0600 files and Kubernetes Secret references; send paths only.

## Refresh the application

Use active GitHub account `chronoai-shining`; stop on a different account without switching. Do not rebase, force-push, create another worktree or poll CI. The helper archives the exact commit, so uncommitted peer edits cannot enter the build. Use the existing checkout as the Git object source.

```sh
gh auth status --active
git -C source fetch origin tl/cma-am-replicas/integration
CANDIDATE=f6d77bbd30361e75c304690229e7d17e4540472a
git -C source rev-parse "$CANDIDATE^{commit}"
TASK_TOOL=/Users/shining/code/work/.tl-worktrees/cma-cma-release/api/target/fast-lane-batch4/tooling
export PATH="$TASK_TOOL/rustup/toolchains/1.96.1-aarch64-apple-darwin/bin:$TASK_TOOL/cargo-zigbuild-aarch64-apple-darwin:$TASK_TOOL/zig-aarch64-macos-0.16.0:$PATH"
export RUSTUP_HOME="$TASK_TOOL/rustup" RUSTUP_TOOLCHAIN=1.96.1-aarch64-apple-darwin
export CARGO_HOME="$PWD/cache/native-cargo-home" BUILDX_CONFIG="$PWD/private/buildx"
export ZIG_GLOBAL_CACHE_DIR="$PWD/cache/zig-global" ZIG_LOCAL_CACHE_DIR="$PWD/cache/zig-local"
scripts/build-cma-scale-native-load.sh "$CANDIDATE" --repo "$PWD/source" --cluster-root "$PWD" --platform linux/amd64 --builder cma-scale-package --cache-root "$PWD/cache/native" --build-only > evidence/native-f6d77bbd3-build.log 2>&1
python3 scripts/rollout-local-images.py "$CANDIDATE"
```

The helper comes from local-env-manager-01 and automatically uses the normal shared build-slot. Honor an announced exclusive quiet window before starting another build. This toolchain already has x86_64-musl stdlib; no install is required. Native Mac compilation is faster than the initial Docker build; Linux amd64 execution is emulated on arm64 kind nodes. Match architecture and resource settings between performance cohorts. EG media transformation is disabled in these local images and is not qualified.

The rollout command checks image source labels/platform, requires fixed2 and no owned HPA, loads into kind, and patches only application image/source fields. It preserves private configuration. It allows840s for retained production-like termination grace. Before invoking it, finish any active scenario and wait for previous Pods to terminate. It then calls the observer below. Do not use the helper's full-manifest deployment path over an active fixture.

```sh
python3 scripts/observe-deployment.py "$CANDIDATE" --record
```

The observer requires exactly two Ready/Available Pods per application, two workers each, no terminating Pods, expected source annotations and TLS200. It records Docker config IDs separately from actual CRI image digests in `evidence/latest-deployment.json` and `evidence/deployment-ledger.jsonl`. Omit `--record` for a read-only peer check. No readiness check is a user-journey test.

## One serialized EG fixture campaign

Ownership: environment02 provisions and controls Pods/proxies/forwards; developer17 owns exactly one outer fixture process; developer08 owns inner requests/oracle; verifier02 observes the same run. The order is the named cross-ingress host diagnostic, then fresh OFF, fresh ON N1, ON N2; automatic scale and actual-owner loss are separately bound phases. Host-process diagnostic results do not stand in for kind results.

The corrected fixture binary is `bin/cmaeg-admission-fixture`, copied from developer08's clean published `eb8c48ca892b6d54c5f4b74b2dc0dbf41900e7a1` export,17/17 tests and build PASS. It includes the permanent FIFO connection-head correction and excludes the later SSE relay. Its source is distinct from the application image. Never reuse old1630 for oracle acceptance.

Prepare a fresh database and config, **without starting a fixture**:

```sh
python3 scripts/provision-eg-fixture.py --cohort off --run-name f6-off
```

This creates `private/eg-fixture-f6-off/config.json`, a fresh `eg_diag_<UUIDhex>` database, separate migrator/gateway/oracle identities with schema-derived grants, and one local Mongo primary forward (normally127.0.0.1:28117). Provisioning uses an owned Job and existing Secret references. The Job is removed; the private state and database are retained. An already used run name/output or occupied Mongo port is refused. ON must use another fresh name and database after OFF finality/forward cleanup.

Send developer17 the config and owned binary paths and the selected PREPARE window. Developer17 launches its integrated `.github/scripts/eg-admission-lifecycle/fixture_process.py`, not a second bootstrap process:

```sh
python3 /ABSOLUTE/INTEGRATED/.github/scripts/eg-admission-lifecycle/fixture_process.py \
  --binary "$PWD/bin/cmaeg-admission-fixture" \
  --config "$PWD/private/eg-fixture-f6-off/config.json" \
  --out /ABSOLUTE/DEVELOPER17/NEW_OBSERVATION_DIRECTORY \
  --ready-seconds 60 --lifetime-seconds 1800 --shutdown-seconds 63
```

The1800s lifetime is an upper bound for one coordinated cohort, not permission to begin traffic. Setup60s, each request25s, terminal30s and normal shutdown63s remain distinct budgets. The wrapper emits `fixture_ready` after bootstrap is written and leaves the child alive. Developer17 returns the `output/bootstrap.json` path only. Owner then injects configuration:

```sh
python3 scripts/configure-eg-fixture.py --cohort off --run-name f6-off
scripts/k rollout status deployment/cma-event-gateway --timeout=840s
scripts/k get pods -l app.kubernetes.io/name=cma-event-gateway
```

The configuration script selects only text Secret fields for env; the binary cookie key is a mounted file. It sets the dedicated database/isolated authority, Pod-IP `CMAEG_REPLICA_ADDRESS` on8090 and an unprivileged loopback proxy127.0.0.1:19100→host.docker.internal:19100. EG is set to1. Wait for all old Pods to disappear before readiness handoff. Do not copy binary cookie bytes into envFrom: that caused the earlier invalid-UTF8 container error.

The provider/JWKS proxy does **not** establish B→A replica connectivity. The stream relay uses POST `/internal/replicas/custom-stream` on the existing Pod-IP8090 listener. Keep that listener reachable between gateways; actual relay acceptance must demonstrate the cross-process route.

Create direct Pod forwards after rollout, using the exact expected ready count:

```sh
python3 scripts/forward-eg-fixture.py --run-name f6-off --source "$CANDIDATE" --count 1
```

The script refuses an existing endpoint record or occupied port, verifies current Pod source/readiness/no termination, and records the actual Pod UID/process PID privately. After ON N2 is ready, use its fresh run name and `--count 2`. Under owner lifecycle, reserve28081 for ingressA and28082 for ingressB. Bind only127.0.0.1. Never hand off a planned port as a live endpoint. Record exact PID/Pod UID/port privately, check `/readyz`, and end those exact owned forwards before Pod replacement. Send **RESOURCE READY** with actual source/images, private config/bootstrap paths, fixed replica count, endpoints and window. Only then may08 send inner traffic.

`/control/run` supplies signed Telegram callback events and independent durable/provider/sink accounting. It is not a Custom SSE client.08 supplies the separate Custom observer;15 joins routing/target evidence. Preserve the original completed0ac event without resend. A healthy Custom result cannot close EG-SSE-001.

End sequence:08 finishes requests; owner establishes actual gateway process finality;08 closes/reconciles the fixture;17 stops its single process within63s. Preserve private database/journal and all failed outcomes. `/control/close` alone is not gateway death. Then stop only the exact recorded forwards and prepare a fresh ON config. AM browser/create/recovery fixtures remain missing and are separately owned with01/17/02.

## Fixed modes, HPA and faults

Owner controls only, at agreed boundaries with no concurrent rollout/load:

```sh
scripts/mode.sh fixed-1
# Matched N1 scenario by17/08; preserve its output and outcome.
scripts/mode.sh fixed-2
# Matched N2 scenario, then finality before the automatic phase.
scripts/mode.sh auto
scripts/k get hpa,pdb,deployments
```

Fixed modes remove only HPAs targeting the two application Deployments, scale both, and wait for exact nonterminating Ready counts. Each transition preserves its initial HPA/deployment checkpoint and partial failures under a new `evidence/mode-transitions/` directory. A failed attempt is never overwritten. For the EG-only admission slice, the owner may transition only EG under the announced schedule; state actual AM/EG counts explicitly.

`auto` validates and applies `manifests/hpa-local.json`: AM CPU60%, EG CPU200%, min2/max4 locally, scale-up0s with Max(2Pods,100%)/60s; scale-down stabilization300s and onePod/120s. Production source remains min2/max5. Extend the down period if qualified drain/handoff p95+30s exceeds120s. The actual termination horizon and survivor capacity still require measurement. EG250m request/750m limit permits CPU to exceed the500m target. A fixture sidecar changes aggregate resource-metric arithmetic and must be included in interpretation.

Manual scaling and readiness load do not prove workload autoscaling.17 must bind accepted operations, load, independent outcomes and actual-owner kill before the corresponding essentials can pass. Observe HPA growth under load and return to2 after load; record Pending/OOM/throttling, quota, Mongo/runtime/system and terminating-process usage. No HPA/image rollout overlap. PDB governs eviction, not forced deletion or scale-down.

```sh
DURATION=60s scripts/load.sh steady-health
scripts/fault.sh kill am
scripts/fault.sh kill eg
scripts/fault.sh delay am 150 30
# While the delay is active, from another owned terminal:
FAULT=delay DURATION=20s scripts/load.sh delayed-health
```

These are bounded k6/readiness and fault tools, not a ready M3 user-work adapter. The supplied kill picks a running Pod only with at least2Ready; actual-owner acceptance additionally requires operation→owner→Pod binding before selecting a target. Do not use the arbitrary-Pod helper as proof of owner loss. Toxiproxy delays only the selected LB→application route for requests carrying `X-Scale-Fault: delay`; it does not partition Mongo or replica relay. Use the same proxy path with zero delay as the control.

Historical readiness-only runs: N1 and N2 each60/60 checks; abrupt AM/EG loss126/128 (FAIL zero-error threshold); same-path mean5.82ms→156.71ms with150ms delay→5.00ms after removal. These older binaries/results are retained as tooling context, not current acceptance.

## Reproduce the base infrastructure

The existing cluster is retained for teammates. Do not rerun render/apply over private cohort configuration. For a new empty owned installation only:

```sh
mkdir -p bin cache manifests evidence private
chmod 700 private
# Install the pinned kind CLI into bin/ with its upstream checksum verified.
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
scripts/k apply -f manifests/pdb-local.json
```

Mongo8.0.13 uses authenticated three-member rs0,0.25GiB WiredTiger cache/member,128MiB oplog,3GiB PVC/member and application pools capped10/server. `check-mongo.py` runs a disposable Job with the probe identity, reporting only healthy/member/primary counts. Metrics-server0.8.0 uses insecure kubelet TLS only for kind certificates. The sandbox controller1.0.2 is present for AM startup; runtime pools and a real create fixture are not supplied by its presence.

## Verification, handoff and retention

The existing offline controls tests are meaningful argument/partial-failure checks:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s source/.github/scripts/cma-scale-local/tests -v
```

13 checks pass: destination override rejection, local pinning, immutable prior transition preservation, snapshot/HPA/scale/rollout failures and termination exclusion. Native build, packaging and actual rollout provide separate current validation. No new tooling-review gate is imposed.

After each rollout send a plain handoff: actual source/image digests, readiness, config paths, changes, how to test and remaining failed row. Required recipients include17, verifier02,13,15,16 and debugger03 as applicable. Use authorized forced delivery to WORKING recipients:

```sh
TL_HOME='/Users/shining/code/work/tech-lead' /Users/shining/.claude/skills/shining-tech-lead/scripts/team.sh peer-send developer-17 RUNBOOK-scale.md --force
```

Current create observer is17's corrected `timing/local-create.mjs` (18 checks PASS), not01's earlier draft. Actual local runtime/provider/synthetic identity/custody/same-origin Web inputs and ordinary-work recovery admission adapter are still required. No create measurement exists. Owner absolute targets hold: create≤5s, open/send/interrupt≤1s, usage≤2s. Use architect A3 classes and matched cohorts; no blanket regression allowance or relabeling after results.

Retain the active cluster, cache, private config, database and fixture journals until owners finish. Preserve failed outcomes. After the lead retires this environment, remove only its cluster with `bin/kind delete cluster --name cma-scale` and builder with `BUILDX_CONFIG="$PWD/private/buildx" docker buildx rm cma-scale-package`. Do not prune shared Docker state or remove peer checkouts. The lead owns worktree cleanup. Historical pre-f6 runbook is retained locally at `evidence/runbook-before-f6.md`; it is not current operational guidance.

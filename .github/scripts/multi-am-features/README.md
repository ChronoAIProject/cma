# Two-AM feature regression

This suite turns the run17 real-process journey into a repository-owned CI job.
It starts two independently addressable Agent Managers with coordination enabled,
one authenticated Mongo replica set, a synthetic identity provider and a held
response provider. Native runtimes use the agent-sandbox controller and RWOP CSI
storage inside a uniquely named disposable Kind cluster.

**Draft: full feature coverage is not complete.** The frozen inventory contains
120 CMA user, 76 CMA admin and 79 Event Gateway features: all 275 identities are
retained. Event Gateway rows preserve their original actor roles and require the
real platform journey; direct AM calls cannot substitute for these assertions. The current
case mapping marks incomplete feature assertions as `BLOCKED`, including features
with implemented API assertions but missing browser/negative/fixture steps. The
full CI result intentionally fails while any row is blocked, unexecuted or fails.
An executable subset passing is never a full-inventory PASS.

## What executes

- The run17 create, open, held Send, interrupt, owner-loss recovery, desktop frame,
  lifecycle cleanup and ten-minute two-process serving assertions.
- Live owner lease/member observations select the other direct Pod endpoint for
  Agent feature requests. The harness checks ownership again after completion;
  ownership drift or a missing response request ID fails attribution without
  replaying a mutation. A positive ownership epoch is required before dispatch.
- Files: directory/file creation, byte and digest readback, listing, CAS edit,
  stale-write refusal, download and deletion. Preferences: rename, peer-visible
  readback, stale-revision refusal and guarded restoration.
- Documents: create, search, known-receipt idempotent replay, cross-process edit,
  source digests and history, stale-write refusal, comment grants and revocation,
  and read links with revocation. A separate admin subject proves that global
  operator permissions do not grant implicit access to personal documents. Both
  processes must read unchanged source identity after the owner restart.
- Shared admin assertions for inventory, exact permission denials, settings CAS,
  full replacement, inactivity grace, orchestration pause/resume, admission
  stop/resume and complete usage reports. Global admin operations have no Agent
  owner; their results explicitly identify that scope and both AMs.
- Agent-bound Always-on writes use the known Sandbox-to-Agent binding to select
  the live non-owner. Public Agent reads must remain valid through both AMs after
  enable/revoke, and replaying the already acknowledged create intent must retain
  its original lifecycle identity. Rename and Always-on failures retain typed
  private responses, request IDs, and separate primary/restoration evidence.
- Changed system prompts and permission modes run through both personal and
  authorized operator entrypoints, with exact revision, acknowledged runtime
  configuration, stale-write refusal, preserved prompt and guarded restoration
  assertions. While a real provider response is held, busy configuration writes
  must refuse without changing state; changed titles must survive the actual
  owner restart. Private cases retain the checked state and owner/request joins.
  These do not inject a deterministic store conflict or count native Configure
  effects. The full single-writer regression requirements and remaining gaps are
  tracked in `writer-regressions.json`.
- Feature hooks run before and after the owner restart. Each selected feature case
  repeats three times in each phase. The actual owner StatefulSet scales to zero;
  the old native container task must be absent. The survivor must recover a higher
  ownership epoch and finish the held response exactly once. Restoring the second
  process overlaps useful reads through the survivor.

The identity and upstream provider are deterministic dependencies. The AM,
ownership coordinator, command ledger, Mongo transactions, runtime, files and
desktop are real. No application store is patched to create ownership, force
completion, or repair a failed run. Controller scaling is the planned restart
scenario; failure handling preserves diagnostics and tears down the owned cluster.
Requests are paced across clients sharing a fixture identity to stay within the
default Create and read budgets. Pacing happens before owner observation and does
not retry failed mutations. Mongo observation shells are serialized within a
2 GiB request / 3 GiB limit, and each failure capture has a unique directory.

Document object bytes use a bounded synthetic HTTPS storage service with exact
machine-client authentication and content-addressed write receipts. The real AM
storage adapter, document policy, revision checks and Mongo records execute. This
dependency persists across AM restart; its in-memory bytes do not prove recovery
from an object-store crash. The same fixture platform origin refuses unsupported
service routes, so these cases do not imply skill-catalog or title-service coverage.

## CI inputs and execution

`.github/workflows/ci.yml` runs offline contracts immediately and invokes
`multi-am-features.yml` after the existing image jobs. The suite consumes the
`api`, `runtime` and `runtime-omarchy` exports from the same workflow invocation.
Archive hash, source revision, run/attempt, platform, runtime contract, loaded
configuration digest and imported containerd identity are checked. Historical
release images and external registry credentials are not accepted as substitutes.
The pinned Mongo registry reference is pulled for the native platform, then its
imported containerd manifest is rebound to that same image configuration. The Pod
uses this verified immutable imported reference; a registry index need not survive
the Docker archive format. No mutable Mongo tag is used for execution.

Use a native Linux amd64 runner with at least 16 GiB RAM and sufficient disk for
the three image archives, Kubernetes and native homes. `MULTI_AM_CI_RUNNER` can
select an organization's larger runner; the default is `ubuntu-24.04`. Insufficient
memory fails before starting the fixture. Python 3.13, Node 24.15, Docker, kubectl
and pinned Kind 0.30 are required. Fork PRs run the offline contracts; the existing
image export policy does not export their images for the runtime job.

To reproduce with those same image exports:

```sh
npm ci --ignore-scripts --no-audit --no-fund --prefix .github/scripts/multi-am-features
python3 -B .github/scripts/multi-am-features/run_ci.py \
  --artifacts "$ARTIFACT_DIRECTORY" --output "$FRESH_OUTPUT_DIRECTORY" \
  --revision "$CHECKOUT_REVISION" --run-id "$CI_RUN_ID" --attempt "$CI_RUN_ATTEMPT"
```

The fixture refuses remote Docker endpoints and uses its explicit kubeconfig for
every Kubernetes call. All credentials are freshly generated test inputs held in
ConfigMaps in this disposable cluster. The client rejects Secret resource access.
The runtime role comes from the exact AM image, not an administrator application
principal. No production account, namespace, registry or workstation path is part
of the fixture.

## TLS and desktop prerequisites

Native provider requests retain default WebPKI verification. The fixture downloads
the public `localhost.direct` test bundle from its official download destination,
`https://get.localhost.direct/localhost.direct.OP.zip`, obtains
its publicly documented archive password from the upstream README, and checks
pinned archive/member hashes, expiry, hostname and key agreement. It serves the
full chain with the original SNI and rewrites only this cluster's DNS. The pinned
certificate expires on 2026-12-25; rotation requires a reviewed hash update. Neither
the public fixture key nor any generated credential is committed or uploaded.

The desktop uses the repository's existing device-free browser seccomp policy,
materialized for native amd64, no outer capabilities, and Linux >=4.8. The generated
OCI profile SHA256 is
`e31f24f956896eeb4eed94eaa619250a126e401f2d42aee2b3eaaf6624ef0aff`.
Its base is the pinned Moby profile used by `../desktop-seccomp.mjs`; only the
existing constrained clone/unshare and in-namespace chroot additions apply. A
fixture admission hook binds the profile only to the selected desktop image and
rejects changed UID, privilege or capability settings. It does not establish that
an unmodified production `RuntimeDefault` policy supports the desktop.

## Coverage and failure evidence

`inventory/manifest.json` binds the original inventory hashes, counts and complete
ID sets. Inventory files retain feature steps, expectations, API routes and
repository source hashes, while omitting private evidence paths. `case-mapping.json`
names the executable assertions and remaining gaps per feature. Add focused cases
for the missing browser controls, organization roles, connection repair,
admission uncertainty, document browser interactions, trigger execution,
retained-runtime upgrades and
admin resource mutations before making this a merge-ready feature gate.

`writer-regressions.json` retains all SW01–SW13 requirements independently of the
275-feature inventory. The runtime summary evaluates each mapped assertion and
reports missing cases, incomplete mappings and failed observations. All thirteen
groups must be complete and pass before the overall suite can pass. Passing a
subset of native or journey assertions cannot clear a group's missing evidence.
M1–M5 supply their exact native selectors, deterministic conflict barriers and
effect-custody assertions to this mapping; their historical receipts alone do not
qualify a successor image.

Only `summary/coverage.json` and `summary/result.json` may be uploaded. They contain
coverage dispositions, counts, source/image bindings, cleanup finality and private
file hashes. Raw response bodies, logs, cookies, fixture keys and resource IDs stay
under `private/`. Normal exit, failure and signals join fixture children and remove
the cluster. An `always()` CI step repeats identity-checked cleanup after command
cancellation, then removes private data. Cleanup failure fails the run.
Setup failures expose only the fixed stage, exception class, safe error code or
HTTP status in the summary. An upstream public download failure is a fixture
failure; it does not constitute a product feature execution.
Infrastructure commands also retain fixed stage/program, exit status, duration,
allowlisted error classifications and private output hashes. The initiating
failure remains separate from cleanup. The HPA 2-to-3-to-2 scenario is still
unimplemented; owner restart is not counted as HPA acceptance.

Offline validation:

```sh
node --test .github/scripts/multi-am-features/contract.test.mjs \
  .github/scripts/multi-am-features/features/*.test.mjs
python3 -B -m unittest discover -s .github/scripts/multi-am-features -p 'test_*.py' -v
```

These tests exercise corrupt/mismatched image rejection, owner attribution,
uncertain-write custody, coverage accounting, permission denials, generated
configuration, desktop policy scope and cleanup ownership. They do not constitute
a two-process execution result or production/performance/HPA acceptance.

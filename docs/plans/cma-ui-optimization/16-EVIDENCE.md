# Ticket 16 evidence — complete

Recorded 2026-11-03 in isolated worktree
`/Users/liuyitian/code/cma-ui-worktrees/ticket-16`, branch `codex/cma-ui-16`.
Initial integration was `9bc2f04e883c6619e65ad7f424994f333c385055`;
latest integration `6722f9b0e5818d4337483804e896421a66da39a6` is merged.
Native proof source is `0437bd154`; later changes strengthen host tests,
declare one observed validation problem in the inventory, and merge integration
quality/E2E-type repairs. They do not change the native runtime or feature behavior.
Final exact commit, changed paths, source SHA-256 and artifact manifest are in
external `ticket16-proof/manifest.json` and `ticket16-proof/HANDOFF.md`.
Original acceptance text is unchanged.

## Behavior and ownership

Home offers **Enable desktop**, default off, with preparation/time/memory copy.
An authenticated, typed `GET /api/v1/desktop-capability` supplies availability
and a reason for the selected Workspace/Sandbox or a new environment.
An older service without the new OpenAPI contract keeps the option unavailable.
Viewing Home, choosing it, cancelling connection setup, refreshing, and returning
from account connection do not create resources. Only explicit submission does.

`desktop: true` is part of the original public creation request, retained operation,
admission identity and first-message preparation record. The Home recovery key,
exact whitespace/Unicode task and selection survive receipt loss and reload.
Accepted selections are frozen; a changed draft cannot change an admitted request.
Starting another task is an explicit new intent. Omission and explicit false
retain old server behavior; normal clients omit false. The Rust typed SDK can
also represent explicit false for exact wire conformance.

Layer 1 owns validation/OpenAPI/error projection; Core owns desktop eligibility
and preparation; existing Adapter/Runtime browser APIs perform actual enablement.
CMA checks replay before current availability, so disabling new admission does
not invalidate an already accepted operation. New unsupported requests fail
before admission/profile/environment writes. Unverifiable retained runtime state
is retryable unavailability. The existing first task stays queued until browser
preparation succeeds; transient loss of the enable receipt recovers the original
context. No observer action dispatches the first task or restarts a Sandbox.

The operator must explicitly set `CHRONO_DESKTOP_VERIFIED_RUNTIME_IMAGE` to a
verified immutable `name@sha256:<64 lowercase hex>` image. Default is empty/off.
The configured image must match, runtime contract is 3, capacity must be at least
1000 millicores/2048 MiB, the adapter must support Agent browser and retained home,
and an existing Sandbox must be active, unfenced, credential-ready, not restoring,
with the same observed generation/boot. Paused, undersized or unverified existing
environments are refused with a reason; they are never automatically upgraded,
replaced or restarted. This setting is an operator attestation, not an automated
claim that arbitrary images or hosts have passed native verification.

Progress distinguishes **Desktop preparation** from task execution. Preparation
ready does not mean Live or task success. Home can enter the admitted Agent early;
the existing Browser panel shows Live only after a fresh real frame decodes.

## Actual first red and green

Logs are external under `/Users/liuyitian/code/cma-ui-worktrees/`.

| Boundary | Actual red | Actual green |
| --- | --- | --- |
| First Home receipt-loss/reload story | `ticket16-first-red.log`: missing Enable desktop checkbox, 1 failed | `ticket16-first-green.log`: same original body/key/task/Agent and browser projection, 1 passed; native completion below replaces projection-only evidence |
| Real public router + Mongo refusal | `ticket16-preflight-red.log`: unknown desktop yielded invalid_json instead of typed refusal | `ticket16-preflight-green.log`: 1 passed, desktop_unsupported before admission |
| Supported public creation | `ticket16-enabled-red-retry.log`: 422 instead of 202 | `ticket16-enabled-green.log`: 1 passed, preparation precedes exact first task |
| Retained unavailable draft | `ticket16-unavailable-draft-red.log`: could not clear unavailable selection | `ticket16-unavailable-draft-green.log`: 2 passed |
| Truthful progress | `ticket16-progress-red.log`: missing desktop row, 1 failed/7 passed | `ticket16-progress-green.log`: 8 passed |
| Contract completeness | `ticket16-inventory-final-red.log`: missing-task case emitted undeclared invalid_request | `ticket16-inventory-final.log`: 2847 cases/239 operations/1627 named HTTP pairs/23 protocol pairs |
| Markdown/OpenAPI inventory | `ticket16-openapi-docs-red.log`: new endpoint described but missing from checked table, 1 failed/9 passed | `ticket16-openapi-docs-final.log`: all10 tests pass with the exact operation row |

The initial supported-create attempt in `ticket16-enabled-red.log` encountered
shared Mongo exiting with WiredTiger error24, not a feature assertion. It is not
counted as TDD red. The same stopped Mongo container was restarted, and only its
live soft nofile limit was raised from1024 to65536; no VM/daemon restart or data
cleanup occurred. Subsequent genuine red/green logs are retained separately.

Final public/Mongo filter `initial_task_public_` passes **9 tests, zero failed or
ignored** (`ticket16-public-final.log`). Assertions include exact task/key recovery,
changed-body/foreign-owner refusal, legacy omission, process exit, worker restart
before/after dispatch, uncertain runtime-write retention, dropped enable response,
preparation ordering, and existing Workspace admission with the original runtime
and lifecycle unchanged. Availability GET plus refused POST leave every Mongo
collection's count unchanged; the rejected key can still admit an ordinary task.
The existing-environment admission assertion uses the synthetic external runtime
boundary; the following native proof exercises creation of a new environment.

The final focused Home/progress tests pass **3 files/32 tests**, including a desktop
repository intent cancelled in the connection dialog, refreshed after connection,
and resumed only explicitly with frozen options/body/key. This complements the
full Web **336 files/3011 tests** pass. No skips or weakened assertions were added.

## Formal creation through native runtime to public Web

The successful run is `ticket16-proof/native/`. It starts with the real built Web
Home and an actual public `POST /api/v1/agents`; it does not pre-create an Agent,
Workspace, browser or first task. The first202 is deliberately dropped. Refresh
and Recover original task resend the exact key/body and receive the same receipt.
Real Mongo, lifecycle admission, Sandbox/Workspace/Agent workers and the production
Hosted control/ACP transport prepare the native process. A network-none private
file relay forwards exact private HTTP/WebSocket messages without changing them.

Pinned Codex0.147.0 and patched ACP1.4.0 execute the actual `cma-browser navigate`
tool inside the runtime, visiting the local proof page. Native Chromium renders
it; Core/public browser handlers and the typed SDK deliver its JPEG to the actual
Web panel. The original task is present once in the durable transcript and visible
in the conversation. Reloading that Agent preserves the browser context; the
native result still requires exactly two model rounds and one completion.

| Identity/result | Actual value |
| --- | --- |
| Agent | `agt_9b856e8373cd4646acf51c60ac1b6b3c` |
| Operation | `fb2a904b5913488eb86c17421d0600b3` |
| Sandbox | `sbx_04553feecdbc40f4ba9390e3e6485001` |
| Browser context | `22f9977e-ee55-4ca6-b7c7-b29f49e9499c` |
| Runtime incarnation | `b090fedbf662c113bf60cc39a2a3a75d4adc45f29c733412e734cece790206cb` |
| Public control forwards | 31 |
| Model rounds/completions/page navigations | 2 / 1 / 1 |
| Actual frame | 1280×720 JPEG, 10227 bytes |
| Frame SHA-256 | `ae43a896c6c1d908716a10b3686e1881e5febb3d4d1e8e218988c66574729b0d` |
| Web observations / creation replay | 3 / 1 |
| Rust public/native driver | 1 passed, zero ignored,39.66s |
| Chrome Web driver | 1 passed,34.2s test/47.4s runner |

Primary artifacts: `run.log`, `public-run.log`, `web-run.log`, `native-run.log`,
`relay/creation.json`, `relay/provisioned.json`, `relay/native-result.json`,
`relay/web-result.json`, and `relay/web/.../actual-native-frame.jpg`.
The1440/390/320 screenshots were inspected; the real browser page is visible,
the frame lies within the viewport, and the document does not overflow.
The source is reproducible through committed `desktop-chain.cjs`,
`desktop_native.rs`, `desktop-native-relay.mjs`, and `desktop-native.check.ts`.
The external `native/run.mjs`, `sample.py`, `config.json` and archived source/rootfs
retain the exact local orchestration. A future rerun must use a separately reserved
VM slot and unique state/cgroup names; do not overlap sibling17's workload.

## Image correction and failed native attempts

The inherited ticket15 proof image was
`sha256:ddda63f74cd1b2067102fb43e66e1a2c47ced2f569c701bcc52d309bf816e112`.
Its Docker COPY placed the patched ACP bridge at mode0644. Earlier15 probes invoked
`node index.js`; actual formal creation executes the `codex-acp` symlink, exposing
EACCES. Attempt01 therefore created the real environment but retained the first
task without dispatch. It is preserved, not relabelled as a pass.

The ticket16 immutable derivative changes only that file's mode to0755:
`sha256:da56a37266cd285f584ba4ecc3982102b20bbdaa7c0b5ba36842b5087e43fead`.
This is a local immutable image ID, not a published registry-manifest claim.
The runtime binary remains
`364d71a3424476e5c7b27583b0df39cce1751e0d92be06347baa62a1e6dbf531`;
the patched ACP bridge remains
`84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c`.
Production `infra/runtime/Dockerfile` now checks `test -x /usr/local/bin/codex-acp`
after npm installation/patching, preserving the correction as a build gate.

Production Layer3/Layer4 and Cargo inputs are byte-equivalent to image source
`8be7293571476efd4c0e00e4b38e5abc9efd091b`; differing paths are only test/proof
scripts. The proof source mounted read-only in the VM is archived from9b2c25328;
its native script is unchanged in the final candidate. Host tests use the later
committed composition/driver. Exact hashes and path comparison are in the manifest.

Attempt02 exposed a five-second test navigation deadline shorter than cold startup.
Attempt03 ran the actual task successfully but exposed the proof router's missing
Sandbox reads required by Home's owner/binding verification. Attempt04 reached a
real Live frame but exposed incomplete proof Usage grants and a mobile reload
driver race. The proof now composes actual hierarchy reads/Usage, verifies the
frame's viewport position, and reloads at desktop width. Attempts01–04 are retained
under `native/attempt01` through `attempt04`; only the final run is acceptance proof.

## Preparation timing and resource measurement

The final run enters the actual Agent **655ms after submission**, during setup.
Public events record desktop preparation from08:56:35Z to08:56:39Z (~4s), with
the Agent ready ~6s after admission. First fresh decoded Web Live is **31.083s**
after submission. This includes actual application navigation/observation delays;
it is not substituted with native startup time or represented as a latency SLA.

`cgroup-samples.json` contains886 samples at approximately100ms and
`cgroup-budget.json` records the full89.920s native interval, including the host
Cargo-build wait. Peak memory is **910155776 bytes (~868MiB)**, peak PIDs **93**,
and cumulative CPU **14.103332s**. Preparation samples span6.855s and4.664 CPU-s;
task/observation samples span26.582s and5.673 CPU-s. The accounting includes the
native runtime, ACP, Codex, Chromium and proof bridge/server in that cgroup;
host API/Web/Mongo and unrelated VM tasks are excluded.

The final cgroup readback is `memory.max=2147483648`, `pids.max=512`,
`cpu.max=100000 100000`. Earlier15's unlimited CPU observation is retained as its
own limit; this run's observed quota does not establish a production host guarantee.
The VM remains2vCPU/4GiB, gVisor release-20261102.0/systrap/network-none,
uid10001, empty capabilities, noNewPrivileges, read-only rootfs, private writable
home/tmp, and native Chromium sandbox enabled. No Chrome `--no-sandbox` is used.
The heavy VM slot was explicitly released at16:58; no ticket16 native workload
remains. Host private build target is `ticket16-cargo-target`.

## Contract, compatibility and rollback

The shared corpus adds13 desktop cases, replayed against actual public handlers,
typed TypeScript and typed Rust SDKs. Rust executes12 typed HTTP cases; the malformed
unknown query is explicitly classified as unrepresentable by its closed typed
scope and verifies that shape/decoder, not silently skipped. The TypeScript full
shared suite passes2852 tests. Final inventory passes2847 cases,239 OpenAPI
operations,1627 exact named HTTP problem pairs and23 protocol failure pairs.
A mistakenly absolute corpus filename selected no cases and failed; that log is
retained as `ticket16-public-contract-selector-error.log`, not counted as proof.

Current-binary process exit/replay returns the same operation/Agent. The retained
ticket15 executable was actually run against the new desktop journal and returned
503 temporarily_unavailable because its strict request reader cannot decode the
new field. Its separate legacy omission/process-restart test passes1/1.
Artifacts are `old-reader-restart.log`, `old-reader-legacy-positive.log`,
`old-reader-binary.sha256`, and `new-reader-restart.json` in `ticket16-proof/`.
The old executable SHA is
`f15635a23cfbfc7a96518198466fd2eaad32908c4e1580ad76d0d6bd4738af7a`.

Therefore retained16 desktop operations/first-message records require a16-aware
reader, in addition to15's agent_browser-aware RuntimeStatus reader floor.
Disabling the UI or new desktop admissions is not a safe old-binary rollback.
Do not remove/translate accepted records or silently create a replacement environment.
The native proof uses the existing default autonomous/full-access Agent profile;
it neither changes permissions nor proves guarded/restricted execution modes.

## Final checks and limits

- Full Web336files/3011tests; focused final Home/progress3files/32tests pass.
- Web and E2E TypeScript, TS SDK types, Web/SDK ESLint and CSS collision gates pass.
  The merged parent15 E2E repair removes the previously recorded11 type errors.
- Four-layer+CMA all-target Clippy and Rust SDK all-target Clippy pass without allowances.
- Real public/Mongo9tests, public desktop corpus13, inventory, Rust typed desktop
  corpus, current process recovery and actual old-reader positive/negative checks pass.
- Markdown/OpenAPI inventory10tests and retained browser/environment36+10 real
  handler cases pass, preserving authorization and runtime/context fences.
- Web production build passes. Source line limits, changed-file formatting,
  append-only Web Style, exact original AC wording and diff checks are recorded
  with the final SHA manifest. No original workspace files were modified.

This proves local formal creation and actual native browser observation. The model,
IdP, provider endpoint discovery and home allocation are controlled external
boundaries; there is no GKE, production IdP/network, registry publication, durable
PVC or external model-quality claim. Optional access/feedback/SSE startup fixture
limitations remain visible in screenshots; they are not hidden with fabricated
successful conversation updates. The actual durable transcript and native/browser
path are real. Runtime restarts/host crashes after frame capture, long-running cost,
guarded permission modes and production deployment belong to separate validation.
Original unrelated full-format/skill-fixture/508-line-service/LocalAgentV2 limits
and ticket13 human-quality acceptance remain independent; this does not mark all19
tickets complete. Ticket17 control/takeover remains a separate sibling change.

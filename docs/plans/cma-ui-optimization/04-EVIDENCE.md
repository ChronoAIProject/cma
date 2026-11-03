# Ticket 04 evidence — 2026-11-03

Worktree: `/Users/liuyitian/code/cma-ui-worktrees/ticket-04`; branch:
`codex/cma-ui-04`. Initial integration baseline:
`9a900786a99ee2bb9a05f56f68f2a62d61abf189`. Final integration merge:
`32bfb92c6428d1b12fce23e4d9a7a9a21437f92f`, containing integration
`a426b87ee5852866850cab192b37d14f9f2087c6`, including 05 and 14.
The final merge only updated the shared implementation record; it changed no
tested source. The worker does not merge into integration, push, or deploy.

## Result and contracts

- Home retains optional GitHub URL/ref beside the exact task draft. Clearing
  context preserves text. Draft and retained-request scopes remain per account
  and parent context; corrupted retained repository fields fail closed.
- Repository tasks require the advertised `x-cma-task-repository` capability.
  Ordinary empty-workspace tasks remain available against compatible older
  task servers. Existing Workspace/Sandbox task creation inherits its context;
  the server rejects a repository override before admitting an idempotency key.
- The existing lifecycle operation freezes task, repository/ref and request
  key. Its existing service prerequisite requests `api-github`; the existing
  connection flow and explicit continuation replay the same operation. Cancel,
  connection refresh and next-draft edits do not silently submit another task.
- Repository preparation precedes first-task dispatch. The runtime uses the
  existing scoped GitHub Unix proxy, resolves an upstream commit once, fetches
  that commit's archive, and creates a real local Git checkout. Its receipt
  records both `source_commit` and the actual local `resolved_commit`. The
  worktree is created at the latter. Public Workspace/Agent reads, both SDKs
  and Details distinguish requested ref, upstream source and actual checkout.
- Preparation uses the existing workspace staging journal and Git/file
  protection. A completed receipt replays after restart and ref movement;
  ambiguous interrupted extraction fails closed without resolving the ref
  again. An incompatible existing path is preserved. No new creation state
  machine, credential channel, automatic title policy or desktop option exists.
- Unavailable repository, missing ref, unsafe source and connection failures
  retain distinct preparation evidence. Preparation failure prevents dispatch.
  Terminal failure keeps the original intent visible and requires explicit
  new-task action to change it; execution then uses the existing task lifecycle.

Archive preparation is bounded to 64 MiB downloaded, 256 MiB expanded and
20,000 entries. Protected metadata, duplicate/traversing paths, escaping links,
special files, submodules and unresolved LFS pointers are rejected. This source
mode creates a local snapshot commit; it does not claim a fetched upstream Git
history or that the two commit hashes are equal. Archive retrieval depends on
the existing broker's redirect handling and fails closed if unavailable.

## Test-first evidence

All logs below are outside the repository under
`/Users/liuyitian/code/cma-ui-worktrees/`, with prefix `ticket04-`.

| Boundary | Observed red | Green behavior |
| --- | --- | --- |
| U connection/recovery | `u-red.log`: repository controls absent | `web-final.log`: frozen repository/ref, task and key survive refresh; explicit continuation only; cancellation and next draft preserved |
| A public creation | `a-red.log`: actual public route returns 422 for repository + initial task | `a-final.log`: prerequisite pause/replay, one Agent, actual checkout provenance and exact first task after preparation |
| R actual candidate | `r-red.log`: baseline binary rejects the new source frame | `r-linux-final.log`: candidate prepares real Git/worktree and pinned native provider executes the first task there |
| Failure classification | `r-errors-red.log`: inaccessible repository mislabeled missing ref; `a-failure-red.log`: specific failure lost as generic Sandbox failure | Metadata access checked before ref resolution; public operation retains safe source class and sends no task on failure |
| Compatibility/provenance | `u-capability-red.log`, `u-commit-red.log` | Old-server empty task fallback; separate requested ref/source/actual checkout display |
| Corrupted recovery data | `u-storage-red.log`: two malformed repository values fail | Both are rejected without crash or replacement submission |

`a-validation-red.log` also contains an invalid-ref admission failure, but its
Mongo descriptor exhaustion and standalone subprocess-helper failures are
infrastructure/harness failures, not accepted feature-red or green evidence.
The successful final run uses a disposable replica set with an explicit
65,536 descriptor limit and bounded WiredTiger cache. The subprocess helper is
executed by its parent test with the required environment, not as a separate
top-level case. Zero-test cache hits and interrupted runs are not evidence.

## Final validation

| Check | Result | Log |
| --- | --- | --- |
| Web: AppCreation, Console, ConsoleTitle, ConsoleTaskTitle, useDocumentTitle, controller.title, TaskHome, Details, AgentLifecycleCard, ConnectionWizard | 88 passed, 10 files | `web-final.log` |
| Chrome: new/contextual task creation, uncertain command replay, signed-in/out landing, composer settings, access approval | 7 passed | `browser-final-green.log` |
| Composed public/Mongo task Home contracts | 11 passed, 0 ignored | `a-final.log` |
| Latest merged runtime unit tests | 298 passed, 2 existing ignored | `runtime-final-tests.log` |
| Runtime assignment CLI integration | 1 passed | `runtime-final-tests.log` |
| Pinned native Linux repository proof | 1 passed, 0 skipped | `r-linux-final.log` |
| TypeScript SDK lifecycle/agents/instances/profiles | 33 passed | `sdk-ts.log` |
| Rust SDK exact task and repository provenance | 2 passed | `sdk-rust.log` |
| Web/SDK TypeScript, browser fixture TypeScript, Web production build | Passed | `typecheck-merged.log`, `sdk-ts-typecheck.log`, `web-build-final.log`; final merged Web and fixture checks also ran successfully |
| Changed Web ESLint, new modules' formatting, new Rust formatting, diff whitespace | Passed | `format-new-rust.log`; CLI checks |
| Clippy, cma + layer4-runtime, all targets, warnings denied | Passed | `clippy-final.log` |
| Changed production source <=500 lines; append-only Web Style | Passed | `source-gate.log` |
| Scoped new patch and own non-merge commit secret scans | No leaks | `secrets-patch.log`, `secrets-history.log` |

The two existing ignored runtime cases require the genuine external `gh` CLI
and a separately supplied pinned skill archive. The new native repository proof
ran explicitly and was not skipped. No full-repository green claim is made.

Reproduction uses Vitest 4.1.12 with
`--configLoader=runner --no-cache --maxWorkers=2`. Browser tests use the actual
built Web app at loopback port 4344, a fresh Chrome context and
`e2e/agent-settings.config.ts`, filtered to
`LOCAL (composer settings|new task|contextual task|access approval|landing)`.
Rust uses `/Users/liuyitian/.cargo/bin` on PATH, `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/Users/liuyitian/code/cma-ui-worktrees/cargo-target`, and
`RUSTFLAGS='--cfg cma_ui_ticket04'`. The Mongo suite additionally uses
`RUST_MIN_STACK=16777216`,
`CHRONO_TEST_MONGODB_URI=mongodb://127.0.0.1:27404/?replicaSet=rs0` and
`cargo test --locked -p cma task_home_tests -- --include-ignored --test-threads=1 --skip task_home_create_child`.
Runtime tests use `cargo test --locked -p layer4-runtime`; SDK Rust uses
`cargo test --locked --test initial_task` from its own crate. Named nonzero
counts and candidate provenance were inspected, not only command exit status.

## Actual Linux/native proof

Immutable candidate source: `c104179b0` (04 plus integration through 14).
Later changes only extract public OpenAPI composition and Console concerns,
update browser fixtures and documentation; runtime/adapter source and Cargo
lock have no delta from that candidate.

- Source/binary artifact directory: `/Users/liuyitian/code/cma-ui-worktrees/ticket04-runtime-final`.
- Candidate binary SHA-256: `f42fb1e729283b80741f09f3565604e8f2268c315be114fc18eada8f3d0bf3cb`.
- Final minimal image ID: `sha256:b8fb789ea6e1e6a50f6faa4d34df90a49856823e8f28526cdc7ac211ebe907d4`.
- Base image: `sha256:22071f29fff5db494c8e5633a74692ebae442d7c2c7a0856e2d15ff7f9802057`.
- Pinned Codex: `codex-cli 0.147.0`; ACP 1.4.0 bridge SHA-256:
  `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c`.
- Build/image logs: `linux-final-build.log`, `linux-final-image.log`.
- Execution: `docker --context colima-cma-ui-proof run --rm --name cma-ticket04-runtime-proof --network none cma-ui-runtime-proof:ticket04-final`.

The test creates a real isolated repository and Git archive. The actual candidate
runtime installs the existing proxy credential, resolves the test ref, prepares
the checkout, restarts, replays the same receipt and creates the Agent worktree.
The actual pinned ACP/Codex process starts in that returned worktree with the
existing autonomous permission mode. A controlled model response invokes the
native execution tool; the returned output must include the known source bytes
and actual Git HEAD, and a first-task marker must exist. These are execution
observations, not fixture-only final file state.

Observed upstream source: `2d8e09c0b5dcfb0fdfce5aa82a33bc4ac43da479`;
actual checkout: `c6f6c3033b3c08734252d261e494773a3513beee`;
moved ref: `6f53d398221b2f3436062630588ded7ec20ea770`.
One resolution, one archive, `firstTaskExecuted:true`. Existing sibling bytes
remain intact. Inaccessible repository, missing ref, unsafe symlink and
incompatible destination are rejected. Killing preparation during archive
download yields `workspace_prepare_outcome_unknown` after restart and
`reResolvedAfterRestart:false`; the previously completed receipt still replays.

U/A/R are complementary approved seams: browser and public tests own creation,
authorization and exact dispatch; R owns real preparation and native execution
in the resulting directory. External GitHub/model responses are controlled and
no production GitHub is touched. This minimal Linux image is not the full
production security/software image, nor evidence of a deployed public route
connected to a live model or live OAuth/GitHub availability.

## Integration repairs and separate findings for parent/19

1. **01/03 introduced navigation/creation regressions, repaired:** AppCreation
   follows Settings → Manage environments → Workspace New agent and asserts
   exact Workspace/task binding. Legacy new/contextual browser tests now use the
   actual task composer and explicit recovery, retain exact body/key assertions,
   and verify the resulting Agent. Signed-in landing asserts the real task draft
   and navigation across resizing; signed-out compact navigation remains tested.
2. **03/05/14 source growth, repaired:** ConsoleLanding owns Home/fallback
   selection; ConsoleCreationAccess owns creation/access permissions. Console is
   494 lines. All 05 title hooks and 14 computer state/observation remain intact;
   no title ownership conflict required coordination. OpenAPI composition moved
   into a cohesive module, leaving its schema inventory at 498 lines (the merged
   file was already 503 before 04's capability declaration).
3. **14 introduced fixture reads, repaired within the creation regression
   boundary:** final browser rerun initially found four cases reporting new
   `/environment` and `/files/tree` reads as unexpected. LocalStartup now supplies
   explicit read-only fixture responses matching the public observation/file
   shapes, including unsupported browser/replay/control. All existing write,
   unexpected-request and error assertions remain; final 7/7 passes. No computer
   capability implementation or private file-scope change was made.
4. **Original LocalAgentV2 fixture defect, confirmed and left for 19:** the
   isolated original `60fc98ee5` Web build reproduced cold 390px idle and text
   scaling failures. An additional actual browser observation explicitly
   recorded `GET /api/v2/agents/agt_contract/events/head` returning 404 on that
   original baseline (`browser-head-attribution.log`, 1/1). Fixture/head-loader
   source matches the pre-14 integration. These cases are not relabeled as
   repaired or silently bypassed. Baseline archive and observational check are
   outside the repo in `ticket04-browser-baseline`; LocalAgentV2 is unchanged.
5. **14 OpenAPI documentation inventory omission, separate integration defect
   for 19:** broader `rest::openapi::tests` runs 10 named tests: 9 pass and the
   inventory test fails for `GET /api/v1/agents/{id}/environment`. Integration 14
   documents it in prose but omits the required inventory table row. The route,
   inventory test and omission are inherited unchanged; 04's composition
   extraction does not introduce it (`openapi-final.log`). It is not an original
   baseline defect and is not masked or fixed outside the assigned scope.
6. Original formatter discrepancies, aggregate skill-error fixture mismatch and
   508-line `agent_operation_service.rs` remain untouched. The seven disclosure
   regressions belonged to 14 and its merged fixes are preserved.

The worktree and proof artifacts remain for the separate merger. Only ticket04
services are stopped after validation; the shared proof VM remains available.

## Changed-path manifest

Relative to integration a426b87ee: 64 paths.

```text
api/crates/cma/src/agent_lifecycle_profiles.rs
api/crates/cma/src/agent_lifecycle_progress.rs
api/crates/cma/src/agent_lifecycle_validation.rs
api/crates/cma/src/sandbox/agent_worker_tests/sdk_conformance_tests/task_home_tests.rs
api/crates/cma/src/sandbox/agent_worker_tests/sdk_conformance_tests/task_home_tests/environment.rs
api/crates/cma/src/sandbox/warmup/hierarchy.rs
api/crates/cma/src/sandbox/workspace_worker.rs
api/crates/cma/src/sandbox/workspace_worker/support.rs
api/crates/cma/src/store/native_prepared/tests/workspace_proof_tests.rs
api/crates/layer1-public/src/rest/agent_lifecycle.rs
api/crates/layer1-public/src/rest/agent_openapi_shapes.rs
api/crates/layer1-public/src/rest/instances_openapi_shapes.rs
api/crates/layer1-public/src/rest/openapi.rs
api/crates/layer1-public/src/rest/openapi/composition.rs
api/crates/layer1-public/src/rest/profiles_openapi_shapes/content.rs
api/crates/layer2-core/src/profiles/workspace.rs
api/crates/layer2-core/src/sandbox/binding/profile.rs
api/crates/layer2-core/src/sandbox/workspace.rs
api/crates/layer2-core/src/sandbox/workspace_service/support.rs
api/crates/layer3-adapter/src/codex/repository_task.test.cjs
api/crates/layer3-adapter/src/hosted/memory_workspace.rs
api/crates/layer3-adapter/src/wire_tests.rs
api/crates/layer3-adapter/src/workspace.rs
api/crates/layer4-runtime/src/github_proxy.rs
api/crates/layer4-runtime/src/main.rs
api/crates/layer4-runtime/src/repository_snapshot.rs
api/crates/layer4-runtime/src/workspace_journal_tests.rs
api/crates/layer4-runtime/src/workspace_journal_validation.rs
api/crates/layer4-runtime/src/workspace_runtime.rs
api/crates/layer4-runtime/src/workspace_source.rs
docs/API_REST.md
docs/Web_Style.md
docs/plans/cma-ui-optimization/04-EVIDENCE.md
docs/plans/cma-ui-optimization/issues/04-repository-context.md
sdk/rust/src/agents/types.rs
sdk/rust/src/instances/types.rs
sdk/rust/src/profiles/types.rs
sdk/rust/tests/initial_task.rs
sdk/typescript/src/agents/types.ts
sdk/typescript/src/instances/types.ts
sdk/typescript/src/profiles/types.ts
sdk/typescript/tests/agent-lifecycle.test.ts
web/e2e/agent-settings.check.ts
web/e2e/agent-startup-fixtures.ts
web/e2e/shell-layout.check.ts
web/src/AppCreation.test.tsx
web/src/Console.tsx
web/src/ConsoleCreationAccess.ts
web/src/ConsoleLanding.tsx
web/src/agent/Details.test.tsx
web/src/agent/Details.tsx
web/src/api/client.ts
web/src/api/rest.ts
web/src/api/rest/hierarchy.ts
web/src/api/types.ts
web/src/create/ProfilePicker.tsx
web/src/create/TaskHome.test.tsx
web/src/create/TaskHome.tsx
web/src/create/lifecycleIntent.ts
web/src/create/taskDraft.ts
web/src/profiles/client.ts
web/src/profiles/marketplace/RevisionContents.tsx
web/src/profiles/pickers/WorkspaceLocationPicker.tsx
web/src/trigger-publisher/model.ts
```

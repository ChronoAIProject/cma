# Bounded integration repairs — tickets 01 / 03 / 07 / 10 / 14

This is the authorized repair batch within the existing 19-ticket workflow, not
an additional ticket or the final standards/spec review. Work is isolated to
`codex/cma-ui-integration-repairs`, initially based on `d19c225bd`; the original
workspace and the integration worktree remain untouched by this worker.

## Scope and agreed seams

- Preserve the computer drawer and embedded file layout while giving each CSS
  class one stylesheet owner; no collision allowlist or policy changes.
- Extract cohesive filesystem access guards and live reasoning folding to keep
  production files below 500 lines without changing their behavior.
- Complete the environment operation's shared JSON inventory, real public
  handler corpus and typed SDK dispatch. The REST Markdown inventory is already
  fixed separately by `54a7414b6`.
- Repair stale Home/navigation and event URL assertions through the existing
  rendered App and public client seams, retaining all no-creation/no-launch
  authority assertions.
- Avoid task-evidence queries for a known non-Ready Agent status. Validate the
  public inventory/status behavior, Ready completion/queued/corruption handling,
  and the separately measured Mongo query scaling; do not claim production
  latency from a synthetic query probe.
- Complete ticket 07's four delivery operations in the shared JSON inventory,
  real-handler corpus and typed SDK dispatch, preserving independent resource
  access, selected revision/hash identity and download reauthorization.
- Extract ticket 10's reusable feedback memory/seed fixture from its overlong
  test source. Keep the existing filename limits and behavioral tests intact.

## Initial reproduced failures and repairs

Evidence logs are outside the worktree at `/tmp/cma-integration-repairs/`.

- `web-red.log`: 3 files, 7 failed / 55 passed. Six AppPublic assertions assumed
  the old Home title and no session storage; task identity now records only the
  viewer marker. The copy check also exposed the visible label `Retained task`
  and mistakenly scanned a multi-class CSS attribute as visible text.
- `rest-web-red.log`: 1 failed / 22 passed. The event read correctly requests
  `tool_output=true`; the old expected URL omitted it. These are all eight
  integration failures identified by ticket 07, with its separate upstream
  reproduction recorded in that ticket's handoff.
- `home-green.log`: 3 files / 62 tests passed. The App renders the actual task
  composer for an owner, preserves the visitor landing and checks the exact
  viewer marker, empty task, enabled explicit action, zero resource mutations,
  zero launches and no unexpected requests across remounts. Visible copy uses
  `Submitted task`; the copy scanner still checks visible attributes and its
  example now verifies that multiple CSS class names are not user-facing copy.
- `css-red.log` reproduces the `.agent-files-panel` collision. The embedded
  modifier now belongs to `AgentFiles.css`, retaining the same flex, height and
  min-height declarations; `css-green.log` passes with the existing three
  deliberate shared classes. No design rule or Web Style entry is added.
- `structural-web.log`: 4 files / 41 tests passed, including the public event
  URL, conversation folding/performance and retained computer panel behavior.
  Live reasoning and keyed upsert are extracted without changing their bodies.
  Filesystem path and mutation-authority guards move together into a private
  scope module; protected homes, foreign uploads and runtime filters remain.
- `inventory-red.log`: the exact named shared JSON inventory test ran **1 test**
  and failed with 228 actual / 227 registered operations before the later
  ticket merges. This is distinct from the already-fixed REST Markdown row.

## Isolation and completion status

Web uses the existing linked dependencies with `--configLoader=runner
--no-cache --maxWorkers=2`. Rust uses jobs=2 and the private, non-symlink
`/Users/liuyitian/code/cma-ui-worktrees/cargo-target-repairs`, APFS-cloned from
the integration cache, then cleaned for cma/layer1-public/layer2-core/layer3-adapter
before recompilation. The retained cache flag is `--cfg cma_ui_ticket04`; target
isolation, clean output and compilation paths establish this worktree's binary.
Zero-test runs never count as evidence.

## Environment shared contract repair

The exact operation is registered with its real authorization problems
`not_authenticated` and `resource_not_found`. `agent-environment.json` adds ten
actual handler cases: owner, read-only accepted link, writable accepted link,
runtime not ready, unsupported mounted home, paused runtime, foreign subject,
revoked link, anonymous request and missing Agent. Cases retain private/no-store
headers, exact Agent identity, policy revision, opaque runtime incarnation,
renewal deadline, file mutation authority and read-only output/browser/replay/
control facts. Only the live link renewal deadline is captured symbolically.

- `environment-record.log` records the real handlers; an independent non-recording
  replay in `environment-handler-green.log` passes **1 named test / 10 cases**.
- TypeScript dispatch initially fails all ten cases and the completeness test
  (`environment-sdk-ts-red.log`, 11 actual failures). The repaired typed method
  passes the entire corpus: **2,747 tests**, `sdk-ts-green.log`.
- Rust initially runs **1 failing test**, reporting the unmapped environment
  operation (`environment-sdk-rust-red.log`). The typed method and structural
  equality of the exported environment models give **1 passed / 10 typed HTTP
  cases**, then the complete corpus gives **1 passed / 2,302 typed HTTP, 173 typed
  preflight, 240 unrepresentable wire-shape and 27 browser-owned cases**
  (`environment-sdk-rust-green.log`, `sdk-rust-all.log`). SDK TypeScript typecheck
  passes. Neither SDK bypasses the typed method with a raw HTTP call.
- At the 07/12/10 merge, the **global** inventory remains separately blocked by ticket 07's four
  unregistered delivery operations. Before this repair it actually ran **1 failed
  test**, with **234 actual / 229 listed** (`inventory-merged-red.log`). This repair
  adds the environment entry and raises the exact listed count to **230**; it does
  not count absent delivery entries as covered. Those routes and ticket 10's two
  feedback routes remain intact. This is a new integration gap, not an original
  skill-fixture failure and not a passing global gate.

## Merged Web result

`web-all.log` passes the complete merged suite: **329 files / 2,963 tests**, no
committed skips or test exclusions. Web typecheck, eslint and the unchanged CSS
collision gate pass (`web-typecheck.log`, `web-lint.log`, `css-final.log`). jsdom
prints its existing cross-Document navigation notices without failing tests.

The first source-size pass no longer reports either repaired production source:
`agent_filesystem.rs` is 473 lines and `conversation.ts` is 469. It reports the
unchanged original 508-line `agent_operation_service.rs` and the newly merged
ticket-10 `agents_tests/message_feedback.rs` at 556 lines. The later explicitly
authorized 07/10 follow-up below resolves the latter overrun.

## Task status query repair

The independent probe (`query-probe.jsonl`) repeats the parent's exact Mongo
query and `{agent_id: 1, seq: 1}` index with 10,000 intervening tool updates:
MongoDB 8.0.34 returns one result after examining **10,001 keys and documents**.
The probe's unique database was dropped. Its 7 ms synthetic debug observation
is not a production latency benchmark.

`agent_task_status::read` now returns the existing domain projection immediately
for non-Ready status. It introduces no second status mapping. Ready Agents keep
the original latest-event and pending-command reads, validation and reducer.
Thus incomplete or corrupt evidence still cannot certify a Ready Agent as done.

The existing Mongo shared-inventory corruption regression now also exercises the
public `HierarchyReadStore` and `AgentProvenanceInventoryStore` seams. It checks
completed Ready state; rejects mismatched log identity and malformed queued
command evidence for Ready; and preserves Starting, Working, WaitingApproval,
Paused, Stopped and Failed observations when that unused evidence is corrupt.
It restores the original fixture before checking revoked-grant behavior. There
is no test of the private read helper or copy of its implementation, and no new
ignore/skip annotation.

- `status-red.log`: **1 real failing test**, with non-Ready hierarchy reading the
  irrelevant corrupt log and returning `Corrupt`.
- `status-green.log`: the same exact named regression passes **1 test**, including
  both corruption paths and both public status projections.
- Mongo is the shared `cma-ticket11-mongo` fixture on `colima-cma-ui-proof`, port
  27027, replica set `ticket11`; all mutation is in this test's PID/sequence-scoped
  database. The shared container and VM are not stopped or reconfigured.

## Final integration verification and handoff

The repair branch contains integration `a2965df09581de5fa0fa35af54b69db56cd83ab9`,
including 07 → 12 → 10 → 13 and the merger's composition inventory correction.
Merge `def14528e` resolved one adjacent module declaration conflict by preserving
both 07's `read_check` and the extracted `scope`. Merge `65bbccd34` brought in the
first batch's latest composition checks and evidence without conflicts. Merge
`0cf5ce754` preserves ticket 13 at `83ea396a4d49535cbc1484034b34f2b3202b3bb2`,
and `afae935e3` includes its final integration evidence. Ticket 13's literal human
quality signoff remains unchecked and is not this repair batch's blocker. No
integration or active-worker source was written by this worker.

Final affected Rust checks use Rust 1.96.1, `--offline --locked`, jobs=2,
`RUST_MIN_STACK=33554432` and the private target described above:

| Check / filter | Actual result | Log in `/tmp/cma-integration-repairs/` |
| --- | --- | --- |
| layer1-public `task_navigation` | 6 passed after 13, including pending-work, completion, authority and shared navigation | `navigation-after13.log` |
| layer1-public `filesystem` | 5 passed after 13; existing ticket-07 native runtime proof remains 1 ignored and is not counted as evidence here | `filesystem-after13.log` |
| layer2-core `sandbox::tests::filesystem` | 4 passed before 13: environment scope, owner/cross-owner files, revoked full-home consent and typed link read/write authority; the repaired scope module is unchanged by 13 | `core-filesystem.log` |
| layer1-public `rest::openapi::` | 20 passed after 13, including REST Markdown inventory and merged composition counts | `openapi-after13.log` |
| layer1-public environment shared corpus | 1 passed / 10 cases after 13 | `environment-after13.log` |
| layer1-public suggestions shared corpus | 1 passed / 6 cases after 13 | `suggestions-after13.log` |
| cma public hierarchy/shared-inventory status regression | 1 passed after 13; 1,606 filtered, no zero-test claim | `status-after13.log` |
| layer1-public exact shared JSON inventory, before 07 follow-up | 1 failed after `a2965df09` merge: 235 actual / 231 registered; the four ticket-07 delivery routes were missing | `inventory-final-tip.log` |

The first Core filter used the source filename rather than its registered module
and selected zero tests (`core-filter-miss.log`). It is excluded from evidence.
`core-test-list.log` supplied the actual module name; the corrected run above
contains all four named nonzero tests. SDK Rust likewise uses a separate private
APFS-cloned/cleaned target `cargo-target-repairs-sdk` and retained cache cfg
`cma_ui_ticket10`; `sdk-clean.log` and compilation paths identify its source.

The copy scanner received one further focused red/green guard: event-handler
error sentences must still be scanned while `className` is excluded. The example
fails before that narrower rule (1 failed / 32 passed) and then passes all 33
copy tests (`copy-scanner-red.log`, `copy-scanner-green.log`). No rule excluding
other JSX attributes remains. This final test-only adjustment passes Prettier;
the prior full Web run is supplemented by this focused verification and the
complete post-13 Web run below.

After the final requested ticket-13 merge, the complete Web suite passes
**330 files / 2,974 tests** (`web-after13.log`). Both shared SDK corpora retain
the environment and all six incoming suggestion cases: TypeScript **2,753 tests**
pass (`sdk-ts-after13.log`); Rust **1 test / 2,307 typed HTTP, 173 typed preflight,
241 unrepresentable wire-shape, 27 browser-owned cases** passes
(`sdk-rust-after13.log`). The inventory contains 231 actual registered OpenAPI
entries, including both environment and suggestions; `f450bfab5` reconciles their
parallel count additions without claiming the four absent delivery entries.
Post-13 Web and SDK TypeScript typechecks, Web lint and the unchanged CSS gate
also pass (`typecheck-after13.log`, `sdk-ts-typecheck-after13.log`,
`lint-after13.log`, `css-after13.log`). That run's `file-size-after13.log` reports
the baseline file and the ticket-10 overrun subsequently repaired below. No
checks are suppressed.

The four previously missing shared operations, now covered, are:

```text
POST /api/v1/agents/{id}/deliverable-references
GET /api/v1/agents/{id}/answers/{answer_id}/deliverables
GET /api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}
GET /api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}/download
```

## Authorized 07/10 follow-up — final owned gates

The parent confirmed that the delivery omissions and feedback overrun were new
19-ticket integration defects owned by this repair batch. The inventory failure
was reproduced as one named test, 235 actual / 231 declared operations
(`deliveries-inventory-red.log`). TypeScript's completeness check failed on the
unmapped list operation (`deliveries-sdk-ts-red.log`); Rust's named shared replay
failed on the same missing dispatch (`deliveries-sdk-rust-red.log`). These are
actual red runs, not inferred failures or zero-test selections.

`deliverable_cases.rs` reuses ticket 07's public-test composition and the
deterministic filesystem boundary used by its native fixture. The real service
publishes an artifact, creates a document, verifies a mounted-home file, and
appends their explicit markers to a completed final answer. The new shared
`answer-deliverables.json` contains **44 cases** across all four operations:
three resource kinds; owner and authorized viewer reads; independent document
denial and revocation; foreign and anonymous callers; nonfinal/missing targets;
invalid revision/answer identity; unavailable capabilities; original document
bytes after a later edit; and refusal of changed live-file bytes. JSON bodies,
named problems, digest/revision identity and private download headers are
recorded from the actual handlers and then replayed without recording.

Both SDKs invoke their typed delivery methods. Successful downloads consume
real shared metadata, opaque bytes, then a second metadata read, with every
request and exact total checked. Refused downloads pair a successful earlier
metadata observation with the real refusal case, representing access/session
loss or unavailable/changed bytes after precheck. No synthetic metadata is
substituted. Three invalid-input cases exercise actual SDK preflight and decode
the corresponding backend error; they are not counted as typed HTTP replay.
The TypeScript expected URL encodes the answer identity's colon exactly as its
typed path builder does; Rust emits the equivalent literal-colon path. Other
operations retain their one-request contract and exact assertions.

The feedback extraction moves the existing memory-store implementation and
seeded completed-answer fixture into `message_feedback_fixture.rs` (**123
lines**). The original `message_feedback.rs` remains under its original name at
**438 lines**, retaining its behavior tests and reasons submodule. This changes
neither test classification nor the size gate. No production feedback behavior
or database service changes are needed.

| Final affected check | Actual result | Log in `/tmp/cma-integration-repairs/` |
| --- | --- | --- |
| Exact shared inventory | 1 passed; **235 actual / 235 declared** operations, 2,792 cases, 1,599 named HTTP problem pairs, 23 protocol failure pairs | `deliveries-inventory-final.log` |
| Delivery real-handler shared corpus | 1 passed / **44 cases**, recording disabled | `answer-deliverables-final-public.log` |
| Existing delivery public regressions | **5 passed**; existing native proof remains ignored and is not claimed as rerun | `deliveries-public-regressions.log` |
| Existing feedback public/schema regressions | **12 passed** | `feedback-extraction-regressions.log` |
| Feedback reasons and old vote shared handlers | 1 passed / **26 cases**, then 1 passed / **18 cases** | `feedback-reasons-final-public.log`, `message-feedback-final-public.log` |
| TypeScript full shared corpus + delivery/feedback SDK regressions | **3 files / 2,809 tests passed**, including 2,797 shared-harness tests | `deliveries-feedback-sdk-ts-final.log` |
| Rust full shared typed corpus | 1 passed; **2,348 typed HTTP, 176 typed preflight, 241 unrepresentable wire shapes, 27 browser-owned** cases | `deliveries-sdk-rust-final.log` |
| SDK TypeScript typecheck, changed-test lint and formatting | passed | `deliveries-sdk-ts-typecheck-final.log`, `deliveries-sdk-ts-lint.log`, `deliveries-sdk-ts-format.log` |
| Unchanged source-size gate before 08 | Only the original 508-line `agent_operation_service.rs` remains; **1 / 4,472** files reported | `file-size-deliveries-feedback.log` |

The repair code commit is `5d957af04`. During the final merge, integration advanced
to `7cd701d0fe6c2496256a01eca59919ee8d899c82` with ticket 08's preview work. Merge
`bdf59333e` incorporates it cleanly in this worktree (`merge-final-owned.log`),
also retaining the required `a2965df09` ancestry. All incoming 07/10/13 routes and
cases remain present. Ticket 15's new browser operations remain its worker's
responsibility and are not part of this integration tip.

The incoming preview changes received focused composition validation:
**8 files / 85 Web tests passed**, including preview, delivery, AppPublic, task
home and REST checks (`preview-merge-web.log`); Web typecheck and the unchanged
CSS gate passed, now **899 classes / 3 existing exceptions**
(`preview-merge-typecheck.log`, `preview-merge-css.log`). The exact shared inventory
passed again at **235/235** (`preview-merge-inventory.log`). The size gate now
reports only the original service, **1 / 4,480 files**
(`preview-merge-file-size.log`). This worker changes no additional frontend source;
the prior complete Web run **330 files / 2,974 tests** plus this focused merge
check remain the Web evidence. No complete Web rerun was needed. The committed
range secret scan passes (`gitleaks-deliveries-final.log`); `git diff --check`
passes. The repair-only manifest is also saved as `repair-manifest.txt` alongside
the logs.

## Ticket 18 integration delta

Merge `743a1732d` incorporates the requested integration
`eab5011c02d32959512ddd2ed09f7af618e46526` without conflicts, retaining ticket 08's
preview and all existing repairs. Ticket 18 adds six shared cases and replay
semantics to existing v2 head/snapshot/tail operations; it does **not** introduce
an additional HTTP operation. The unchanged exact inventory remains **235/235**.

The real environment replay exposed one stale corpus expectation
(`ticket18-environment-red.log`, one failed named test). Ticket 18 now advertises
authorized read-only replay as `available` / `operation_records_only` /
`agent_turn_history`. Only those three fields in the six successful environment
cases change; `writable: false`, browser/control availability, file scope,
incarnation, authority failures and timestamp captures remain intact. Recording
mode was not used. This is the only additional repair-code delta after
`a1d0685c9`; the other edited file is this handoff.

While finishing, the integration branch also received the independent legacy
Responses compatibility repair. Merge `8fbe79bef` includes its code at
`1440a107b`; merge `0b1ed6b54` includes its documentation tip
`134c27aa0892ec2422fb775b69bf93c24ec76def`. The latter adds only an IMPLEMENTATION
handoff, so the preceding code checks remain applicable. No UI conflict or local
frontend change occurred; the prior Web evidence is retained without rerunning
the full suite. All prior repair evidence above remains historical evidence.

| Latest affected check | Actual result | Log in `/tmp/cma-integration-repairs/` |
| --- | --- | --- |
| Exact shared inventory on latest code | 1 passed; **235 operations, 2,798 cases, 1,599 named HTTP problem pairs, 23 protocol failure pairs** | `ticket18-inventory-final.log` |
| Replay shared real handlers | 1 passed / **6 cases** | `ticket18-operation-replay-public.log` |
| Environment shared real handlers after the expectation repair | 1 passed / **10 cases** | `ticket18-environment-green.log` |
| Delivery shared real handlers | 1 passed / **44 cases** | `ticket18-answer-deliverables-public.log` |
| Public replay boundary/no-command regression | **1 passed**, exact named test | `ticket18-public-replay-regression.log` |
| Core environment/filesystem authority regressions | **4 passed**, registered `sandbox::tests::filesystem::` filter | `ticket18-filesystem-core.log` |
| Complete TypeScript shared typed corpus and typecheck | **2,803 tests passed**; typecheck passed | `ticket18-sdk-ts-final.log`, `ticket18-sdk-ts-types-final.log` |
| Complete Rust shared typed corpus | 1 passed; **2,354 typed HTTP, 176 typed preflight, 241 unrepresentable wire shapes, 27 browser-owned cases** | `ticket18-sdk-rust-final.log` |
| Incoming Responses compatibility public regressions | **2 passed**; existing loopback serving fixture ignored, not claimed as rerun | `ticket18-latest-compat-regressions.log` |
| Unchanged size gate on latest code | Only untouched baseline `agent_operation_service.rs`, **508 lines; 1 / 4,487 files**; merged `agent_filesystem.rs` is 448 lines | `ticket18-latest-file-size.log` |

The repair-only manifest remains **40 paths** against the latest integration tip.
`git diff --check` passes. Merge logs are `merge-ticket18.log`,
`merge-response-compat-tip.log`, and `merge-ticket18-final-tip.log`; final audit
logs are `ticket18-final-tip-confirmation.log` and `ticket18-gitleaks.log`.
No worker-owned service was started; the shared Mongo/VM and original/integration
worktrees remain untouched.

The query-probe database and the failed-red fixture database
`cma_804_shared_provenance_corrupt_5560_0` were dropped after verifying the unique
fixture marker (`mongo-cleanup.json`). The green Mongo test drops its own
PID/sequence database. This worker started no persistent service; the shared
Mongo container/VM stays running. The worktree and private build/evidence paths
remain for parent cleanup.

Remaining limitations are explicitly separated:

- **Owned new integration defects:** none remain in the assigned gates. The four
  delivery entries have actual shared cases/typed dispatch, and the feedback
  source overrun is resolved by cohesive fixture extraction.
- **Original and untouched:** public/Core skill fixtures, LocalAgentV2 head-404,
  and formatter drift remain the earlier tickets' documented limitations, not
  newly rerun failures. The original 508-line `agent_operation_service.rs` is
  byte-identical to `c9e2d7b91` (SHA-256
  `fdf19898a69c9f918a872372a2b99762d2c1fc4d40dbb1d77925f5ccad81ccdc`).

The owned repair work is complete; this does not declare the whole repository,
native runtime, all 19 tickets or the final standards/spec review complete. No
push, deployment, PR, original-workspace mutation or shared-service shutdown
occurred.

## Repair manifest

40 paths differ from integration `134c27aa0892ec2422fb775b69bf93c24ec76def`.
Incoming feature changes are excluded from this repair-only manifest.

```text
api/crates/cma/src/store/agent_task_status.rs
api/crates/cma/src/store/sandbox_mongo_integration_tests/resource_access_tests/shared_inventory_tests.rs
api/crates/cma/src/store/sandbox_mongo_integration_tests/resource_access_tests/shared_inventory_tests/task_status.rs
api/crates/layer1-public/src/rest/agents_tests/message_feedback.rs
api/crates/layer1-public/src/rest/agents_tests/message_feedback_fixture.rs
api/crates/layer1-public/src/rest/public_contract_tests.rs
api/crates/layer1-public/src/rest/public_contract_tests/deliverable_cases.rs
api/crates/layer1-public/src/rest/public_contract_tests/filesystem_cases.rs
api/crates/layer1-public/src/rest/public_contract_tests/inventory.rs
api/crates/layer1-public/tests/fixtures/public-contract-inventory.json
api/crates/layer1-public/tests/fixtures/public-contract/agent-environment.json
api/crates/layer1-public/tests/fixtures/public-contract/answer-deliverables.json
api/crates/layer2-core/src/sandbox/agent_filesystem.rs
api/crates/layer2-core/src/sandbox/agent_filesystem/scope.rs
docs/plans/cma-ui-optimization/10-EVIDENCE.md
docs/plans/cma-ui-optimization/14-FILE-SCOPE.md
docs/plans/cma-ui-optimization/INTEGRATION-REPAIRS.md
docs/plans/cma-ui-optimization/issues/01-task-navigation.md
docs/plans/cma-ui-optimization/issues/03-task-home.md
docs/plans/cma-ui-optimization/issues/07-answer-deliverables.md
docs/plans/cma-ui-optimization/issues/14-computer-files-activity.md
sdk/rust/src/agent_environment.rs
sdk/rust/tests/public_contract.rs
sdk/rust/tests/public_contract/classification.rs
sdk/rust/tests/public_contract/deliverables.rs
sdk/rust/tests/public_contract/resources.rs
sdk/typescript/tests/public-contract.test.ts
sdk/typescript/tests/public-contract/deliverables.ts
sdk/typescript/tests/public-contract/expectations.ts
sdk/typescript/tests/public-contract/resources.ts
web/src/AppPublic.test.tsx
web/src/agent/conversation.ts
web/src/agent/conversationValues.ts
web/src/agent/environment/AgentComputer.css
web/src/agent/files/AgentFiles.css
web/src/agent/files/FileExplorerFrame.tsx
web/src/agent/liveReasoning.ts
web/src/api/rest.test.ts
web/src/create/TaskHome.tsx
web/src/vocabulary/publicCopy.test.ts
```

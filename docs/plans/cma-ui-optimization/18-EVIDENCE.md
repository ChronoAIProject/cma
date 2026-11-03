# Ticket 18 — operation record replay evidence

Date: 2026-11-03. Branch: `codex/cma-ui-18`. Worktree:
`/Users/liuyitian/code/cma-ui-worktrees/ticket-18`.
Started from `d19c225bd2a373f0e4861e95ebbbb5a5bca8adc8`; merged integration
`a2965df09581de5fa0fa35af54b69db56cd83ab9` in `1dd75c6f4`.
Implementation and shared-corpus verification source: `bb139fbf8a5dac25d467fefc0d135a1fc946dcba`.
The original acceptance text and the user-approved U/A/R boundaries are unchanged.

## Delivered contract

The Computer's existing Operation output panel provides Operation record replay.
It consumes the existing typed public v2 snapshot, head and finite event-tail
contracts. There is no additional private first-party route or Chat model.
The v1 environment contract advertises `operation_records_only`, read-only
`agent_turn_history`, only with current ViewHistory authority. Browser and control
remain independently declared capabilities.

Identity comprises Agent incarnation, actual turn sequence, tool call identity
and durable event cursor. The UI retains at most 50 operation records and renders
one selected command with recorded output, state, timestamp and the plan known at
that point. Public pages retain their existing 1–200-event / 4 MiB boundaries.
Recorded output retains its upstream bound and explicit truncation indicator.
Historical folds stop at the selected cursor; an absent plan is shown as not
recorded. No current file content or browser image is substituted into history.

Previous/Next and ArrowLeft/ArrowRight navigate records; Return Live and End read
the latest fixed-head snapshot. The existing observation continues receiving live
work while a historical cursor stays fixed. Live catch-up uses a finite read from
the last durable cursor, without another persistent subscription. Session storage
holds only the scoped cursor, never cached content or authority. Reload performs
a fresh authorized read. Scope/authority transitions unmount or abort old reads.
Plan and completed-answer links carry the actual turn, including turns with no
plan or no commands. History offers no execution, recovery, approval, feedback,
suggestion or control action.

## Acceptance mapping

| Original criterion | Evidence |
| --- | --- |
| Durable identity, bounded public pagination | `replayIndex.ts`; 50-operation boundary test; real public fixed-head pagination test; six shared-corpus cases |
| Keyboard progress and honest operation labeling | Previous/Next/Return Live U tests and desktop/mobile Chrome R; no iframe/canvas in replay |
| Recorded command/output/state/time/plan | Reload test excludes future plan/output; new-turn and gap tests clear preceding plan; explicit missing/truncated output |
| Completed process and 06 links target the same turn | Actual-turn PlanCard and completed-answer tests, including no-plan and empty-command turns |
| History holds while live progresses, safe resume | First red/green; finite unread-tail test; bounded navigation; existing ordered/deduplicated public SDK event consumption; explicit gap/expiry tests |
| Agent/account/permission isolation | Scoped mount/request cancellation; account late-response test and revoked environment/history tests |
| Read-only replay, unavailable controls | Replay reader exposes only read methods; every U/R replay request is GET; physical native execution count stays four |
| Authorized reload, unavailable history explained | Exact saved-cursor Chrome reload; fresh authorization denial, expired cursor, gap/truncation U tests; no historical file/browser projection |

## Red → green and focused validation

Log root: `/Users/liuyitian/code/cma-ui-worktrees/ticket18-proof`.
The required first red and green are additionally retained at
`/Users/liuyitian/code/cma-ui-worktrees/ticket18-first-red.log` and
`/Users/liuyitian/code/cma-ui-worktrees/ticket18-first-green.log`.
The first behavior is exactly: hold operation 2, receive operation 4, remain on 2,
Return Live shows 4, and the four recorded executions gain no extra command.

| Gate | Final result and log |
| --- | --- |
| Replay, plan and memo/performance U | 3 files / 20 passed, including all 11 replay cases; `final-focused-web.log` |
| Merged Console/shared observation seam | 2 files / 34 passed; `merge-scope-fixed.log` |
| Public v2 read suite | 26 passed, one native test deliberately ignored in this ordinary invocation; `merged-public.log`; native test separately run below |
| Current environment and filesystem authority | 4 named tests passed; `final-core.log` |
| Shared real-handler corpus | 1 named test, six replay HTTP cases; `corpus-backend.log` |
| Shared TypeScript typed dispatch | Six replay cases plus loaded-fixture dispatch coverage, 7 passed; `corpus-ts-green.log` |
| Shared Rust typed dispatch | 1 named test, six typed HTTP cases; `corpus-rust.log` |
| Separate Markdown REST inventory | 1 named test passed; `rest-inventory.log` |
| Standalone SDK tests, separate from corpus | TypeScript 103 passed (`sdk-typescript.log`); Rust 9 event tests plus 1 file test (`sdk-rust.log`) |
| Web and E2E types | `merge-types.log`, `e2e-types.log` passed |
| Built web bundle | `merged-build.log` passed, existing large-chunk advisory |
| Targeted formatting/lint | `lint.log`, `format.log`, `final-format.log`, `final-rust-format.log` passed; API edition 2021, Rust SDK edition 2024 |
| Ticket commit secret scan | No leaks found; `final-gitleaks.log` |

The shared JSON inventory now explicitly identifies operation replay semantics on
the reused snapshot/tail routes. `public-contract/operation-replay.json` exercises
head-before/head-after, bounded first/next pages, reload and finite catch-up,
including commands, actual turn, recorded output/truncation and plan events.
Both SDK dispatchers send the typed `toolOutput` option rather than forwarding a
raw private request. Its red showed four missing-Prefer failures; the real handler
recording and final replay bind only generated timestamp fields through the
existing corpus symbol mechanism. Markdown API documentation and standalone SDK
tests remain separate gates. No new HTTP operation is introduced by ticket 18.
Incoming 07 delivery and 14 environment inventory repairs belong to the parent
repair worker; this branch preserves the integration inventory and adds no
exemption, replacement inventory or route-count relaxation.

Reproduction uses `--configLoader=runner --no-cache --maxWorkers=2` for Vitest.
Rust uses `cargo +1.96.1 test --offline --locked`, jobs 2 and private target
`/Users/liuyitian/code/cma-ui-worktrees/ticket18-proof/cargo-target`.
The private clone retains cached `--cfg cma_ui_ticket04`; it is never a shared
unisolated target. Named test counts were checked. An initial filename-based core
filter selected zero tests and was rejected; the final `sandbox::tests::filesystem::`
filter executes all four named tests. Shared-corpus selectors are
`CMA_PUBLIC_CONTRACT_FILE=operation-replay.json` and
`CMA_SDK_CONTRACT_FILE=operation-replay.json`; recording mode is off for verification.

## Native R proof

Authoritative R artifacts are in `ticket18-proof/final-runtime/`, produced on the
merged application source `1dd75c6f4`. Later changes add shared-corpus coverage and
correct targeted Rust formatting; they do not change the web/native replay path.

The image is the existing immutable ticket04+14 candidate:
`sha256:b8fb789ea6e1e6a50f6faa4d34df90a49856823e8f28526cdc7ac211ebe907d4`.
All Docker calls use `docker --context colima-cma-ui-proof`. The owned native
container uses no external network, 0.75 CPU and 768 MiB. No Layer 4 source changed,
so no competing Linux build or new candidate image was needed.

`operation_replay.test.cjs` runs actual Codex 0.147.0 through the pinned patched
ACP 1.4.0 bridge. A controlled local model emits four harmless shell commands.
Each command appends to a physical execution counter and emits its known
`native-replay-N` output. The native process stays alive while replay is tested.
The real DurableAgentUpdateRecorder writes those captured native updates into
the public test Agent. The real public router serves bounded output-bearing
pages over HTTP. Chrome reads those exact responses in the built application:
Live 4 → previous 3 → keyboard 2 → reload same cursor 2 → End/Live 4 → mobile 3.
It records zero non-GET API requests, and public head/pages remain unchanged.
Only then are the proof completion signals released.

- Native runner: 1 passed, no skips; `final-runtime/native.log`.
- Actual public handler/native proof: 1 named test passed, no skips;
  `final-runtime/public-native.log`.
- Desktop/mobile Chrome: 1 passed; `final-runtime/browser.log`.
- Physical execution count: **4 before / 4 after**; model calls after replay: **5**.
  `final-runtime/native-operations.json.verified.json`.
- Screenshots show recorded command output and read-only history. They are UI
  screenshots of this proof, never claimed as recorded historical browser frames.

| Artifact under `final-runtime` | SHA-256 |
| --- | --- |
| `native-operations.json` | `f4c52ce2bd861a0f1d55c7a5fbf187283b1a06a3938d26af51f3dfd359bb7157` |
| `native-operations.json.verified.json` | `14aad7cd46502ade3f9d918327994f16d419e85fcf1b1ba31ff5216b50544d4f` |
| `public-operations.json` | `2557188d9561d787a26d936eadb3e4c628ef548d47c52e4053374312be3f764e` |
| `native-operation-2-desktop.png` | `b0e553a7d5b619d3a51573e60fd4774cd480a67a80b6a52824a29060967b88c7` |
| `native-operation-3-mobile.png` | `80cc4b324a75a88fd89ecde4961ddd7d4342035c7ef6cc621d63609a27768116` |

PNG files are under `screenshots/operation-replay.check.ts--ee1ec-oad-without-extra-execution/`.
This proves pinned native execution, durable record projection through the actual
public handler, browser navigation and authorized fresh reload. The model boundary
is controlled; the HTTP test uses an in-memory store implementation and a known
owner context. Surrounding account/inventory and environment responses are browser
fixtures, with environment policy separately tested in core. This is not a
production login/deployment, a Mongo process-restart test or a production-packaged
image. The minimal test image omits production packaging. No video, PTY, recorded
browser frames, Browser private context or control feature is implemented.
Earlier exploratory logs (including an abandoned server shutdown timeout) are
not substituted for the clean final native/public/browser proof above.

## Integration and handoff boundaries

The merge preserves 07 deliverables, 10 feedback, 12 editing, 13 saved/contextual
suggestions, 06 plan links and 14 panel observation. The shared owner observation
remains available even on clients without replay; optional replay navigation is
gated independently. StreamEntry's existing memo comparisons are retained.
The integration Web Style content remains an exact prefix with the 18 appendix
at the end. The parent Browser worker owns its independent private context and
capabilities; ticket 18 does not depend on 15 or 17.

Every ticket-owned source is ≤500 lines and test source ≤2000. Console and
AgentSurface are 499; AgentFilesystem is 492 after moving pure environment helpers
to its existing model module. The full size gate still reports three files byte-
identical to integration `a2965df09`: `agent_operation_service.rs` (508),
`conversation.ts` (503), and `agents_tests/message_feedback.rs` (556; classified as
source by the existing gate). See `final-file-size.log`. These are not ticket 18
changes. Existing broader formatting/skill fixtures and original LocalAgentV2
head-404 limitations remain separate; this work does not claim a green full repo.

The owned preview, public proof server and native container are stopped. Shared
Colima and `cma-ticket11-mongo` remain running. Worktree, private target and proof
artifacts are retained for parent cleanup. No original/integration/other worker
source was written, and no push, deployment or PR was performed. The separate
merger integrates this branch. Final paths are listed in `18-FILE-SCOPE.md`.

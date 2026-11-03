# 03：在首页直接提交并可靠启动一项任务

**What to build:** 用户在首页输入一句任务就能进入自己的 Agent；创建、首条任务、刷新及响应丢失属于同一条可恢复链路。

**Blocked by:** None（评审确认后可独立开始）。

**Status:** done (2026-11-03; verified at the approved U/A/R boundaries, with runtime proof limits below)

**优先级:** P0

## Acceptance criteria

- [x] 登录后的首页呈现主输入框和调研/表格/文档/网页/代码等模板；模板只填入可编辑草稿，不创建 Agent、不自动发送。未登录仍走已有登录入口。
- [x] 首页、侧栏 New agent 和正常快捷入口进入任务输入，不再先弹配置表单；在已有 Workspace/Sandbox 中创建时保留明确上下文及原授权检查。
- [x] 发送冻结任务原文和创建上下文；服务端持久记录后给出 operation receipt，区分 operation ID 与 Agent ID，接受不等于执行完成。
- [x] 将首条任务纳入生命周期原请求与幂等比较；同 owner、同 key、同内容恢复同一结果，改内容复用 key 得到明确拒绝。连续点击、丢响应和 worker 重启不产生第二个 Agent 或第二条任务。
- [x] 自动 setup 与真实 task 的目的、时序和状态清楚；准备完成不能冒充用户任务完成。无 initial task 的旧客户端保持原有 setup 行为。
- [x] 一旦确切 Agent 身份可读即可进入准备中的会话并看见任务状态，无需等最终执行成功才导航；启动、连接授权、任务拒绝各有独立可恢复状态。
- [x] 绑定 viewer/origin/创建上下文的首页草稿在同一标签页刷新后恢复；已发送请求保留冻结原文/key/receipt，已受理操作以服务端记录为准。退出或换号清理，重新鉴权后才能恢复；不承诺跨设备草稿同步。
- [x] 未确认请求不可自动换 key 重建；网络拒绝保留原任务和后续新草稿，恢复旧请求的回调不会覆盖新输入。存储不可用时在发请求前提示并保留可复制文本。
- [x] 空白、超出约定字节上限、缺少创建权限均有可见错误；浏览器前进/返回、刷新和权限失效不自动重新提交。
- [x] 公开契约、持久记录兼容及运行时首任务行为随页面一起交付；服务端不支持本能力时显示可理解的不可用状态。

## Testing boundaries（2026-11-03 已确认）

- **U：** 首页输入→提交→准备中的 Agent→失败/恢复；覆盖同标签刷新、账号变化和晚到回调。不能单独证明服务重启后的投递。
- **A：** 公开创建/操作查询/Agent 历史，注入丢响应和服务重启后读取同一操作与唯一用户任务；不以私有存储断言替代历史。不能证明 provider 真正收到文本。
- **R：** 固定 runtime 的隔离 Agent 收到确切任务并返回结果，覆盖一次不确定发送恢复。不能穷举所有停机点，持久性故障矩阵仍由 A 覆盖。
- **首个 red → green：** 首页发送“比较三个工具”，创建被受理后刷新，通过同一操作进入唯一 Agent，历史只出现一次该任务。

## 范围与依赖说明

本票先完成无仓库、默认环境的一条完整路径；04 增加仓库，16 增加共享桌面。先核实现有 durable first-message/queue 机制，再决定 initial_message 的精确形状，禁止用前端收到 Agent ID 后的无记录补发替代原子业务意图。

## Implementation decisions and evidence

- U/A/R seams are already approved. Work is isolated to `codex/cma-ui-03` in the ticket-03 worktree; navigation remains owned by ticket 01.
- `initial_message` is exact UTF-8 text, bounded by the existing 262144-byte first-message limit. For this default-environment slice it replaces the automatic readiness prompt, with `purpose: task`; omission retains the original captured setup message and `purpose: setup`.
- Environment provisioning still precedes dispatch. Operation completion means provisioning readiness, not task completion; conversation delivery/history remain authoritative.
- Both Mac and the explicitly selected local Linux Docker context are now available. The pinned native provider proof ran on both; no production deployment, remote PR or real model-quality claim is made.

## Delivery and integration

- `05b0eddfa`: Web Style first, with appended task-home rules and changelog.
- `5a54e6119`: optional public `initial_message`, byte validation before admission, exact lifecycle request persistence, task/setup purpose and the first real Mongo/public-route proof.
- `fcbf2f6a8`: TypeScript and Rust SDK request contracts, exact-byte/key replay tests and API examples.
- `ae404896d`: Home/contextual task composer, session draft/account isolation, preparation navigation and explicit recovery.
- `d45cdd6df`: real default-environment provisioning, process/worker replacement and ambiguous-send public proofs; pinned native provider proof.
- `c57d58156`: damaged retained-request recovery and visible focus/error corrections.
- `a84757330`: merged integration `d656e542e0bfd7ff0e5c43f329f427e93aec6e79`, including tickets 01/09 and the separate baseline E0308 correction. The sole merge conflict was appended Web Style content. All integration text is retained as an unchanged prefix, followed by this ticket's section and changelog.

No source changes to Layer 3/4 runtime behavior were needed: the existing durable first-message writer, dispatch fence, ambiguous-write state and provider session recovery are reused. The new core entry accepts an existing `FirstMessagePurpose`; the old entry continues to choose setup. Optional-field omission preserves old request serialization/fingerprints and stored-record decoding. Repository plus initial task is rejected before retaining a key; repository UI remains ticket 04 and desktop remains ticket 16.

## Acceptance evidence

| Boundary | Result and observable behavior |
| --- | --- |
| U: actual TaskHome + REST web client + TS SDK | 18 task-home tests. Exact Chinese task, rapid repeated click, receipt/Agent identity distinction, refresh, unknown parent owner, already-prepared Agent, lost response, original-key recovery, separate next draft, waiting connection, terminal preparation failure, account/client/context/unmount late callbacks, sign-out/switch, storage outage, damaged stored body, blank/UTF-8 limit/permission and unsupported schema. Only external fetch/auth/storage boundaries are controlled. |
| U: related regression after integration | 23 files / 218 tests passed across creation, authentication, App/Console/commands/status, ticket 01 task navigation and Agent provisioning. The final double-click assertion also passed in the 18-test TaskHome run. Existing follow-up clears the retained creation after preparation succeeds in the verified Agent. |
| U: real Chrome | Fresh isolated browser, desktop 1440×1000 and mobile 390×844. Reviewed before/after captures; one visible focus ring, no horizontal overflow, 44px composer/template controls. Research fills editable text and focuses it without POST. Refresh and history back/forward preserve exact whitespace/Chinese draft with zero create POSTs. New agent focuses the Home textarea; rechecked after navigation merge. |
| A: real Mongo + public create/query/history | 9 named tests passed after integration. Actual `InlineAgentLifecycle`, Mongo operation/resource/profile stores and production Sandbox/Workspace/Agent workers; loopback external identity/key service and hosted runtime boundary fixtures. Default no-context create provisions a new Sandbox/workspace before one exact owner message. Contextual create uses original workspace authority. Assertions use public operation, Agent and transcript responses. |
| A: durable failure matrix | Lost create response + reopened ledger returns the same operation and Agent. Different whitespace under the same key returns 409. Other owner cannot read operation/Agent/history or reuse the workspace. A child service process exits after durable admission; a new service recovers the same operation and exact single task. Worker replacement before dispatch, after confirmed dispatch and after ambiguous runtime write never duplicates the transcript or runtime input. Ambiguous delivery remains `recovery_required`, without automatic resend. |
| A: compatibility and rejection | Blank, oversized UTF-8 and repository/task combination return 422 before reserving the key; the same key can then admit valid text. Legacy omission retains `purpose: setup`, new input uses `purpose: task`. 32 core Agent service tests passed after integration. |
| Public discovery / SDK | 8 actual OpenAPI/discovery/router tests passed, including optional string `initial_message` in full and focused contracts. TS lifecycle tests 2/2, typecheck, build and docs check (48 examples / 11 documents) passed. Rust SDK HTTP test 1/1 verifies exact text/key replay and old-field omission. |
| R: pinned real provider | `initial_task.test.cjs` passed 1/1 on macOS arm64 and 1/1 inside Linux aarch64. Actual Codex 0.147.0 + ACP 1.4.0 bridge SHA-256 `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c`; the test asserts version and bytes before running. Exact `  比较三个工具\n保留原文。  ` reaches the controlled external Responses model. The prompt reply is discarded, native processes stop, and public ACP `session/load` replays exactly one original user task and its result, with one model invocation. |

The first A red was the real public create route returning 422 for unknown `initial_message`; green admits, reopens and reads one exact task. The first SDK red was its public request rejecting the new field. Additional U red→green cycles cover storage failure, connection continuation, stale owner identity, terminal failure and corrupt stored text (previously a React object-child crash). Follow-up A matrix tests characterize the existing durable machinery; they do not introduce replacement persistence or synthetic success states.

## Reproduction and evidence paths

All commands run in `/Users/liuyitian/code/cma-ui-worktrees/ticket-03`. Rust uses pinned 1.96.1 with:

```sh
export PATH=/Users/liuyitian/.cargo/bin:$PATH
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR=/Users/liuyitian/code/cma-ui-worktrees/cargo-target-ticket03
export RUST_MIN_STACK=16777216
export CHRONO_TEST_MONGODB_URI='mongodb://127.0.0.1:27303/?replicaSet=ticket03'
cargo test --manifest-path api/Cargo.toml -p cma initial_task_public -- --ignored --nocapture
cargo test --manifest-path api/Cargo.toml -p layer1-public agent_manager::tests -- --nocapture
cargo test --manifest-path api/Cargo.toml -p layer2-core agent_service_tests
cargo test --manifest-path sdk/rust/Cargo.toml --test initial_task
```

The Mongo URI belongs to this ticket's disposable local replica set; recreating that set is required after cleanup. The private target was APFS-cloned from the dependency cache, then all API workspace packages and `cma-sdk` were cleaned before rebuilding. Only actual nonzero named test results are counted. Earlier shared-target/scratch runs are diagnostic red evidence, not final validation. An exploratory filename filter that selected zero OpenAPI tests was discarded; the correct module filter above ran eight named tests.

```sh
npm --prefix web run typecheck
npm --prefix web run lint
npm --prefix web test -- --run src/create src/auth src/App.test.tsx src/Console.test.tsx src/ConsoleCommands.test.tsx src/ConsoleStatus.test.tsx src/shell/TaskNavigation.test.tsx src/agent/provisioning.test.tsx
npm --prefix web run build
npm --prefix sdk/typescript run typecheck
npm --prefix sdk/typescript test -- --run tests/agent-lifecycle.test.ts
npm --prefix sdk/typescript run build
npm --prefix sdk/typescript run docs:check
CMA_CODEX_TEST_BINARY=/Users/liuyitian/.npm/_npx/abfeab83ffa1581c/node_modules/@openai/codex/bin/codex.js CMA_ACP_TEST_BRIDGE=/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/bridge.mjs node --test api/crates/layer3-adapter/src/codex/initial_task.test.cjs
docker --context colima-cma-ui-proof run --rm --name ticket03-initial-task-proof --mount type=bind,src=/Users/liuyitian/code/cma-ui-worktrees/ticket-03/api/crates/layer3-adapter/src/codex,dst=/candidate/codex,readonly -e CMA_CODEX_TEST_BINARY=/usr/local/bin/codex -e CMA_ACP_TEST_BRIDGE=/usr/local/lib/node_modules/@agentclientprotocol/codex-acp/dist/index.js cma-ui-runtime-proof:baseline node --test /candidate/codex/initial_task.test.cjs
```

Durable local evidence directory: `/Users/liuyitian/code/cma-ui-worktrees/evidence/ticket-03/`.

- `cma-ticket03-public-merged.log`, `cma-ticket03-openapi-final.log`, `cma-ticket03-core-merged.log`, `cma-ticket03-sdk-merged.log`: isolated Rust counts and names.
- `cma-ticket03-web-merged-tests.log`, `cma-ticket03-home-final.log`, `cma-ticket03-web-merged-build.log`: relevant UI and build results.
- `cma-ticket03-native-final.log`, `cma-ticket03-linux-final.log`: actual provider passes, neither skipped.
- `home-before-desktop.png`, `home-before-mobile.png`, `home-after-desktop.png`, `home-after-mobile.png`, `home-history-proof.json`: visual/focus/history evidence. Browser network fixtures are U evidence only.
- `changed-paths.txt`: only ticket 03 changes against integrated `d656e542e`, excluding upstream ticket 01/09 files.

## Limits and shared integration patches

- R uses the public ACP protocol and actual pinned provider processes with a controlled external model. Linux image ID is `sha256:22071f29fff5db494c8e5633a74692ebae442d7c2c7a0856e2d15ff7f9802057`. This is the prepared minimal proof image, not production packaging. No claim of live model answer quality, production authentication/deployment, or exhaustive process-kill coverage. This ticket changes no production Layer 4 binary; it does not claim that a baseline binary contains candidate runtime changes.
- U, A and R are separate approved proofs, not a claim that one browser test exercised a deployed CMA→Layer4→Codex stack. A proves the candidate public lifecycle, persistence, exact admission and public history; R proves native text/result/restart behavior. A uses a controlled hosted-runtime boundary, so its success alone is not provider evidence.
- Web typecheck, ESLint, production build and owned-file Prettier pass. Production build retains its existing >500kB chunk warning. All 15 owned Rust files pass rustfmt with their actual editions (API 2021, SDK 2024) and `skip_children=true`. Whole-repository formatting was not rewritten; preserved baseline failures remain in `/Users/liuyitian/code/cma-ui-worktrees/baseline-rust-format.log`. The unrelated aggregate skill-error contract mismatch documented by integration is not included in this ticket's pass claims.
- Shared root patches: `App.tsx` redirects ordinary global `new_agent` to Home and focuses `.task-home textarea`; contextual requests retain their route. `Console.tsx` renders TaskHome only under existing personal-scope/create guards. No router/navigation state model is added. `CreationFlow.tsx` uses TaskHome for ordinary contextual creation and preserves profile-use creation. Ticket 01 navigation source enters only via integration merge.
- Shared auth/recovery patches: auth retains/clears task viewer storage after authentication and on sign-out; lifecycle intent validates optional retained task text and checks storage isolation. Agent identity recovery accepts verified preparing or prepared Agents, requiring exact parent ownership. Existing Agent follow-up and transport handoff remain the recovery path.
- Unresolved acceptance items at the approved U/A/R seams: none. Broader deployment/runtime-image certification and the full deployed-stack journey remain outside these local proof claims. Repository option 04 and desktop 16 remain untouched. Original and integration worktrees were not modified; no deploy, push or remote PR.

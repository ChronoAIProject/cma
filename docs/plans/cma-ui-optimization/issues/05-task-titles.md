# 05：按真实任务自动命名并保留手动名称

**What to build:** 第一条真实任务开始后，列表、顶栏和刷新后的 Agent 使用可读任务名；用户改名始终优先。

**Blocked by:** None（可使用现有 Agent 消息入口独立验证）。

**Status:** done

**优先级:** P0

## Acceptance criteria

- [x] 保留既有优先级：owner 手动名称优先，其次有效 provider title/真实任务预览，再次安全回退；未执行任务的 setup preview 不成为任务标题。
- [x] 首条真实任务的标题及来源可通过已授权读取恢复，列表、当前页、浏览器标题一致；不靠当前页内存或扫描全部历史维持命名。
- [x] provider 名称缺失或生成失败时使用有限长度任务摘要；不额外阻塞首条任务，也不为等待命名再发一轮用户对话。
- [x] 手动改名与晚到 provider 更新并发时保留 owner 名称；刷新与切换 Agent 不闪回旧名称。
- [x] 自动准备、空文本、很长标题、未知来源和重新连接有稳定回退；正文与 composer 不因名称更新重挂载。
- [x] 已存在的来源/手动名称字段足够时直接复用；确需补齐持久化投影时兼容旧记录，不默认新增一套标题服务。

## Testing boundaries（2026-11-03 已确认）

- **U/A：** 从现有任务发送/改名入口观察列表和顶栏，刷新后公开读取同一名称；验证手动覆盖和晚到事件隔离。不能证明 provider 一定会生成好标题。
- **R：** 固定 provider 版本实际观测到名称或走真实 fallback，记录来源；命名质量是人工样本审查，不用实现同构的断言评分。
- **首个 red → green：** 用户改名为“工具选型”后到达旧 provider 名称，列表与重开页面仍显示“工具选型”。

## 范围与依赖说明

现有发消息链路足以触发命名，所以不依赖 03；也不依赖 01 的视觉重排。共享区域需协调合并。

## Implementation record

- 2026-11-03: isolated worktree `ticket-05`, branch `codex/cma-ui-05`, clean starting integration commit `d0d085c90` (01/09/03).
- Confirmed U/A/R seams remain as approved above. First slice exercises the public rename/read path and visible navigation/header/browser title after a late provider update; runtime evidence uses pinned Codex 0.147.0 and patched ACP 1.4.0, with bounded manual title review.
- First red → green: `ConsoleTaskTitle.test.tsx` renamed via the existing preferences dialog to “工具选型”, delivered the old provider name, then reopened the page. Navigation/header already preserved the owner override; the browser title failed with the fixed product name. The focused test now passes (1/1), including draft/message DOM identity and reopened public-client reads. The legacy `AgentTitleService::set_user_title` is genuinely write-once; the normal owner rename uses revisioned `conversation.preferences.title`, which is preserved.
- Completed 2026-11-03. Implementation commits: `bd9c047c5`, `68d286c55`, `85d430830`, `311249514`. Merged integration `7739c431f` in `3345f2b2b`, then latest integration `4e86b3b75` in `ce3d9dea4`; the latter changes only the integration progress document. Final focused verification ran after that merge.
- Task admission now persists the first real task preview and existing `prompt_preview` source in the same transaction, including paused queues and Home's explicit `initial_message.purpose = task`. Setup is excluded. Existing title generation also skips the recorded setup turn. Reconnect metadata cannot replace an established task preview with setup, empty, or unknown-source metadata; a valid provider title may upgrade it.
- Navigation, header, and active browser tab share the existing title selector. Owner preferences win every late provider update and repeated rename. Automatic labels are bounded; legacy internal IDs become “Untitled task”. Access loading/denial clears the prior task's browser title. Retained surfaces preserve composer, focus, draft, and message DOM.
- `docs/Web_Style.md` was updated before UI implementation. Existing ticket 01 `taskTitle.ts` and ticket 03 explicit task purpose are preserved. No new title service, storage field, source enum, endpoint, migration, SDK method, or extra model/user turn was introduced. The existing API behavior documentation was updated.

## Verification and evidence

Evidence root: `/Users/liuyitian/code/cma-ui-worktrees/evidence/ticket05/`. Test logs use `/Users/liuyitian/code/cma-ui-worktrees/ticket05-` as their prefix. These local artifacts are retained for the merger; they are not production telemetry.

| Boundary / check | Result | Evidence |
| --- | --- | --- |
| U: Console, navigation, document title, drafts, presentation | 8 files / **61 tests passed** after the final integration merge | `ticket05-final-web.log` |
| A: authenticated REST detail/list/events/preferences and real task admission | **4 named tests passed**, 412 filtered; repeated rename, source restoration, late provider, explicit initial purpose, queue, and cross-owner denial | `ticket05-final-public.log` |
| A: operation regression | **122 tests passed**, 1024 filtered; only the independently recorded baseline skill fixture excluded | `ticket05-merged-core.log` |
| A: existing title generation/value/store services | **22 tests passed**, 1124 filtered | `ticket05-merged-core-titles.log` |
| A: existing public conversation command contract | **16 tests passed**, 400 filtered | `ticket05-merged-public-commands.log` |
| A: actual Mongo replica-set persistence and reopened-store authorized reads | **2 named tests passed**, 1595 filtered; setup exclusion, fallback generation, real-task source, owner name after late provider, and cross-owner denial | `ticket05-merged-mongo.log` |
| U: real Chrome desktop 1440×1000 and mobile 390×844 | Both passed rename → late SSE → reload; same composer/messages, retained draft/focus, no horizontal overflow; mobile name target 44px | `browser-proof.json`, `browser-proof-merged.log`, `title-before-{desktop,mobile}.png`, `title-after-{desktop,mobile}.png`, `title-renamed-{desktop,mobile}.png` |
| R: actual pinned native provider, macOS and Linux | **3 samples on each platform**, each restored after process restart/session load; actual `prompt_preview`, no provider-generated title, exactly 3 model calls per platform | `native-fallback-macos.json`, `native-fallback-linux.json` |
| Static/build | Typecheck, lint, build, touched-file Prettier and Rust format checks, and `git diff --check` pass; build retains the existing large-chunk warning | `ticket05-final-{typecheck,lint,build}.log` |
| Secret scan | Gitleaks scans all 31 commits since the starting integration commit; no leaks found, no new exceptions | `gitleaks-final.log`, `gitleaks-final.json` |

Rust verification uses `PATH=/Users/liuyitian/.cargo/bin:$PATH`, `CARGO_BUILD_JOBS=2`, and private `CARGO_TARGET_DIR=/Users/liuyitian/code/cma-ui-worktrees/cargo-target-ticket05`; the test logs identify the ticket worktree and named nonzero test counts. Reproduction filters are `cargo test -p layer1-public task_titles`, `cargo test -p layer2-core sandbox::agent_operation_tests -- --skip installed_skill_version_change_is_reported_as_unsupported`, `cargo test -p layer2-core sandbox::title`, and `cargo test -p layer1-public conversation::commands_tests`. The Mongo run uses `CHRONO_TEST_MONGODB_URI` pointing to a disposable replica set and `cargo test -p cma task_title_tests -- --ignored`.

Additional public-behavior red → green slices covered missing persisted preview, explicit task versus setup, queued preview bounds, reconnect downgrade, setup exclusion in existing Mongo title generation, bounded visible labels, and access loading/denial clearing the tab. Three existing operation assertions were updated to allow the new metadata event while retaining their command-order and owner-message assertions.

### Pinned runtime and bounded quality inspection

- Actual Codex **0.147.0**, ACP **1.4.0**. Patched bridge SHA-256 on macOS and Linux: `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c` (`bridge-macos.sha256`, `linux-versions.txt`). Linux image ID: `sha256:22071f29fff5db494c8e5633a74692ebae442d7c2c7a0856e2d15ff7f9802057`.
- The real native process talks to a controlled local Responses model boundary that supplies an acknowledgement, without an injected native title. The provider actually emits and restores `prompt_preview`; these are true fallback observations, not a claim that a live production model generated names. The minimal Linux image proves this bounded provider boundary only and is not a production image.
- Manual review of three samples: “比较 Rust 和 Go 的部署工具” retains the comparison intent; “Investigate checkout failures and propose a fix” remains actionable; the long release-checklist prompt becomes the bounded application summary “Review the release checklist for the deployment”, which loses detail but still identifies the task. No semantic quality score or implementation-shaped quality assertion is used.
- Browser U evidence uses controlled public HTTP/SSE responses, A uses actual public services/REST and Mongo, and R runs the actual native provider separately. This establishes the approved seams; it does not claim a deployed browser-to-provider production run.

### Baseline limitations and cleanup

- Broad web run before the integration update: 321 files, **2862 passed / 10 failed** (`ticket05-web-tests.log`). The same ten failures were reproduced on a pristine `d0d085c90` archive: `AppCreation.test.tsx` (3), `AppPublic.test.tsx` (6), and `vocabulary/publicCopy.test.ts` (1), with 34 other tests passing (`baseline-web-failures.log`). They are unrelated and unchanged; the full suite is not reported green.
- Core `installed_skill_version_change_is_reported_as_unsupported` has the preexisting fixture `Conflict`, independently reproduced by the integration work on pristine `60fc98ee5` and recorded in `IMPLEMENTATION.md` / `runtime-proof/ticket11-evidence/ticket-11-core-baseline.log`. Preexisting whole-repo formatting failures, the public skill fixture mismatch, and the 508-line `agent_operation_service.rs` are untouched.
- Stopped task-owned Vite on port 5195 and removed task-owned `cma-ticket05-mongo`; all native proof containers/processes and temporary Chrome sessions exited. Verified no `cma-ticket05*` containers remain. Shared `cma-ui-proof` VM remains running. Worktree, private build output, and evidence are retained for parent cleanup. No deploy, push, or PR.

## Changed paths

Relative to merged integration `4e86b3b75` (27 files):

```text
api/crates/cma/src/store/agent_title_claims.rs
api/crates/cma/src/store/agent_titles.rs
api/crates/cma/src/store/sandbox_mongo_integration_tests.rs
api/crates/cma/src/store/sandbox_mongo_integration_tests/task_title_tests.rs
api/crates/layer1-public/src/rest/agents_tests.rs
api/crates/layer1-public/src/rest/agents_tests/task_titles.rs
api/crates/layer2-core/src/sandbox/agent_operation_dispatch.rs
api/crates/layer2-core/src/sandbox/agent_operation_tests.rs
api/crates/layer2-core/src/sandbox/agent_operation_tests/admission_fence_tests.rs
api/crates/layer2-core/src/sandbox/agent_operation_tests/first_message_tests.rs
api/crates/layer2-core/src/sandbox/conversation_observation.rs
api/crates/layer2-core/src/sandbox/conversation_observation_fold.rs
api/crates/layer2-core/src/sandbox/first_message_service.rs
api/crates/layer2-core/src/sandbox/title.rs
docs/API_REST.md
docs/Web_Style.md
docs/plans/cma-ui-optimization/issues/05-task-titles.md
web/src/Console.tsx
web/src/ConsoleConnectionLoading.test.tsx
web/src/ConsoleTaskTitle.test.tsx
web/src/ConsoleTitle.test.tsx
web/src/agent/AgentSurface.tsx
web/src/agent/presentation.test.ts
web/src/agent/presentation.ts
web/src/agent/runtimeMetadata.ts
web/src/resources/SharedAgent.tsx
web/src/ui/useDocumentTitle.tsx
```

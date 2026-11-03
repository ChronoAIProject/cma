# 04：选择仓库上下文后启动同一项任务

**What to build:** 用户可在首页输入框下选择仓库和 ref，必要时连接账号，随后 Agent 在明确的代码版本上执行原任务。

**Blocked by:** 03（首页任务原文、创建身份与恢复链路）。

**Status:** done

**优先级:** P0

## Acceptance criteria

- [x] 仓库是可选上下文，省略时继续创建空工作区；选择或清除仓库不清空任务正文。
- [x] 合法 GitHub 仓库/ref 与原任务一起冻结在创建操作中；展示实际解析的 commit，不把输入 ref 当已验证版本。
- [x] 缺少 GitHub 权限时沿已有服务连接流程处理；完成连接后继续原请求，不能新建第二个 Agent或在取消连接后悄悄执行。
- [x] 仓库不可访问、ref 不存在、准备失败和任务执行失败分别说明；保留原文与选项，明确何时恢复原请求、何时修改后发起新意图。
- [x] 自动仓库准备与真实任务有明确顺序，准备失败时用户任务不能在错误目录执行；已有上下文不静默被新仓库覆盖。
- [x] 重复点击、刷新及准备期间服务恢复仍对同一任务/同一实际代码版本负责；沿用已有来源检出与文件保护规则。
- [x] 公开契约接受的上下文组合与服务端一致；UI 不允许服务器不支持的 Workspace/Sandbox/repository 组合。

## Testing boundaries（2026-11-03 已确认）

- **U/A：** 从仓库选择到连接、创建、失败恢复及公开读取的实际版本；捕获串上下文和多次创建。不能证明外部 GitHub 当前可用性。
- **R：** 隔离仓库的指定 commit 能在 Agent 中实际读取并执行首条任务，已有文件保持完整；不操作用户生产仓库。
- **首个 red → green：** 指定测试 ref 解析为固定 commit，任务读取该版本的已知文件内容，授权回跳后仍只有同一个创建操作。

## 范围与依赖说明

不引入第二种创建状态机，也不实现新的 Git 凭据通道；桌面选项和自动命名可独立迭代。

## Execution

- Worktree `/Users/liuyitian/code/cma-ui-worktrees/ticket-04`, branch `codex/cma-ui-04`, baseline `9a900786a99ee2bb9a05f56f68f2a62d61abf189` (01/03/06/09 integrated).
- Use the approved U/A/R seams above, one behavior per red → green cycle. Preserve the existing lifecycle key, connection continuation, durable first task and immutable workspace receipts. Runtime proof uses an isolated repository and pinned Linux provider; any runtime source change requires the candidate binary.
- Existing prompt-only repository preparation is insufficient evidence of a pinned checkout. The implementation must finish and record source preparation before dispatching the exact task.

### Integration repairs requested by parent

- Updated `AppCreation.test.tsx` through Settings → Manage environments → Workspace New agent, retaining the exact Workspace assertion and adapting the already-integrated task composer text. Before: stale removed navigation and Home assertions fail. After: AppCreation + Console 12/12 pass (`ticket04-appcreation-before.log`, `ticket04-appcreation-after.log`, outside repo).
- Extracted Home/fallback selection into `ConsoleLanding.tsx` and creation/access policy into `ConsoleCreationAccess.ts`; final Console is 494 lines with all merged 05/14 hooks intact. OpenAPI document composition is also extracted, leaving its schema inventory at 498 lines.
- These are task-introduced integration repairs, distinct from the documented original formatting, aggregate skill fixture and 508-line Agent operation baseline defects.

### Completion evidence

- Full contracts, red/green records, source/image hashes, test commands and parent/19 findings: [04 evidence](../04-EVIDENCE.md).
- 88 Web tests, 7 real Chrome checks, 11 public/Mongo task contracts, 298 runtime unit tests plus one CLI integration, 33 TypeScript SDK tests and 2 Rust SDK tests passed. Two pre-existing external-fixture runtime tests remain ignored; the new pinned native proof passed 1/1 without skips.
- The final actual Linux candidate includes 14. Its real worktree retains the resolved source after ref movement and runtime restart; pinned Codex 0.147.0/ACP 1.4.0 reads the known file and actual HEAD and executes the first task. Ambiguous partial preparation fails closed without re-resolving the ref.
- Latest integration `a426b87ee5852866850cab192b37d14f9f2087c6` merged in `32bfb92c6428d1b12fce23e4d9a7a9a21437f92f`; no 05 ownership conflict, no desktop scope. Original LocalAgentV2 head-404 and inherited 14 OpenAPI inventory omission are separately documented for 19; neither is claimed green.

# 06：展示真实计划进度与当前活动

**What to build:** 用户能看到 Agent 已完成哪些步骤、当前在做什么，并按需展开原始操作；完成后过程可收起。

**Blocked by:** None（复用已有 PlanSet 和工具事件）。

**Status:** done — 本票 U/A/R 验收完成，等待集成 merger 接收

**优先级:** P1

## Acceptance criteria

- [x] 固定 runtime 的真实计划事件能够经公开流和历史重放呈现；检查已有 ACP/PlanSet 投影并只补缺失环节，不为显示卡片创建第二种计划源。
- [x] 已完成/当前/未开始各有明确文本与图标；已知计划为完成、执行中、待开始三步时显示“已完成 1/3、当前第 2 步”，进度条可被读屏理解。
- [x] 同一 turn 新计划替换旧计划，去重重连事件；重排、清空、未知状态和多个 in_progress 不虚构稳定步骤 ID 或完成比例。按设计取第一个当前步骤并允许展开全部。
- [x] 无计划显示真实当前活动/Thinking，不能从工具次数猜总步数；当前活动用已报告的安全描述，无明确关联时不称其属于某一步。
- [x] 工具调用默认折叠并显示操作数量，失败和需用户处理事项始终可发现；审批不藏在折叠组里。
- [x] 成功且计划全部完成时可显示“已完成全部 N 步”；成功但计划未全部报告完成、失败、取消各有准确摘要，不补写 completed。
- [x] 无可靠估计来源时隐藏预计剩余时间；已报告估计带来源/更新时间，过期即撤下，不把进度比当耗时预测。
- [x] 活跃计划、历史计划、顶栏摘要与完成折叠保持同一 turn scope；新轮不继承旧计划，用户阅读历史时更新不强行滚到底。

## Testing boundaries（2026-11-03 已确认）

- **U/A：** 公开事件/重放→会话页面的计划和状态，固定三步例子、重连、清空及失败；验证可见行为而非私有 presentPlan 函数。不能证明 provider 在所有任务都产出计划。
- **R：** 在固定版本获得至少一份实际计划与一次变更，并验证重连；provider 未发计划时验证降级，不把缺失事件伪装为支持证明。
- **首个 red → green：** 已完成一步、正在执行第二步的三步计划，断线重放后仍只有一张当前卡片，显示 1/3 和第 2 步。

## 范围与依赖说明

使用现有工具摘要，不等待浏览器或操作回放。18 交付后才增加真实回放链接，未接入前保留“查看计划和操作”，不放空入口。

## 实现与交接（2026-11-03）

分支 `codex/cma-ui-06`，工作区 `/Users/liuyitian/code/cma-ui-worktrees/ticket-06`。起点为 `444de4f5d`（含 09）；已合入最新集成 `c7452a7ed`（含 01、09）。仅在本工作区实现、提交和合入；原工作区及集成工作区未写入，没有部署或远端 PR。

- `7d580ac6f`：真实计划计数、当前步骤、状态图标、进度条、按 turn 的终态与折叠操作。
- `5397065d7`：固定 provider 的计划变更/进程恢复证明，以及真实 recorder → 公开 AG-UI cursor/replay 测试。
- `f295e7049`：Chrome 中消费上述公开流字节，验证重连、刷新、键盘、移动端和历史阅读位置。
- `ccc45fcc4`：计划与终态通过单一 atomic polite status region 播报。
- `f957f3b7a`、`48e16b292`：合入集成。唯一冲突为 Web Style 两票同时追加，保留双方内容，验证集成版文件仍为最终文件的完整不变前缀。

继续使用 `ConversationState.plans`、`setPlan`、现有 chronology/timeline 和 `runId`。没有新增计划协议、私有路由或第二份计划源。ACP → core `PlanSet` → AG-UI `/plan` / Responses v2 `plan_set` 已存在；补足的是 Web 的 reported-progress、按 turn 保存的终态投影以及 transcript hydration 合并。审批中断保持当前 turn，旧 cursor 的状态重放不能复活已清空计划。异常条目不再让剩余的 completed 条目冒充完整计划；未知状态和多个当前步骤保留可读事实并隐藏确定进度条。

终态来自 durable turn、公开 RUN_FINISHED/RUN_ERROR 或 transcript outcome；没有从最终文本或 Agent ready 状态猜成功，没有改动 09 的 `finalAnswer` eligibility。失败、取消、未知结果和成功但计划未全部报告完成分别呈现；只有成功且所有有效条目 completed 才显示 Completed all N steps。当前协议没有可靠 ETA 字段，因此始终不显示预计剩余时间；没有新增估算或过期缓存。

02 交接：`AgentHeader.progress?: ReactNode` 是 identity 与 actions 之间的紧凑插槽，`OwnerAgentHeader` 原样透传；06 通过 memoized `PlanProgress` 放入当前 turn 的链接。02 可在该插槽组合 elapsed timer，保留 06 的当前计划入口，不重复显示活动摘要。06 未改 `turnTiming` 或设置流程。`AgentSurface.tsx` 当前 496 行，后续扩展需继续遵守 500 行源文件限制。

07/09/11/14 交接：`StreamEntry` 仅增加 planOutcome/active 参数及对应 memo 判断；消息、最终回答、评价、文件和建议的归属逻辑未改。单独工具也使用现有 disclosure，默认折叠，失败保持可发现；相邻工具按 turn 分组，审批/恢复卡始终在组外。18 之前没有 Replay 空入口。

## 验收证据

| 边界 | 结果与范围 |
| --- | --- |
| U 首个 red → green | 三步计划通过 live `/plan` + durable replay 只渲染一张卡。新增用例先因缺少 Completed 1/3 steps 失败，再通过；读取当前第 2 步和 progressbar 的 now=1、max=3。 |
| U 状态与恢复 | `PlanProgress.test.tsx` 13 个行为用例覆盖重排、清空、未知/多个当前、无计划 Thinking、部分完成成功、失败、取消、审批、完整成功与 transcript 恢复、旧 cursor、操作 turn scope、损坏计划不得变成完成。逐个行为 red → green；未为新 helper 写内部实现测试。 |
| U/A 回归与性能 | 合入 01/09 后 10 个 focused 文件 **75/75** 通过，含原有 Markdown 长历史、conversation 与 timeline 性能用例；最后 accessibility 改动的定向回归 **22/22** 通过。计划 header 只依赖 plans 引用和 activeRunId，历史 PlanCard 沿用 memo；没有增加 token 热路径的历史扫描、排序或历史 Markdown 渲染。 |
| A 公开读写边界 | `agui::` **23/23** 通过，包含新 `recorded_plan_changes_survive_public_stream_reconnect_without_synthetic_completion`，实际执行 1 个新用例。固定 Linux provider 的原始 ACP plan 通知送入真实 DurableAgentUpdateRecorder；通过已授权公开 GET 观察第一次计划、cursor=5 之后的变更及完整 replay。终止成功后仍只有两份 plan 更新，最后一步仍 pending。测试持久化为内存 store，断言只经公开读取，不查数据库证明结果。 |
| R 原生 provider | macOS 与隔离 Linux 各 **1/1** 通过本票新增 native test。模型网络边界受控，实际 Codex app-server 执行 native update_plan，实际固定 ACP bridge 发出 0/3→1/3 两个报告；不是手写 provider 通知。进程重启后 session/load 恢复原 conversation 的消息，实际未重发 plan（reloadPlanCount=0），随后无计划 turn 也未伪造计划；计划恢复依赖 CMA durable log，公开 replay 已验证。 |
| Chrome | 最终集成代码在独立 Chrome context，1440×1000、390×844 **2/2** 通过。直接消费上述 Rust 公开路由导出的 SSE 字节，断线按真实 cursor 续传、刷新只有一张卡；1/3 与第 2 步一致，结束后不冒充完成，键盘可定位/展开，移动目标至少 44px，无横向溢出，重连保留草稿。长文本仅作为浏览器滚动压力 fixture；真实 plan 变更不把阅读历史的用户拉到底。 |
| 工程检查 | Web typecheck、ESLint、E2E typecheck、production build、changed-file Prettier、CSS collision、changed Rust rustfmt 和 `git diff --check` 通过。Gitleaks 扫描本票 4 个实现提交（51.58 KB）无泄漏。构建保留既有大 chunk 提示；未声称全仓所有测试通过。 |

所有生成证据保存在仓库外：`/Users/liuyitian/code/cma-ui-worktrees/ticket06-evidence/`。主要文件：`linux-native-plan.json`、`linux-native-plan.log`、`public-plan.json`、`public-plan-test-final.log`、`web-focused-final.log`、`status-accessibility.log`、`browser-final.log`、`web-build-final.log`、`runtime-image-id.txt`、`runtime-sha256.txt`、`gitleaks.log`。Chrome 改前截图为 `browser-before/initial-{1440,390}.png`，来自 `444de4f5d` 的独立只读基线导出；改后为 `browser-after/{initial,active,replayed}-{1440,390}.png`，已目视检查桌面和移动端。

Rust 使用 `PATH=/Users/liuyitian/.cargo/bin:$PATH`、`CARGO_BUILD_JOBS=2`、共享 target 加 `RUSTFLAGS='--cfg cma_ui_ticket06'`；以日志的实际 **23 tests / 23 passed** 验证，不接受 0-test 空通过。原有全局 Rust format、skill-error corpus 及不相关源文件行数基线问题未改。

## R 版本、复现与证明限制

Linux image ID：`sha256:22071f29fff5db494c8e5633a74692ebae442d7c2c7a0856e2d15ff7f9802057`，使用 `docker --context colima-cma-ui-proof`。Codex **0.147.0** / ACP **1.4.0**，实测 launcher SHA-256 `134063e133f0b4244fa3b251acf973d4fe4b4aeeacbdc135211bf480f59f1477`，Linux arm64 native `e23d0be344d2496986c985cd3db61e6f649b1ddd900e6afc1b5aaabbffcbb4e2`，patched bridge `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c`，均匹配当前 Dockerfile pin。Linux 自建最小测试镜像包含基线 runtime；本票没有修改 Layer 4 或 provider 字节，因而无需构建候选 runtime binary。

复现链路：对 `plan_progress.test.cjs` 设置 `CMA_CODEX_TEST_BINARY`、`CMA_ACP_TEST_BRIDGE` 和 `CMA_PLAN_PROOF_OUTPUT` 运行原生测试；将输出作为 `CMA_PLAN_NATIVE_PROOF`，设置 `CMA_PLAN_PUBLIC_PROOF`，运行 `cargo test --locked -p layer1-public agui::`；启动 loopback Vite，再使用同一 `CMA_PLAN_PUBLIC_PROOF` 与 Chrome executable 执行 `playwright test --config e2e/plan-progress.config.ts`。测试在 bridge SHA 或 provider 版本不符时直接失败，不静默跳过。

这个证据证明固定原生 provider/bridge 的真实计划行为、core 持久化与公开流/重放、SDK/会话页面的消费及 UI 降级。模型输出是隔离 HTTP fixture；不证明真实模型在每次任务都会生成计划。Linux image 是最小 test image，缺少生产 skills/software/security packaging；这不是完整生产镜像验收、部署的 Sandbox 或生产身份认证链路证明。没有凭该证据声明浏览器操控或其他票的 runtime 能力；后续部署/整体验收仍由 19 负责。本票验收项无剩余。

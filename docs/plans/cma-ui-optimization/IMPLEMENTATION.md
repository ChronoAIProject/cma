# CMA 页面优化实施记录

- 授权日期：2026-11-03，Asia/Singapore。
- 本地 tracker：本目录 issues，每票正文记录状态、提交及验证证据。
- 集成分支：`codex/cma-ui-optimization`。
- 集成工作区：`/Users/liuyitian/code/cma-ui-integration`。
- 原工作区：`/Users/liuyitian/code/cma`，保持原分支及未提交文件。
- 原始 HEAD：`677c27440aa96e428ee50f9801ea886136ea04c8`。
- 现有工作快照/实现比较基线：`c9e2d7b91a0c49c68d3cd2968e61dd8568ed3706`。
- 该基线包含原有后端、会话性能及设计文档；这些已有改动不是本轮新实现。

## 执行约定

每票从集成分支创建独立分支和 worktree，按已确认的 U/A/R 测试边界执行 tdd。提交前合入最新集成分支，通过对应行为测试并记录证据；由独立 merger 合并。所有新设计遵循 Web Style，追加其 changelog。运行时能力仅在固定版本的真实环境验证后声明可用；关闭开关或模拟画面不能替代交付。

完成标准为所有票验收通过，集成验证和 standards/spec 两轴 review 无未处理问题。不能验证的项明确保持未完成。该任务不包含部署、合并到 develop/main 或发布远端 issues。

## 进展

- 已确认 19 张票及 22 条依赖，无循环；初始 frontier：01、02、03、05、06、07、09、11、14。
- 已隔离并保存原工作区内容；原文件哈希清单保存在工作区外，交付时复核。
- 首批任务 01、03、07、09、11、14 已在独立 worktree 启动；共享回答区域的改动合并时逐项检查。
- 01、02、03、05、06、09、11、14 已验收合入，八个已完成 worker 的 worktree 已清理。当前实施：04、07、10、12、13、15；12 与 13 复用建议契约分别实现编辑与生成，15 从已验证的电脑面板继续打通真实浏览器，18 等待执行名额。
- 11 的广泛验证暴露两处本轮集成遗漏：01 移动导航后 `AppCreation.test.tsx` 仍查找旧 Sandboxes 按钮，03 使 `Console.tsx` 增至 503 行。已交由 04 在首页上下文工作中修复并回归；这两项不是原始基线问题，不以单票通过掩盖它们。
- 02 的隔离复跑确认的 7 个折叠操作断言已由 14 修复，合并态 54 个相关 Web 测试无排除通过；03 导致的旧首页/创建浏览器断言及 Console 的集成行数超限继续由 04 处理。14 已按既有资源归属契约区分共享 Workspace/worktree 与 Agent 私有目录、上传空间和 runtime 状态，并验证公开读写限制；不将这解释为 OS 进程隔离或新增用户澄清。
- 在 `826de20e9` 对全部 19 张票执行验收条款文本审计：仅规范化 checkbox，所有原始条款与快照基线逐字相同，没有以改写条款代替功能交付。

## 基线验证

- Web 类型检查与 ESLint 通过。worktree 使用原项目已有依赖，SDK 依赖也已接入，未变更锁文件。
- 真实 Chrome 已保存首页、会话页在 1440×1000 和 390×844 下的截图，位于工作区外的 evidence 目录；仅证明本地 fixture 的呈现，不证明后端和 runtime 能力。
- 原工作区 46 个已记录文件的 SHA-256 复核一致。
- Rust 工具链为仓库固定的 1.96.1。基线 `cargo fmt --all --check` 已失败，涉及原有 management_responses、bindings、instance_problem、binding_cases、resource_access/binding、live_skill_tests 和 binding/model 的格式；原始诊断保存在工作区外的 baseline-rust-format.log。新任务只格式化自己修改的文件，不混入无关修正。
- 基线 `cargo test --locked -p layer2-core --no-run` 编译通过；已有 `ToolCallContent` 未使用导入警告另行记录，未把编译通过当成测试运行通过。
- 固定 Codex 0.147.0 与按 Dockerfile 顺序修补的 ACP 1.4.0 已验证源码摘要一致；原生观测测试 2/2 通过，覆盖进程恢复、名称来源和用量。模型边界受控，平台为 macOS，此证据不替代 Linux runtime 或真实模型质量。
- 已建立专用 Linux 验证 VM `cma-ui-proof`（2 CPU、4 GiB），使用显式 Docker context，不改变默认上下文、不接生产 Sandbox。后续真实浏览器与 runtime 验证使用该隔离环境。
- 基线公开接口编译已复现 `E0308`：`instance_problem.rs` 的现有 Skill 版本错误分支返回 `String`，其余分支返回 `&str`。这是新功能合入前的错误；最小类型修正与功能提交分开记录，原工作区不改动。
- 该前置编译修正仅显式统一 tuple 的 detail 类型并借用已生成的错误文本，错误内容和状态码不变；修正后 `cargo check --locked -p layer1-public` 通过。
- 从固定基线归档构建的 Linux aarch64 `layer4-runtime` 已成功，使用 Dockerfile 固定的 Rust builder 摘要。该二进制和测试镜像仅作隔离验证，不代表新浏览器能力已经实现。
- Linux 隔离镜像中的原生观测/工具测试 3/3 通过，覆盖原生工具调用、密钥轮换、会话重载、名称来源和用量。该镜像包含 Node 24.19.0、Codex 0.147.0、固定 ACP 修补字节及 Chromium 156.0.8078.79；未修改生产镜像或部署。
- 已在不含任何 ticket 功能的独立基线 `60fc98ee5`、私有 Cargo target 中复现技能契约失败：`agent-skills-install-version-conflict` 期望 `agent_operation_in_flight`，实际为 `agent_skill_version_upgrade_unsupported`。真实执行 1 个 corpus 测试并失败，400 个被筛除；日志为工作区外 `baseline-skill-contract.log`。该既有期望不在页面票内修改，也不把完整 corpus 宣称通过。
- 09 合入前后的新增提交已通过带脱敏输出的 Gitleaks 检查。基线 fixture Vite 服务已停止，截图与测试记录保留在工作区外。
- 01 的新增提交同样通过 Gitleaks 脱敏检查（6 commits，73.61 KB，无泄漏）。集成态源码行数检查发现 `agent_operation_service.rs` 为 508 行，超过 500 行限制；该文件与基线 `c9e2d7b91` 的 SHA-256 完全相同，属于原有工作，未混入无关拆分。
- 隔离镜像中的 Chromium 156.0.8078.79 已通过私有 DevTools pipe 导航两个本地页面并产生不同截图，记录位于工作区外 `runtime-proof/browser-prerequisite.json`。这是浏览器基础条件验证，尚不证明 CMA 授权、Agent 工具接入、实时画面或接管能力；仅测试容器使用禁用 Chromium sandbox 的参数，不能作为生产安全配置依据。
- 11 使用独立 `60fc98ee5` 基线归档及清理过的私有 target，另复现 Core `installed_skill_version_change_is_reported_as_unsupported` 在 fixture 创建时返回 Conflict；真实执行 1 个失败用例、1140 个被筛除，日志为 `runtime-proof/ticket11-evidence/ticket-11-core-baseline.log`。未修改这一原有技能测试。
- 03、06 新增的 15 个提交通过 Gitleaks 脱敏扫描（178.53 KB，无泄漏）。01、09 的临时截图和证据已另存到工作区外 `evidence/ticket-01`、`evidence/ticket-09`，供整体验收使用。
- 11、02 新增的 18 个提交通过 Gitleaks 脱敏扫描（193.96 KB，无泄漏）；02 的验证日志和截图另存到工作区外 `evidence/ticket-02`，未复制其临时依赖缓存或基线源码副本。

## Ticket 09 集成（2026-11-03）

- 合并前 `codex/cma-ui-optimization` 工作区干净，HEAD 为 `60fc98ee549c88df8d91d84bee7c0b388f525db3`；该提交已包含于票分支。
- 使用普通 `--no-ff` merge 合入 `codex/cma-ui-09` 的 `a34efa525bf83cc0b0c26afafb1a2a93bfb8eb86`，merge commit 为 `fbec0a8b96c01d2cdfd0196b2a6f180406ca8735`。无冲突、无集成代码修正；合并结果与票分支树完全一致，28 文件 manifest 保留。
- 基线内容及 `docs/Web_Style.md` 的既有条目保留，票 09 仅追加设计说明与 changelog。最终回答身份及 07/10/11/14 共享文件交接以 `issues/09-answer-feedback.md` 为准。
- 合并态检查（集成工作区执行）：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/AnswerFeedback.test.tsx` 通过，1 文件 / 14 tests；使用 runner 与禁用缓存避免写入指向原工作区的依赖目录。
- `npm --prefix web run typecheck` 通过；`git diff --check 60fc98ee549c88df8d91d84bee7c0b388f525db3 HEAD` 与记录提交前的 `git diff --check` 通过。未重复 worker 已完成的广泛 Web、SDK、反馈公共接口、core、Chrome、lint/build 检查。
- 已知问题保持原状：parent 已确认基线 public skill-error fixture 在用户 binding 修改后不匹配，`agent-skills-install-version-conflict` 预期 `agent_operation_in_flight`、实际 `agent_skill_version_upgrade_unsupported`；merger 未重跑或修正该无关契约。其他既有格式与 build 提示仍见基线及票文档。
- 本次仅写入集成 worktree；无原工作区或其他 worker worktree 编辑，无生产操作、远端 PR 或推送。

## Ticket 01 集成（2026-11-03）

- 合并前 `codex/cma-ui-optimization` 工作区干净，HEAD 为 `09b8174d370d41887ccbc6be9c3640d6e99a50f4`；该提交已包含于票分支。票文档六项验收完成，53 文件 diff 与 `/tmp/cma-ticket-01-evidence/changed-paths.txt` 完全匹配。
- 使用普通 `--no-ff` merge 合入 `codex/cma-ui-01` 的 `94f1c8fc1efa6d9425d0f613ffe29b2d1aa85156`，merge commit 为 `a0b2cad278e14d5f7faf5c447db07841f7c5ad0b`。无冲突、无集成代码修正；合并树与票分支完全一致，保留既有基线及 ticket 09。
- 已验证 `docs/Web_Style.md` 完整保留合并前内容作为原样前缀，票 01 的 42 行仅追加；既有 changelog 条目无编辑或删除。
- 合并态检查（集成工作区执行）：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/shell/TaskNavigation.test.tsx src/resources/TaskNavigation.test.tsx` 通过，2 文件 / 10 tests。两个准确路径来自票 manifest；runner 与禁用缓存避免写入指向原工作区的依赖目录。
- `npm --prefix web run typecheck` 通过；`git diff --check 09b8174d370d41887ccbc6be9c3640d6e99a50f4 HEAD` 与记录提交前的 `git diff --check` 通过。未重复 worker 的广泛 Web、SDK、公共 API、真实 handler、Clippy 或桌面/移动浏览器验证；本次没有 Rust 运行。
- worker 的 Rust 证据使用私有 target、`--cfg cma_ui_ticket01` 和实际非零测试数。已确认无关的基线 skill-error fixture mismatch 保持原样：parent 在 `60fc98ee5` 使用私有 target 独立复现，见 `baseline-skill-contract.log`；不宣称完整 aggregate corpus 通过。
- 01 已验收合入，可供后续票从本集成分支启动。本次仅写入集成 worktree；无其他修复、原工作区或其他 worker 源码编辑，无生产操作、远端 PR 或推送。

## Ticket 03 集成（2026-11-03）

- 合并前 `codex/cma-ui-optimization` 工作区干净，HEAD 为 `c7452a7edaf43f2c4fc94bfae43447cb82ce8db7`；票分支通过 `056263735` 包含该集成状态。37 文件 diff 与 `evidence/ticket-03/changed-paths.txt` 完全匹配。
- 使用普通 `--no-ff` merge 合入 `codex/cma-ui-03` 的 `af254b8365db7eec58c709e4d454f47f31147b0e`，merge commit 为 `d0d085c90dbf0a3b2bf34b2d02434211dc660d7a`。无冲突、无集成代码修正；合并树与票分支完全一致，保留基线及 01、09。
- 已检查票文档列明的 App/Console/CreationFlow、认证及生命周期恢复共享补丁；全局 New agent 进入 Home，上下文创建保留原权限及路由，准备中的 Agent 通过确切 owner 校验进入会话。`docs/Web_Style.md` 合并前内容完整保留为原样前缀，票 03 仅追加 30 行及其 changelog。
- 合并态检查（集成工作区执行）：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/create/TaskHome.test.tsx src/create/AgentLifecycle src/create/lifecycle` 通过，8 文件 / 95 tests；`npm --prefix web run typecheck` 通过。
- `git diff --check c7452a7edaf43f2c4fc94bfae43447cb82ce8db7 HEAD` 与记录提交前的 `git diff --check` 通过。未重复 worker 的广泛 U/A/R、Mongo 或固定 native provider 验证；本次没有 Rust 运行，原有基线限制及完整部署链路的证据边界保持票文档所述。
- 本次仅写入集成 worktree；无其他修复、原工作区或其他 worker 源码编辑，无推送、部署或生产操作。

## Ticket 06 集成（2026-11-03）

- 先完成 03 的独立 merge 及记录提交，再从干净的 `3ceaac7e9558a8cd171ad931453a98764036f736` 合入 `codex/cma-ui-06` 的 `164835afc809b7a3af7eca7364772fd19700fb44`。共同基线为 `c7452a7ed`；28 文件 diff 与 `ticket06-evidence/changed-paths.txt` 去除绝对 worktree 前缀后的 manifest 完全匹配。
- 普通 `--no-ff` merge commit 为 `c484d209b858cd8920f8d0000097659d89ae23e6`。唯一实际冲突是 `docs/Web_Style.md` 的追加区域；保留含 01、09、03 的全部原文字节，再原样追加 06 的 49 行及两条 changelog，已断言结果等于合并前全文加票 06 的完整增量。无源代码冲突或额外修正。
- 已检查 AgentSurface/Header 的 `progress?: ReactNode` 交接及 conversation/transcript/StreamEntry 共享补丁；09 的 `finalAnswer` 标记和评价 eligibility 保留，03 的全部非样式路径与合并前完全一致。02 可继续复用 progress 插槽，避免重复活动摘要。
- 合并态检查（集成工作区执行）：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/PlanProgress.test.tsx` 通过，1 文件 / 13 tests；`npm --prefix web run typecheck` 通过。
- `git diff --check 3ceaac7e9558a8cd171ad931453a98764036f736 HEAD`、冲突解决后的 `git diff --cached --check` 及记录提交前的 `git diff --check` 通过。仅样式文档冲突，不扩大检查；未重复 worker 的广泛 U/A/R、Chrome 或 native provider 验证，本次没有 Rust 运行。
- worker 的 Rust 证据使用独立 `--cfg cma_ui_ticket06` 和实际 23/23 测试数；原有格式、skill-error corpus、源文件行数及生产完整链路证据限制保持原状。本次仅写入集成 worktree，无原工作区或其他 worker 源码编辑，无推送、部署或生产操作。

## Ticket 11 集成（2026-11-03）

- 合并前 `codex/cma-ui-optimization` 工作区干净，HEAD 为 `9a900786a99ee2bb9a05f56f68f2a62d61abf189`；票分支通过 `38418c436` 包含该集成状态。61 文件 diff 与 `runtime-proof/ticket11-evidence/changed-paths.txt` 去除绝对 worktree 前缀后的 manifest 完全匹配；已读取票文档、`11-EVIDENCE.md` 与 `SUGGESTION_CONTRACT.md`。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-11` 的 `43e8a7a0ab9db5dce2cc3c2275862ffcf9b778ea`，merge commit 为 `7c59f957d4891b7322f6e364cdfbc994dcdf7637`；无 merge 冲突，初始合并树与票分支完全一致。
- 集成验证发现票 11 的 Web Style 块位于既有 09/01/03/06 条目之前，不满足本集成的严格追加顺序。独立文档修正提交 `56da4b424b0ce8f46ee04e146f316e39ecba6e14` 将该完整块原样移至末尾；已断言最终文件等于合并前全文加原样票 11 块和分隔空行，既有内容未修改或删除。除该顺序修正外，全部候选源码与票分支一致。
- 09 的 canonical `finalAnswer`、06 的 PlanProgress/turnOutcomes、03 首页流程及 01 导航保留；`latestTurnId` 仅用于防止旧回答恢复建议资格。共享发送与保留输入的提取保持原样，AgentSurface 为 482 行、controller 为 496 行；12/13 继续使用交接契约。
- 合并态检查（集成工作区执行）：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/AgentSurface.suggestions.test.tsx` 通过，1 文件 / 15 tests；`npm --prefix web run typecheck` 通过。`git diff --check 9a900786a99ee2bb9a05f56f68f2a62d61abf189 HEAD` 与记录提交前的 `git diff --check` 通过。仅修正文档顺序，不重复广泛/native 检查，本次没有 Rust 运行。
- 准确区分待处理问题：`AppCreation.test.tsx:86` 仍点击已移除的 Sandboxes，属于 01 引入的集成测试回归；`Console.tsx` 503 行属于 03 引入的集成行数超限。两项均不是原始基线问题，parent 已交由 04 修正，本 merger 未修改或重跑它们。原始 skill-error public contract、Core skill fixture 及 508 行 Agent service 是另行已有证据的前置问题，保持独立记录。
- 本次仅写入集成 worktree；无原工作区或其他 worker 源码编辑，无推送、部署或生产操作。

## Ticket 02 worker 交付（2026-11-03）

- 独立 worktree `/Users/liuyitian/code/cma-ui-worktrees/ticket-02`、分支 `codex/cma-ui-02`，从 `d656e542e` 开始，合入最新集成 `111530241`，功能合并提交 `a19e88697`。只实现 02；合入集成仍由独立 merger 执行。
- Web Style 先于 UI 修改，保留集成全文及 changelog 原样前缀。模型/推理/权限默认一行摘要，展开保留草稿/附件与原设置恢复流程；Queue/Interrupt 文案对应实际能力。完成耗时仅来自最新轮次记录时间及成功终态。
- 与 06 共用 `AgentHeader.progress`，`AgentHeaderProgress` 保留 memoized PlanProgress 链接；与 11 共用 `latestTurnId`，保留 01 领域状态及 09 最终回答身份。AgentSurface 480 行，未复制活动摘要。
- [02 验收矩阵、完整 28 路径清单与复现命令](02-EVIDENCE.md)。最终聚焦 Web 114/114；唯一 `--cfg cma_ui_ticket02` 下公共 handler 实际 58/58；固定 native Mac 5/5、Linux 1/1；双传输 Chrome 4/4、06 计划 Chrome 2/2、现有设置/审批/恢复 Chrome 4/4。类型、lint、CSS、build、改动文件格式及 secret scan 通过。
- 首条指定行为已证明 red → green；本地真实 native 终态 → recorder → AG-UI/Responses 公共输出 → 浏览器证据在 `/tmp/cma-ticket-02-evidence/`。模型/存储/时钟 fixture 边界明确；4 分钟是记录时间示例，不是性能或已部署整链声明。
- 广泛 Web 926 通过、7 项按精确名称排除；这 7 项在没有 02 的 `9a900786a` 快照独立复现，旧浏览器 fixture 问题也有独立基线记录。没有提交 skip 或弱化 gate；原始 skill contract/格式缺陷未改。无原工作区/其他 worker 源码修改，无 PR、推送、部署或 worktree 清理。

## Ticket 02 集成（2026-11-03）

- 合并前 `codex/cma-ui-optimization` 工作区干净，HEAD 为 `111530241e88806140685382b53de2134b8a7b79`；该状态已包含于票分支。已读取票文档和 `02-EVIDENCE.md`，其中 28 路径 manifest 与实际 diff 完全匹配。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-02` 的 `d3da33071b260d73549a6ae3cdb288bc7b622b0c`，merge commit 为 `b016065a8cd68922b79a22599e6bc98fd172fdac`。无冲突、无集成修正；合并树与票分支完全一致。
- 已断言 Web Style 保留集成全文为原样前缀，仅追加 02 的 48 行。01/03/06/09/11 保留；`AgentHeaderProgress` 同时呈现原 PlanProgress 与确认完成耗时，继续共用 `latestTurnId`，不重复活动摘要。正常 transport、发送提取及草稿/附件所有权保留。
- 合并态检查（集成工作区执行）：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/ComposerSettings.surface.test.tsx src/agent/AgentCompletion.test.tsx src/agent/SessionSettings.test.tsx` 通过，3 文件 / 36 tests，无跳过；`npm --prefix web run typecheck` 通过。
- `git diff --check 111530241e88806140685382b53de2134b8a7b79 HEAD` 与记录提交前的 `git diff --check` 通过。未重复 worker 的广泛/native/浏览器验证，本次没有 Rust 运行。
- 02 发现的 7 个测试失败属于本轮集成回归：ActiveThinking 4 项、MessageIdentity 1 项、SharedAgent 2 项；parent 已交由 14 按 06 的折叠展示契约处理。旧创建/登录后首页浏览器断言由 04 作为 03 集成后续处理；这些均不是原始基线问题，本 merger 不跳过或修正它们，也不宣称全仓绿色。14 仍在独立实施，未合入其未完成文件；原始 skill contract/Core fixture、格式和 508 行 Agent service 问题独立保留。
- 本次仅写入集成 worktree；无原工作区或其他 worker 源码编辑，无推送、部署或生产操作。

## Ticket 05 集成（2026-11-03）

- 合并前 `codex/cma-ui-optimization` 工作区干净，HEAD 为 `4e86b3b75b55525736a4f63e147de7e7485f36f3`；该状态已包含于票分支。已读取 `issues/05-task-titles.md`，其中 27 路径 manifest 与实际 diff 完全匹配。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-05` 的 `12d66f92f75245b0b11d07447486463f0f3ae4e5`，merge commit 为 `7841d06c465a8a90c624e10ac071e8a655e27d33`。无 merge 冲突，初始合并树与票分支完全一致。
- 严格追加检查发现票 05 的 Web Style 块位于 03 与 06 的既有条目之间；独立文档修正提交 `e39903405f18302c354f96822c393e488a1308da` 将完整文字及 changelog 原样移至末尾。已断言既有全文为最终文件的完整原样前缀，仅追加票 05 的 18 行；其他候选源码与票分支完全一致。
- 01/02/03/06/09/11 保留；owner 手动名称优先、有效 provider 名称及真实任务 preview 回退、setup 排除与 browser label 共用既有选择器。未增加重复命名服务、端点、存储字段或额外用户/model turn；正常 composer、消息及已合入的共享 header/建议路径保留。
- 合并态检查（集成工作区执行）：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/ConsoleTaskTitle.test.tsx src/ConsoleTitle.test.tsx src/agent/presentation.test.ts` 通过，3 文件 / 24 tests；`npm --prefix web run typecheck` 通过。`git diff --check 4e86b3b75b55525736a4f63e147de7e7485f36f3 HEAD` 与记录提交前的 `git diff --check` 通过。未重复广泛/native/Rust 检查。
- 待修复的首页/创建及折叠操作测试仍属本轮集成回归，由 04/14 处理，不误记为原始基线。Console 原有 503 行集成超限在本票标题路由补充后为 506 行，继续交由 04 修复；原始 skill contract/Core fixture、格式及 508 行 Agent service 问题独立保留。未合入 14，等待 parent 明确最终 SHA。
- 本次仅写入集成 worktree；无原工作区或其他 worker 源码编辑，无推送、部署或生产操作。

## Ticket 14 集成（2026-11-03）

- 合并前 `codex/cma-ui-optimization` 工作区干净，HEAD 为 `ef5e20c1f51b57833f115a61da55e57a70a4fe2a`；票分支通过 `4f530f283` 包含该状态。已读取票文档和 `14-FILE-SCOPE.md`，94 路径 diff 与 `ticket14-proof/changed-paths.txt` 完全匹配；另逐字断言原始八条验收标准未改变，仅勾选状态不同。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-14` 的 `a8c4283b4fd3dd495526b45904dd57e85dcb6db8`，merge commit 为 `e20ad60052e1804b1211114ec52a08292402a039`。无 merge 冲突，初始合并树与票分支完全一致。
- 严格追加检查发现 14 的四段 Web Style 增量插入了既有正文和 changelog 中间；独立文档修正 `f4e71dd07adfc2a5372a0bfbf0b7ee449fa27a7b` 将四段完整文字与条目按原顺序移至末尾。已断言最终文件保留全部既有字节为原样前缀；仅增加段间分隔空行，其他候选源码与票分支完全一致。
- 文件边界依据现有资源所有权契约：保留授权共享 Workspace/worktree 的读取、下载和编辑；排除实际 `.cma` 私有 Agent home/控制树、其他 Agent 上传命名空间及配置到 mounted home 内的 runtime/checkpoint 状态。内部 managed-instruction API 保持独立兼容；这是源码契约解释，不是假托新用户澄清或批准，也不声明 OS 进程隔离。
- 保留 01/02/03/05/06/09/11，新增 output metadata 与既有 `finalAnswer`、计划、耗时、Home、真实任务名称并存。14 已完成，公开环境观察的 browser/replay/control 仍明确 unsupported；15/18 的依赖 frontier 已开放，后续需各自证明授权数据契约，17 继续负责 control。
- 合并态检查（集成工作区执行）：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/AgentSidePanel.test.tsx src/agent/AgentHeader.test.tsx src/resources/SharedAgent.test.tsx src/agent/MessageIdentity.test.tsx src/agent/ActiveThinking.test.tsx` 通过，5 文件 / 54 tests，无排除或跳过；包含修复全部七项 06 折叠操作断言的三个完整回归文件。`npm --prefix web run typecheck` 通过。
- `git diff --check ef5e20c1f51b57833f115a61da55e57a70a4fe2a HEAD` 与记录提交前的 `git diff --check` 通过。未重复 worker 已有的私有 target 非零 Rust/file-boundary/managed-settings 或广泛/native 检查，本次没有 Rust 运行；新增 durable 输出记录仍要求兼容 reader，具体证据边界和限制见票文档。
- 04 继续负责本轮 Home/创建集成修复；Console 随本票面板接线由 506 行增至 524 行，行数超限亦继续交由 04。已修复的七项披露断言及这些待修复项均不是原始基线缺陷；原始 skill contract/Core fixture、格式及 508 行 Agent service 问题独立保留。
- 本次仅写入集成 worktree；无原工作区或其他 worker 源码编辑，无推送、部署或生产操作。

## Ticket 04 集成（2026-11-03）

- 合并前 `codex/cma-ui-optimization` 工作区干净，HEAD 为 `a426b87ee5852866850cab192b37d14f9f2087c6`；该状态已包含于票分支。已读取票文档及 `04-EVIDENCE.md`，64 路径 manifest 与实际 diff 完全匹配。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-04` 的 `60e312a48ee2ad6c5ff3ba7481df1e95e6388c4c`，merge commit 为 `60af70f63fd3e621298fb50649f96e1ac5d53633`。无冲突，初始合并树与票分支完全一致；Web Style 保留既有全文为原样前缀，仅追加 35 行，无需样式修正。
- 本票交付仓库/ref 与原任务冻结、真实 checkout receipt 及原请求恢复；其实际 candidate/native 证据及限制见 04 文档，本 merger 未重复。原 01/03 首页/创建断言已按实际导航修复，Console 通过职责提取回到 494 行，OpenAPI schema inventory 为 498 行；05 名称、14 computer 观察及其他既有功能保留。私有文件 guard 和内部 managed-instruction 实现与合并前逐字一致。
- 按 parent 明确授权，独立修正 14 遗漏的 REST checked inventory：`docs/API_REST.md` 增加 `GET /api/v1/agents/{id}/environment`，无 body/query、200 `EnvironmentView`，与实际公开 OpenAPI 相符。修正提交 `54a7414b6e8fe2e71adcc1dba5d2b132456393f0`，只增加该行，没有修改或弱化检查器；这是 14 集成遗漏，不是原始基线缺陷。
- 合并态 Web 检查：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/create/TaskHome.test.tsx src/agent/Details.test.tsx src/AppCreation.test.tsx src/Console.test.tsx` 通过，4 文件 / 41 tests；`npm --prefix web run typecheck` 通过。测试输出有两条 jsdom 跨 Document 导航未实现提示，无失败或跳过。
- 库存修正使用集成独有、非 symlink 的 `api/target`，从 04 使用的缓存 APFS clone 后执行 `cargo +1.96.1 clean --offline --manifest-path api/Cargo.toml -p layer1-public`，仅清理私有 target；实际移除 12,139 文件，随后日志显示从 integration 路径重编译 Layer 3、2、1。环境：`CARGO_BUILD_JOBS=2`、`CARGO_TARGET_DIR=/Users/liuyitian/code/cma-ui-integration/api/target`、`RUSTFLAGS='--cfg cma_ui_ticket04'`、`RUST_MIN_STACK=33554432`。
- 实际命令：`cargo +1.96.1 test --offline --locked --manifest-path api/Cargo.toml -p layer1-public --lib rest::openapi::tests::every_exported_operation_appears_in_the_rest_documentation_inventory -- --exact --nocapture`；实际 1 passed、0 failed、0 ignored、421 filtered。日志为集成工作区 `api/target/integration04-inventory-clean.log` 与 `api/target/integration04-inventory-test.log`；无其他 Rust/broad/native 重跑。
- `git diff --check a426b87ee5852866850cab192b37d14f9f2087c6 HEAD` 与记录提交前的 `git diff --check` 通过。LocalAgentV2 head-404 已由 04 在原始 `60fc98ee5` 实测确认，保留为原始 fixture 缺陷；其他已记录原始 skill/Core/格式/508 行 service 限制未混入修正，不宣称全仓或生产环境绿色。
- 本次仅写入集成 worktree；无原工作区或其他 worker 源码编辑，没有清理共享 Cargo target，无推送、部署或生产操作。

## Ticket 07 集成（2026-11-03）

- 合并前集成工作区干净，HEAD 为 `d19c225bd2a373f0e4861e95ebbbb5a5bca8adc8`。已读取完整 07 票证据、主要路径与共享 hunk 交接，并审阅实际 79 路径清单；候选包含该最新基线。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-07` 的 `06f884afcc08210a4ffae699df25f4956bdd2686`，merge commit 为 `2e37395cb94a7ec12d58171978dcfc02776d1fa0`。无冲突，初始合并树与候选完全一致。
- 候选将交付样式插入既有 04 条目前；独立文档修正 `d6d154bc3c81e1541eb023cd4c6b622642db7524` 保留全部既有字节为前缀，将 07 原样追加。其他候选源码完全一致。09 finalAnswer/反馈、06 plan、02 elapsed、14 computer/文件 guard 与既有 browser 接口保留。
- 合并态：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/AnswerDeliverables.test.tsx src/agent/MessageStream.performance.test.tsx` 通过，2 文件 / 11 tests；`npm --prefix web run typecheck`、`git diff --check d19c225bd2a373f0e4861e95ebbbb5a5bca8adc8 HEAD` 及记录提交前的 `git diff --check` 通过。未重复广泛/native/Rust 检查。
- 07 完成，08 可从记录提交启动，沿用独立资源种类、精确 answer/delivery/revision/digest、当前授权、metadata-only 列表与有界 body 下载；不把 live file 当不可变快照。
- parent 的独立 integration-repair worker 继续处理本轮重复 `.agent-files-panel` CSS、filesystem/conversation 行数超限、environment shared JSON inventory、AppPublic/TaskHome/URL 断言及 task-status 查询成本；本 merger 不重复。07 后 filesystem 为 517 行（增加模块声明），此前 515；原始 508 行 Agent service、skill/Core/格式/LocalAgentV2 head-404 仍独立记录，不称全仓绿色。
- 仅写集成 worktree；无原工作区或其他 worker 源码修改，无推送或部署。

## Ticket 12 集成（2026-11-03）

- 完成 07 后从干净的 `a752868183d220dddc98412d253cd69eeaf21b93` 开始；已读取 12 票和 `12-EVIDENCE.md`，16 路径 manifest 与候选相对共同基线 `d19c225bd` 的 diff 完全匹配。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-12` 的 `a977d65b90b74c29d6f6e11675b4630ce8b3e04c`，merge commit 为 `510c175f7b351f6ab418190c9c6fdf75d416be3f`。唯一冲突为 Web Style 末尾同时追加；保留已集成 07 全文，再原样追加 12，已断言严格前缀和增量完全一致。AgentSurface 自动合并，无源代码冲突或额外修正。
- 保留 07 delivery context、既有 finalAnswer、plan/completion/computer，以及正常 composer/transport。编辑复用建议身份，保留附件、明确追加/替换/取消与撤回，未确认请求仍保留原 body/key，失效来源转普通草稿；不新增发送协议。
- 合并态：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/AgentSurface.suggestionEditing.test.tsx src/agent/AgentSurface.suggestions.test.tsx` 通过，2 文件 / 29 tests；`npm --prefix web run typecheck`、`git diff --check a752868183d220dddc98412d253cd69eeaf21b93 HEAD`、冲突解决后的 `git diff --cached --check` 及记录提交前的 `git diff --check` 通过。
- 无广泛/native/Rust 重复验证。07 记录的独立 integration-repair 任务和原始基线问题仍分开保留；未修复、跳过或弱化这些检查。仅写集成 worktree，无其他 worker/原工作区源码编辑，无推送或部署。

## Ticket 10 worker 交付（2026-11-03）

- 独立 worktree `/Users/liuyitian/code/cma-ui-worktrees/ticket-10`、分支 `codex/cma-ui-10`，从 `7739c431f` 开始；`262a2c6fc` 先合入 14，最终 `e5f5e952b` 合入最新集成 `d19c225bd`（含 04/05/14）。样式先于 UI，合并冲突保留集成全文为原样前缀，仅追加本票规则。由独立 merger 合入集成；无推送、部署或 PR，保留本 worktree。
- 六项多选、500 Unicode 码点可选说明；原因独立版本条件写入与公开回读，精确重试、失联回执恢复、旧客户端 up/null 原子清理及墓碑防 ABA。复用 09 最终回答身份、投票恢复和 `feedbackRecovery`，本页草稿按 viewer/Agent/answer 隔离；关闭、失败、重新编辑及账号/权限改变均有用户边界验证，不产生发送或训练操作。
- 首条指定 red → green 为 `message_feedback_delayed_reasons_after_up_never_restore_down_or_reasons`，实际 1 failed → 1 passed。合并态 A/投影 12、Core 2、Mongo 4 个命名测试通过；真实 handler 新原因 26 + 旧 vote 18 案例通过；Mongo 使用共享 ticket11 容器的独立 UUID 数据库，48 轮 up/null 竞态与 12 路重试通过，数据库已清理，共享 VM/容器保留。
- 合并态相关 Web 115 文件/950 tests 通过（含 14 的七项折叠回归）；最终聚焦 44 tests 通过。1440/390px Chrome 各通过，截图已检查；TypeScript SDK 2740 tests、Rust SDK 全部 2732 个分类契约通过；类型、lint、构建和 48 个 SDK 文档示例通过。[完整验收矩阵、日志、复跑命令及 51 路径 manifest](10-EVIDENCE.md)。
- 上游门禁单独交接：14 的 `.agent-files-panel` 双文件声明导致 CSS gate 失败，已在纯 `a426b87ee` 快照独立复现；14 的新增 environment route 缺少共享 JSON contract inventory/corpus 登记，合并态实际 230、登记 229，独立于最新集成已修复的 REST 文档 inventory。10 两个路由及所有新旧反馈契约通过；不通过删路由、改计数或 skip 隐藏缺口。最终 AppCreation/Console 与反馈共 57 tests 通过，AppPublic 六项旧首页断言仍失败。04 的首页/Console 修复只随集成合入，原始格式/技能 fixture/508 行 service 未修改，不声明全仓绿色。
- 本票触及源文件均 ≤500 行，测试 ≤2000 行；OpenAPI 复用 04 抽出的组合函数并保留其 repository 扩展，主文件 496 行。无原工作区、集成工作区或其他 worker 源码编辑；只关闭本票本地服务，保留共享 Mongo 和 worktree。

## Ticket 10 集成（2026-11-03）

- 顺序完成 07、12 后，从干净的 `01656f54f0c42a9e891b906a29172d9e1cbe1d24` 开始；已读取 10 票、`10-EVIDENCE.md`，51 路径 manifest 与候选相对共同基线 `d19c225bd` 的实际 diff 完全匹配。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-10` 的 `f76ba24befce02ec9d8827bc02d428ac65db2b6e`，merge commit 为 `707b108ebda4b9b09a9a390c0c6c691ba0baf535`。实际四处冲突：OpenAPI composition 保留 07 deliverables 与 10 feedback 的 document merge 和既有 repository 扩展；Web client 保留两组独立能力；Web Style、IMPLEMENTATION 保留全部先前文本，再原样追加 10 内容。已逐字断言两份文档等于原完整前缀加候选增量。
- 保留既有 finalAnswer/投票身份、交付资源 authority、plan/elapsed、computer/browser、12 普通 composer 编辑/附件/undo 与发送恢复；无行为选择或额外产品修正。
- 合并态 Web：`npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/FeedbackReasons.test.tsx src/agent/AnswerFeedback.test.tsx src/agent/MessageStream.performance.test.tsx src/agent/AnswerDeliverables.test.tsx` 通过，4 文件 / 35 tests；`npm --prefix web run typecheck` 通过。delivery 回归覆盖实际共享 client 合并，没有重复广泛/native 证据。
- 因实际 OpenAPI composition 冲突，追加局部 Rust 验证。使用集成独有、非 symlink 的 `api/target`，`CARGO_BUILD_JOBS=2`、`RUST_MIN_STACK=33554432`、`RUSTFLAGS='--cfg cma_ui_ticket04'`；先执行 `cargo +1.96.1 clean --offline --manifest-path api/Cargo.toml -p layer1-public`（仅私有 target，移除 522 文件 / 1.1 GiB），日志确认从 integration 路径重编译 Layer 2、1。
- 首次 `cargo +1.96.1 test --offline --locked --manifest-path api/Cargo.toml -p layer1-public --lib rest::openapi::agent_manager::tests:: -- --nocapture` 实际 9 passed / 1 failed：路径计数仍为 07 的 63。10 加一个路径、两个 operation，明确路由表实际为 64 / 79；独立修正提交 `ddb73f7d12bcd4afaef12770ba79312cc493b6ef` 仅将两个精确期望从 63 / 77 更新为 64 / 79，保留逐项 full/focused 等价、唯一 operation ID、真实 router、schema 引用与身份断言。该局部 count 是本批集成问题，不是原始基线；与未修复的 environment shared JSON inventory 缺口不同，没有改其数字或弱化 gate。
- 同命令重跑实际 10 passed、0 failed、0 ignored、428 filtered；包含新旧操作、feedback schema、answer-bound deliverables 和实际 router 选择检查。另执行 `cargo +1.96.1 test --offline --locked --manifest-path api/Cargo.toml -p layer1-public --lib rest::openapi::tests::every_exported_operation_appears_in_the_rest_documentation_inventory -- --exact --nocapture`，实际 1 passed、0 failed、0 ignored、437 filtered。证据保存在集成 `api/target/integration10-openapi-{clean,test,recheck}.log` 和 `api/target/integration10-doc-inventory.log`；无共享 target 清理、0-test 证据或其他 Rust/SDK/Mongo/native 重复。
- 冲突解决后的 cached diff check、`git diff --check 01656f54f0c42a9e891b906a29172d9e1cbe1d24 HEAD`、本批 `git diff --check d19c225bd2a373f0e4861e95ebbbb5a5bca8adc8 HEAD` 与记录提交前的 worktree diff check 通过。已确认 07/12/10 三个交付 SHA 均为最终集成祖先。
- parent 的独立 repair worker 继续负责重复 CSS、filesystem/conversation 行数、environment shared JSON inventory、AppPublic/TaskHome/URL 断言与 task-status 查询成本；不重复这些任务，不将其称作原始基线。原始 508 行 Agent service、skill/Core/格式/LocalAgentV2 head-404 限制保持独立。08 已由 parent 从 `01656f54f` 启动；本次只写集成 worktree，无原工作区/其他 worker 源码编辑，无推送、部署或全仓绿色声明。


## Ticket 13 worker delivery（2026-11-03）

- 独立 worktree `/Users/liuyitian/code/cma-ui-worktrees/ticket-13`、分支 `codex/cma-ui-13`，起点 `424b1534e`。先提交 Web Style `1455d4d06`，再提交 public/UI late-A red→green `f958b0bae`、生产持久化/模型适配/SDK/实际样本 `d0cff82a0`，以及可见曝光和恢复回执指标修正 `3a1aa9a46`。
- 已在本票分支合入最新集成 `d19c225bd2a373f0e4861e95ebbbb5a5bca8adc8`，merge `543ea035e`；仅 Web Style 追加点冲突，最终文件严格保留集成全文前缀并原样追加本票块。未写原工作区、集成 worktree 或其他 worker；无 push/PR/deploy。
- 确认最终回答后同事务保存有界生成任务；既有授权模型代理异步生成；稳定 owner/Agent/canonical answer/strategy identity；GET/history 不生成、不启动 Agent turn。一次持久 claim、重启明确失败、晚到结果及新 input/authority fence、用量/延迟/provider 明示费用保存已实现。复用 11 `SuggestedReplySet` 和 conditional normal-send；12 独占编辑行为。
- 固定 rubric 先于样本；最终 5 次实际调用覆盖比较、文档修改、中文澄清、无合适建议、错误成果声明，公开 GET 与重新打开的 Mongo store 核实保存值。共 2,200 input / 270 output tokens，35,629ms 总生成时间；实际费用字段缺失，保存 null，未推算费用。早期失败质量样本完整保留。样本已交 parent 独立复核，不将实现 AI 或 parent AI 评价称作人工审批。
- 最终合并态：8 public generation tests、4 Mongo tests、6 条本票 public contract cases、53 Web U tests、7 选定 TypeScript SDK tests、2 Rust SDK tests 通过；Web/E2E types 与 focused lint 通过。此前合并态真实 Chrome 5 tests 通过，1440/320 px 双主题截图已保留。日志与明细见 [13-EVIDENCE](13-EVIDENCE.md)。不重复无改变风险的付费模型样本或广泛测试。
- 该早期 checkpoint 的 12 编辑接线已在下述最终整合中完成；状态仅为 literal human quality signoff 保留 in-progress。provider 实际金额仍未知，明确保留限制，不虚构费用。13 的生成、保存、读取和本地有界指标实现均已交付；不建立 analytics 平台。
- 独立报告 incoming 14 的 shared public-contract inventory 缺口：`GET /api/v1/agents/{id}/environment` 无 entry/cases，合并态实际 229 vs inventory 228；本票 endpoint + 6 cases 完整通过。04 已修正 REST 文档 inventory 和 OpenAPI source-size，并修复其负责的 AppCreation/Console 问题；这些与原始 skill fixtures/format/508-line Agent service 基线分开记录，不扩大本票修复范围。
- 自有 preview/model servers 已停止；本票测试数据库已清空，shared Mongo ping=1，未停止 VM/container。保留 worktree 和私有 Rust targets 供 parent 清理。独立人审样本入口：[13-MODEL-SAMPLES](13-MODEL-SAMPLES.md)。

### Ticket 13 最终整合交接

- 已合并最新 integration `f793a41518fa969af068d7913ff6310dee7e040b`，包括 07、12、10；`6de02dae9` 为最终基线合并。12 的编辑行为与正常 admission/recovery 保留，13 只接入保存建议和 edited receipt/outcome 指标，无重复 editor、answer identity 或消息协议。
- 编辑指标先红后绿：直接确认及丢回执恢复两例最初因缺失指标失败；最终真实 AgentSurface/transport 证明未确认不计数、原 body/key/condition 恢复、新草稿保留、一次 edited confirmation 和独立的后续任务 outcome。组合 U 88 passed，最终 hook 生命周期复验 40 passed；Chrome 6 passed，含真实样本文本的编辑与 late-A。
- 最新合并态 public lifecycle 8、Mongo 4、shared HTTP 1 test/6 cases、focused OpenAPI 10、REST docs 1、TS SDK 7、Rust SDK 2 named tests 均通过；types/lint/局部格式/secret scan 通过。精确日志和环境边界见 [13-EVIDENCE](13-EVIDENCE.md)。OpenAPI 13 新增一个 GET 的精确 count 已调为 65 paths / 80 operations；保留全部 identity/schema 检查。
- parent 已独立读取固定 rubric、五例实际 raw output 与 saved public JSON，全部 v3 样本通过。报告原样保存在 [13-PARENT-AI-REVIEW](13-PARENT-AI-REVIEW.md)，仅为 AI review，绝不称 human approval。所有机器工作完成；仅 literal human 质量验收留待整套 19 交付用户评审，tracker 仍明确 in-progress。实际账单缺失仍为 null/unknown，不虚构费用，不阻塞 16–19。
- 共享 inventory 235 actual / 230 declared 的缺口由 incoming 14 environment 和 07 四个 deliverable operation 组成；13 自身 entry/cases 完整。已报告 parent repair，未改 gate 或重复 CSS/size/task-status/AppPublic 修复。原始 baseline 限制仍分开记录。
- 13 自有 source ≤500（AgentSurface/OpenAPI 均 495）、test ≤2000；owned preview/model proxy 已停止，无遗留 13 测试库，shared Mongo/VM 保留运行。保留 worktree/private targets 供 parent 清理；无 push、PR、部署或原/其他 worker 工作区修改。

## Ticket 13 实现集成，人工验收待定（2026-11-03）

- 合并前集成工作区干净，分支 `codex/cma-ui-optimization`、HEAD `f793a41518fa969af068d7913ff6310dee7e040b`。已读取 13 ticket、完整 `13-EVIDENCE.md`、`13-PARENT-AI-REVIEW.md`，审阅相对该基线的实际 82 路径清单与 suggestion/editor/client 共享修改；候选包含该最新基线。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-13` 的 `83ea396a4d49535cbc1484034b34f2b3202b3bb2`，merge commit 为 `171a6a82d90695428f23c7fd6d577ac059369fde`。无冲突，合并树与候选逐文件完全一致，无 integration-only 源码修正。已断言 Web Style 和已有 ledger 全文保留为原样前缀，13 样式只在末尾追加。
- 合并态执行 `npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/AgentSurface.contextualSuggestions.test.tsx src/agent/AgentSurface.suggestionEditing.test.tsx src/agent/AgentSurface.suggestions.test.tsx`，实际 3 文件 / 40 tests 全部通过；`npm --prefix web run typecheck` 通过。覆盖当前回答建议、失效/晚到结果、12 编辑/草稿恢复与 receipt 指标整合；未重复 native、真实模型、Mongo、SDK 或广泛检查。
- `git diff --check f793a41518fa969af068d7913ff6310dee7e040b HEAD` 和记录提交前的 worktree/cached diff check 通过。已断言 MessageFooter、StreamEntry 与 MessageStream performance tests 相对合并前完全未变，保留既有 finalAnswer、10 reasons 和交付展示；共享编辑仍使用 12 的 editor、普通消息协议与既有选择/恢复身份。
- 本次只合入实现与机器证据，13 仍为 **in-progress / 尚未完整验收**。逐字核对原始 AC 文本未变，恰好一个 literal independent human quality criterion 未勾选；parent AI review 通过五个 v3 样本不等于人审，不将其改为完成。实际 billed cost 继续为 null/unknown，无零值替代、估算或虚构 native/model availability；实际样本日志与环境边界保留在既有外部证据中。
- shared JSON inventory 235 actual / 230 declared 的 07/14 缺口仍由 parent repair worker `01a19fd8-0df5-7730-92e2-cefbe954f9b4` 处理；未重复 CSS/size/inventory/task-status/AppPublic 等集成修复。它们与原始 skill/Core/format/508-line Agent service/LocalAgentV2 head-404 问题分别记录，不宣称全仓或部署绿色。仅写集成 worktree，无原工作区或其他 worker 源码修改，无 push、PR 或部署。

## Ticket 08 worker delivery（2026-11-03）

- 独立 worktree `ticket-08` / `codex/cma-ui-08`，确认起点 `01656f54f` 包含 07 + 12；使用已确认 U/A/Chrome 边界，无新增人工澄清。原始七项 AC 原文保留并逐项完成，见 [08-EVIDENCE.md](08-EVIDENCE.md)。
- 样式先追加再实施（`2baa0a81c`），预览实现 `43171c01c`。复用 07 的精确回答关联、版本、digest 和授权下载，以及既有 FilePreview、MessageContent、useAuthorizedRead、AppFrame；没有新协议或动态模型调用。HTML/SVG/链接不成为交互页面，PDF 明确下载回退，1 MiB 预览上限保留。
- 只在 Console 用独立 frame wrapper 接入；没有改 AgentSidePanel/AgentComputer/AgentSurface/MessageStream 或 15、18 的 browser/replay 容器。已访问文件编辑器持续挂载，预览切换和关闭不会替换会话/composer，资源撤销清除受限内容。真正 Chrome red 修正了移动端 inert 清除后的原卡片焦点，以及下载后 Escape 的处理。
- `441524648` 将最新 integration `a2965df09` 合入本分支（10、13 已纳入）。唯一冲突为 Web Style 末尾；逐字断言结果为 integration 完整前缀 + 本票原样追加，没有改写已有规则。`e5b38408b` 收紧关闭 callback dependency。
- U 聚焦 10 文件 / 65 passed；本票三文件共 19 tests。整套合并态 Web 333 文件：330 passed / 3 failed，2,985 tests passed / 8 failed。失败为既有 AppPublic 6 项、REST tool_output URL 1 项及 TaskHome public-copy 1 项，不弱化断言或重复 parent repair。
- A：独立 target + `--cfg cma_ui_ticket08`、Rust 1.96.1、jobs 2；5 个回答交付 Router 测试和 1 个精确 OpenAPI inventory 测试真实非零通过。TS SDK 9 tests 通过。没有新增 API/SDK source 或契约。
- Chrome 1440px/390px 共 4 场景通过，覆盖实际下载字节、键盘/触屏、原卡片焦点、详情/文件/预览往返、草稿/附件/节点/滚动、延迟保存、危险 HTML/SVG/Markdown、PNG 解码、PDF 回退、撤销清除。TypeScript、E2E typecheck、ESLint、production build、owned Prettier 和 diff check 通过；截图已目检。
- 外部证据根目录 `/Users/liuyitian/code/cma-ui-worktrees/evidence/ticket-08`，含逐阶段 red/green、全部命令日志、Chrome PNG、独立 Cargo executable/hash、21 路径 manifest 与 shared-hunks。CSS 剩余 incoming `.agent-files-panel` 碰撞；size gate 剩余 incoming 556/517/503 和原始 508 行问题。没有将整仓或生产声明为绿色。
- 所有写入仅本 worktree；无 push、PR、部署、动态模型生成或生产操作。验证后停止本票 Vite/Chrome，不动共享 Mongo/VM 和其他 worker，保留 worktree/证据供独立 merger 集成。

## Ticket 08 集成（2026-11-03）

- 合并前集成工作区干净，分支 `codex/cma-ui-optimization`、HEAD `a2965df09581de5fa0fa35af54b69db56cd83ab9`。已读取 08 ticket、完整 `08-EVIDENCE.md` 和外部 `evidence/ticket-08/manifest.txt`、`shared-hunks.patch`；21 路径 manifest 与候选实际 diff 完全一致，5 个共享文件的 patch 也逐字匹配。原始七项 AC 文本未改，全部已勾选。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-08` 的 `11dbdbc8b2d02f975653ad74aaf930da2109b326`，merge commit 为 `f01deeb6fad63b03c91b366d84037fdc0acbb063`。无冲突，合并树与候选完全一致；无 integration-only 源码修正。已断言 Web Style 和已有 ledger 保留全部原样前缀，08 样式仅追加。
- 合并态执行 `npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/ConsoleDeliverablePreview.test.tsx src/agent/DeliverablePreview.test.tsx src/agent/SharedDeliverablePreview.test.tsx src/agent/AnswerDeliverables.test.tsx src/agent/MessageContent.test.tsx src/agent/MessageStream.performance.test.tsx src/agent/AgentSidePanel.test.tsx src/agent/files/AgentFiles.test.tsx src/agent/files/AgentFiles.presence.test.tsx src/ConsoleDraftRecovery.test.tsx`，实际 10 文件 / 65 tests 全部通过；`npm --prefix web run typecheck` 通过。
- 聚焦覆盖回答资源选择、安全只读渲染/权限清除、PDF 下载回退、面板/文件编辑保持、消息性能与 composer 草稿恢复。API/SDK 无源码变化且无冲突，不重复 worker 的 public Rust、inventory、SDK、Chrome 或广泛/native/model 检查。`git diff --check a2965df09581de5fa0fa35af54b69db56cd83ab9 HEAD`、worktree 和记录提交前 cached diff check 通过。
- 已断言 13 tracker 与合并前逐字一致，仍为 in-progress，唯一 literal human quality AC 未勾选；没有将 parent AI review 变成人工批准，实际 billed cost 仍为 null/unknown。
- worker 全 Web 的八项已知集成失败以及 CSS/size/shared JSON inventory/task-status/10 helper 等修复仍由 parent repair 负责；其候选尚未合入本次集成，merger 未重复或掩盖这些检查。原始 skill/Core/format/508-line service/LocalAgentV2 head-404 限制仍分别记录。15 的有界环境验证与独立 `response-compat-repair` 工作未触碰；不宣称全仓或部署绿色。
- 仅写集成 worktree，无原工作区或其他 worker 源码修改，无 push、PR、部署或外部运行环境操作。


## Ticket 18 完成与交接（2026-11-03）

- 分支 `codex/cma-ui-18`，独立 `ticket-18` worktree，自 `d19c225bd` 开始；`1dd75c6f4` 合并 `a2965df09`，`d4cc51f0a` 再合并最新集成 `7cd701d0f`（含 08），实现与 shared-corpus 验证提交 `bb139fbf8`。未改原验收文本，8 项原 AC 均有证据；见 [18-EVIDENCE](18-EVIDENCE.md) 和精确 [18-FILE-SCOPE](18-FILE-SCOPE.md)。
- 复用已有 public v2 event snapshot/head/finite tail、类型化 SDK 和 14 panel/observation。按 Agent incarnation、实际 turn、tool id、durable cursor 回放已记录命令/输出/状态/时间与当时计划；50 条窗口、公开有界分页、缺口/到期/截断明示。键盘 Previous/Next/Return Live；历史停留不受 live 新事件拖动，刷新按当前授权重读，账号/权限切换取消旧请求。06 plan 与 completed answer 定位同一实际 turn，无计划也可回放。
- 首个 red/green 覆盖停在操作 2、新操作 4 到达仍在 2、Return Live 到 4 且无额外执行。最终 focused U 20 passed，合并 Console/shared seam 34 passed，public v2 26 passed（native 单独显式运行），core 权限 4 named tests passed。types/build/目标 lint/format 通过；memo/performance 保留。
- shared JSON inventory 已补充复用 route 的 replay 语义；新增六个 shared corpus case，经真实 public handler、TS typed dispatch（6 cases + 1 coverage）、Rust typed dispatch（6 HTTP cases）通过。SDK dispatch 显式使用 typed toolOutput。Markdown API 文档及 standalone SDK 103 TS / 10 Rust tests 为独立证据；不引入 private API、新 Chat model 或 inventory 豁免。07/14 缺项由 parent repair worker 独立处理，保留最新合并 entries。
- 实际 Codex 0.147 / patched ACP 1.4 在已固定 ticket04+14 测试镜像内执行四个无害命令，真实输出经过 recorder/public HTTP/Chrome reload，执行计数 **4→4**，无额外 API 写请求；native、public-native、Chrome 均显式 1 passed。镜像为最小测试包装，模型边界受控，HTTP 使用测试 owner/in-memory store；不称 production/Mongo restart/录像证据。没有 L4 修改或 Linux build，无视频/PTY/Browser private context/control。
- 仅 ticket18 路径符合 500 source / 2000 test gate；全仓三个超限文件与 `a2965df09` 字节一致（508-line Agent service、503-line conversation、556-line feedback test 的现有 gate 分类），既有 skill/format/LocalAgentV2 head-404 问题单独保留，不宣称全仓绿色。最新 integration 保留 07/10/12/13 交付、反馈、编辑和建议逻辑，13 的待人工验收状态未改。
- 08 合并后针对 replay/Console preview/panel/memo 再验 4 文件 / 15 passed，types/build/Console format 通过；Console 500 行。真实 native→public HTTP→Chrome reload 全链再次通过，执行计数仍 4→4；最新证据在 `ticket18-proof/merged08-runtime/`，此前证明日志保留。Web Style 和 ledger 严格保留最新 integration 前缀，再追加 18。
- 自有 preview/native/public 服务已停止；shared Colima/Mongo 保持运行。保留 worktree/private target/proof artifacts，交由 parent cleanup 和独立 merger；无 push、PR、部署，未写原工作区、integration 或其他 worker 源码。

## Ticket 18 集成（2026-11-03）

- 合并前集成工作区干净，分支 `codex/cma-ui-optimization`、HEAD `7cd701d0fe6c2496256a01eca59919ee8d899c82`。已读取完整 `18-EVIDENCE.md`、`18-FILE-SCOPE.md` 和 18 ticket；37 路径 manifest 与实际 diff 完全匹配，原始八项 AC 文本未改且均已勾选，候选包含最新 08 集成。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-18` 的 `68c8b8192aff5ce3f2ecdad035044d4da0f5cfdc`，merge commit 为 `70482a73b88067f4cdfa11bf8fddc570f5a52ae7`。无冲突，合并树与候选完全一致，无额外源码修正；Web Style 与原 ledger 严格保留完整前缀后追加 18。
- 共享 hunk 为 Console/Computer observation、AgentSidePanel replay 入口、AgentSurface/SharedAgent 到 MessageStream/StreamEntry/PlanCard 的实际 turn 导航，以及公开 typed snapshot/head/tail reader；保留现有 finalAnswer/feedback、08 preview wrapper 与文件编辑保持、13 保存建议/12 编辑身份。environment 仅声明只读 operation_records_only，纯 incarnation/history helper 移至既有 agent_environment 模块，文件私有访问 guard 不改；不新增 API route、视频、Browser 私有上下文或 control 能力。
- 合并态执行 `npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/OperationReplay.test.tsx src/ConsoleDeliverablePreview.test.tsx src/agent/AgentSidePanel.test.tsx src/agent/MessageStream.performance.test.tsx src/agent/PlanCard.test.tsx src/resources/SharedAgent.test.tsx src/Console.test.tsx`，实际 7 文件 / 56 tests 全部通过；`npm --prefix web run typecheck` 通过。覆盖历史固定游标/Return Live、实际 turn 链接、共享/owner Computer scope、08 预览与 memo/performance；没有重复 native、Mongo、public Rust/SDK 或全 Web。
- `git diff --check 7cd701d0fe6c2496256a01eca59919ee8d899c82 HEAD`、worktree 和记录提交前 cached diff check 通过。已逐字断言 08 DeliverablePreview/Frame、13 useAgentSuggestions、12 useSuggestionEditing 与 13 tracker 保持合并前内容；13 仍 in-progress、literal human quality AC 未勾选，费用仍 null/unknown。
- worker 的固定 Codex/ACP native→public HTTP→Chrome/reload 4→4、零 replay API 写请求和六条 shared handler/typed SDK case 证据保留，merger 不将受控模型、测试 owner/in-memory store、最小镜像包装称为 production/Mongo restart/录像证明。07/14 shared corpus、10 helper/size、其他既有集成修复仍由 parent repair 负责；原始 skill/Core/format/508-line service/LocalAgentV2 head-404 限制另记，不宣称全仓绿色。
- 仅写集成 worktree，无原工作区或其他 worker 源码修改。独立 Responses compatibility 修复及 15 Linux/gVisor 验证未触碰；无 push、PR、部署或外部运行环境操作。

## Legacy Responses compatibility repair 集成（2026-11-03）

- 合并前集成工作区干净，分支 `codex/cma-ui-optimization`、HEAD `eab5011c02d32959512ddd2ed09f7af618e46526`。已读取 `RESPONSE-COMPAT-REPAIR.md` 和外部 `evidence/response-compat-repair/manifest.json`；七路径 manifest/source SHA-256 与候选完全匹配，34 个外部证据 artifact 的字节数与 SHA-256 全部核实。四个 SDK production parser 文件 blob 与记录的 baseline 完全一致。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-response-compat-repair` 的 `22054a49b0798008a1563d09d0ee38cd674abb07`，merge commit 为 `1440a107ba6e0d738ac494b98c092d45ab4c10e6`。无冲突，合并树与候选完全一致；无 integration-only 源码修正。仅修复 legacy Responses 的 output-only activity envelope/recorded item correlation，保留已记录 kind/target/status 与省略 output 语义；不改默认 transport、v2/replay 或 UI/Web Style。
- 局部 Rust 使用集成独有非 symlink `api/target`、`CARGO_BUILD_JOBS=2`、`RUST_MIN_STACK=33554432`、`RUSTFLAGS='--cfg cma_ui_ticket04'`。先执行 `cargo +1.96.1 clean --offline --manifest-path api/Cargo.toml -p layer1-public`，仅清理私有 target（1038 files / 2.2 GiB）；日志确认从 integration 路径重编译 Layer 2、1。
- 两次执行 `cargo +1.96.1 test --offline --locked --manifest-path api/Cargo.toml -p layer1-public --lib <exact-name> -- --exact --nocapture`：`responses::output_compat_tests::output_only_legacy_frames_preserve_recorded_activity_across_reads_and_replay` 与 `responses::output_compat_tests::legacy_output_requires_recorded_context_and_omitted_output_stays_absent` 各实际 1 passed、0 failed、0 ignored、450 filtered。证据在集成 `api/target/integration-response-compat-{clean,replay,context}.log`；未以共享旧 binary、零测试或未配置 URL 的跳过测试计为成功。
- `web/node_modules/.bin/tsc --noEmit --project sdk/typescript/tsconfig.json` 最初因集成 SDK 缺少 node/vitest type dependencies 报 TS2688；仅在集成 ignored `sdk/typescript/node_modules` 增加 14 个指向已有 Web 依赖的链接，无 install、lockfile 或原工作区写入，重跑通过。`git diff --check eab5011c02d32959512ddd2ed09f7af618e46526 HEAD`、worktree 和记录提交前 cached diff check 通过。
- 已逐字确认 Web Style、18 native replay proof 文档/runner、13 tracker 与合并前一致。18 既有 4→4 proof 不改；13 仍 pending literal human quality signoff，实际 billed cost 为 null/unknown。worker 的 actual HTTP / baseline+current typed-parser 12 帧证据哈希保留，此次不重复 loopback fixture、native、Chrome、Mongo、全 Web 或全 SDK。
- 本修复对应 14 暴露的具体 wire 兼容问题，不将其称为原始基线，也不替代 19 组合验收。独立 integration repair 的 07/14 inventory、10 helper/size 等工作与 15 gVisor/native finishing 未触碰；原始 skill/Core/format/508-line service/LocalAgentV2 head-404 限制仍分开记录。仅写集成 worktree，无原工作区/其他 worker 源码写入，无 push、PR、部署或外部运行环境操作。

## 全部已分派 integration repairs 集成（2026-11-03）

- 合并前集成工作区干净，分支 `codex/cma-ui-optimization`、HEAD `134c27aa0892ec2422fb775b69bf93c24ec76def`。已读取完整 `INTEGRATION-REPAIRS.md`，40 路径 manifest 与候选实际 diff 完全一致；候选包含 08、18、legacy Responses 修复和最新 ledger。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-integration-repairs` 的 `948a771ca4d979e103b3a07c2882274a6cc5058c`，merge commit 为 `e63baa5fadc85656c4d878f25e854ff966f85bf2`。无冲突，合并树与候选完全一致，无额外源码修正。Web Style 未改、无新增设计规则；18 proof、Responses wire 修复和 13 pending-human tracker 均与合并前逐字一致。
- 自该 merge 起，本轮已分派的新集成缺口全部闭合：Computer/files CSS owner 碰撞；filesystem/conversation source 超限；八项 AppPublic/Home/copy/event URL 旧断言；14 environment 与 07 四项 delivery 的 shared JSON inventory、real-handler corpus 和 typed SDK dispatch；10 feedback fixture 超限；01 non-Ready status 无用 evidence 查询；18 合入后六个成功 environment case 的 replay 能力期望。未加入 skip、allowlist、替代 inventory 或放宽断言。
- 合并态执行 `npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/AppPublic.test.tsx src/create/TaskHome.test.tsx src/vocabulary/publicCopy.test.ts src/api/rest.test.ts src/agent/MessageStream.test.tsx src/agent/MessageStream.performance.test.tsx src/agent/conversation.test.ts src/agent/conversation.performance.test.ts src/agent/AgentSidePanel.test.tsx src/agent/OperationReplay.test.tsx src/ConsoleDeliverablePreview.test.tsx src/agent/DeliverablePreview.test.tsx src/agent/SharedDeliverablePreview.test.tsx src/agent/files/AgentFiles.presence.test.tsx`，实际 14 文件 / 142 tests 全部通过；Web `typecheck` 通过；`npm --prefix web run check:css` 通过，899 classes / 3 原有 deliberate shared classes，无碰撞。
- worker 最新 inventory 日志已直接核实：实际命名测试 1 passed、0 failed、0 ignored，235 actual / 235 declared operations、2,798 cases、1,599 named HTTP problem pairs、23 protocol failure pairs。沿用其最新真实 handler replay/environment/delivery 6/10/44 cases、2,803 TS shared tests、Rust 2,354 typed HTTP cases 与 private-target 证据；此前完整 Web 330 文件 / 2,974 passed 是历史 checkpoint，merger 只运行上述合并态 focused checks，没有声称重跑 full Web、Rust inventory/SDK、native 或 Mongo。
- status fast path 复用现有非 Ready domain projection，保留 Ready 的证据读取和 corruption 检查，避免 parent 已独立复现的 10,001-document 无用查询；worker 的真实 public inventory/Mongo red→green 仍是行为依据，不将合成查询时间称作生产 SLA。私有文件访问/写权限 guard 原逻辑移入 scope 模块，文件分别为 filesystem 448、conversation 469、feedback tests 438、fixture 123 行，已在当前集成核对。
- `git diff --check 134c27aa0892ec2422fb775b69bf93c24ec76def HEAD`、worktree 和记录提交前 cached diff check 通过。直接核实 worker 最新 size gate 仅报告原始 `agent_operation_service.rs` 508 行；当前该文件与原 baseline `c9e2d7b91a0c49c68d3cd2968e61dd8568ed3706` 完全一致。原始 skill/Core fixtures、full-format drift、LocalAgentV2 head-404 仍为独立既有限制，不称全仓/全部 19 票完成。
- 13 仍 in-progress，literal human quality signoff 未勾选，实际费用仍 null/unknown。15 Linux/gVisor/resource-budget 工作继续由原 worker/parent 完成，本 merger 未介入。仅写集成 worktree，无原工作区或其他 worker 源码修改，无 push、PR、部署或共享运行环境操作。

## Ticket 15 contract and native runtime checkpoint（2026-11-03）

- Isolated `codex/cma-ui-15` now includes integration `7cd701d0fe6c2496256a01eca59919ee8d899c82`, preserving 08 previews and all earlier style history. Exact own runtime binary, image, source and test provenance are recorded in `15-EVIDENCE.md`.
- Browser GET/PUT/DELETE are registered in the shared JSON inventory; 36 actual-handler cases and 36 typed HTTP cases in each SDK pass. The global inventory still exposes the five 14/07 entries assigned to the parent repair; no exemptions or invented coverage were added.
- Mac actual native two-page evidence is preserved. CPU is null/unknown after rejecting process-tree deltas; sampled aggregate RSS is not isolated browser working memory, and five-second windows are not sustained-load measurements.
- Own immutable Linux runtime HTTP/ACP now proves two actual native browser-helper refusals under Docker namespace denial. No `--no-sandbox` or production-policy weakening is used. This is not a conclusion about production gVisor: the parent owns an independent runsc feasibility probe.
- 15 remains in progress. Compatible Linux live navigation, the complete authorized-public-service/Web chain and stable Linux resource acceptance are still required before releasing the 16/17 dependency.


## Ticket 15 complete handoff（2026-11-03）

- Isolated branch codex/cma-ui-15 includes latest integration cf6774fd87dd39be8325e01984165efba12c7e87. The 04 Console extraction, 08 preview, 18 replay/selection, full 14/07 shared corpus and style ledger remain intact. All eight original AC statements are unchanged and checked.
- Own immutable runtime build 8be729357, binary SHA-256 364d71a3424476e5c7b27583b0df39cce1751e0d92be06347baa62a1e6dbf531 and image sha256:ddda63f74cd1b2067102fb43e66e1a2c47ced2f569c701bcc52d309bf816e112 pass under verified official gVisor release-20261102.0. No Chrome sandbox weakening, Docker restart or Mongo/VM stop. This is bounded pinned-environment evidence, not production GKE certification.
- Actual native Agent two-page navigation → Rust supervisor HTTP/ACP → private runtime frame transport → real Core authorization/public handlers → typed SDK → Web passes. Test-only file RPC bridges network-none; auth/store/model and unrelated conversation startup are declared fixtures. Final desktop/390/320 screenshots, frame hashes, foreign/viewer/stale refusals, freshness expiry and same-page reconnect are retained in ticket15-proof/native-chain-final.
- Primary resource proof is the full runtime HTTP/native run, not the smaller mediator-only run: four approximately five-second stable host-cgroup windows, CPU 20.35/16.13/15.28/19.78 percent of one core, peak charged bytes 968544256/965300224/952246272/933285888; hidden/closed have zero captures/bytes/leases. Application JSON bandwidth excludes HTTP/TLS. Requested one-core hard cap was not observed; Mac negative CPU is retained only as explicitly invalid superseded evidence.
- Shared JSON inventory passes 238 operations / 2834 cases / 1620 named HTTP problem pairs / 23 protocol failure pairs. Actual handler browser36, environment10, deliverables44; typed Rust browser36, TypeScript browser-name filter56; Web3files/16tests, actual Chrome U1 and native Web1; 14 private-file/shared-workspace guards2, typecheck/build pass. A merged environment corpus exposed a paused unsupported-provider state ordering bug; da83bbdc0 fixes it and the unchanged corpus passes.
- Real default-off owner enablement is ready for 16. Private mediated runtime browser/context boundary is ready for 17; no takeover, PTY, public input route or available-control claim. Full evidence and limits: 15-EVIDENCE.md; paths: 15-FILE-SCOPE.md; source/artifact hashes: external ticket15-proof/manifest.json. Own proof services are stopped; worktree and VM retained for parent cleanup, no deploy/push/PR.

## Ticket 15 集成完成与 16/17 放行（2026-11-03）

- 合并前集成工作区干净，分支 `codex/cma-ui-optimization`、HEAD `cf6774fd87dd39be8325e01984165efba12c7e87`。已读取完整 `15-EVIDENCE.md` 的 native/public/Web 链、资源边界与 16/17 handoff，并核实外部 `ticket15-proof/manifest.json`：84 路径、每项字节数及 source SHA-256 与候选全部一致；八项原始 AC 原文未改且均已勾选。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-15` 的 `77a8faa203b21b373d0282897cf8a2999564682d`，merge commit 为 `0c91396d20aed9268c199ce4c1c335e85abece61`。无冲突，合并树与候选完全一致，无额外源码修正。用户中断时该 merge 已提交；恢复后确认祖先关系与干净状态，继续收取原检查结果，没有重复 merge 或 runtime 探索。
- Web Style 和既有 ledger 完整原样前缀保留，仅追加 15。共享改动在 AgentSidePanel/browser capability/client 接线；已逐字断言 08 preview wrapper、12 editing、13 saved suggestions、18 OperationReplay 与 14 private filesystem scope guard 未改。单一 selection/observation、文件编辑保持和显式 replay 选择保留；13 tracker 仍为 pending literal human signoff，实际费用仍 null/unknown。
- 合并态执行 `npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/BrowserPanel.test.tsx src/agent/AgentSidePanel.test.tsx src/agent/OperationReplay.test.tsx src/ConsoleDeliverablePreview.test.tsx src/agent/DeliverablePreview.test.tsx src/agent/SharedDeliverablePreview.test.tsx src/agent/files/AgentFiles.test.tsx src/agent/files/AgentFiles.presence.test.tsx src/resources/SharedAgent.test.tsx src/agent/MessageStream.performance.test.tsx`，实际 10 文件 / 78 tests 全部通过。`npm --prefix web run typecheck` 与 `npm --prefix web run check:css` 通过，900 classes、3 原有 shared exceptions、无碰撞。
- `git diff --check cf6774fd87dd39be8325e01984165efba12c7e87 HEAD`、worktree 和记录提交前 cached diff check 通过。无实际 Rust merge seam 冲突，未重复 native/Chrome/VM/Mongo、public Rust、SDK 或 full Web；沿用 worker/parent 已审阅的实际 native→supervisor HTTP→private file RPC→Core/public auth→typed SDK→Chrome Web 1/1 链与 Linux native HTTP 1/1 稳定 cgroup proof。
- 已交付的链证据为 15 forwarded runtime requests / 7 Web reads、实际 A/B frames，以及 foreign 404 / viewer enable 403 / stale boot 409；shared inventory 238/238 operations、2,834 cases，最终 Core paused/unsupported precedence 修正后 browser/environment/delivery 36/10/44 real-handler cases 通过。测试替代 private runtime transport，auth/store/model 和无关 conversation startup 明示为 fixture；不称 production network/IdP、Mongo restart 或 GKE。未采用 Chrome `--no-sandbox`；未声称 enforced one-core quota，资源数据是短时稳定 full-cgroup 窗口。
- 15 已完成并解除 16/17 依赖：16 可用 owner-authorized 显式 browser enablement，仍 default-off；17 可扩展受 Agent/Sandbox/boot/context fence 保护的私有 mediated boundary。15 本身不提供 takeover、PTY、public input/navigation route 或 available-control。原始 skill/Core/full-format/508-line service/LocalAgentV2 head-404 限制仍独立，13 人工验收未完成，不将整套 19 票或部署称为绿色。
- 仅写集成 worktree；无原工作区或其他 worker 源码写入，无 push、PR、部署或共享服务/VM 操作。

## Bounded quality repairs 集成（2026-11-03）

- 合并前集成干净，分支 `codex/cma-ui-optimization`、HEAD `9bc2f04e883c6619e65ad7f424994f333c385055`。已审阅 `QUALITY-REPAIRS.md` 和全部实际代码 diff。外部 `evidence/quality-repairs/manifest.json` 的 SHA-256 精确为 `064d80047838dc9c61cb7ed8b9b1912c949b8c6d1d9b9902edede518406c4ec8`；11 路径及各 source 字节数/hash 与候选一致，54 raw-log/evidence hashes 和 4 test-binary hashes 全部读取核实。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-quality-repairs` 的 `3b12254d30622cbb6fa9e70a4b621bc2f0d8a95f`，merge commit 为 `6c7f7cb62c56e353181201e58da9612f8164a8d0`。无冲突，合并树与候选完全一致，无 integration-only 源码修正。相对已验证 source `8ab9c2deeb4ea51e169d15e5498c06e3cc364c76` 仅多出交付文档。
- 范围仅为 07 retained authority boxing、07/10 typed error 的同一 handler projection、共用一次 filesystem runtime fixture、13 Rust SDK 等价 let-chain、SDK corpus 去冗余 borrow 和现有 suggestion/browser public-client harness 的准确请求断言。无 allowance、断言跳过、wire/storage schema 或 UI 变化；Web Style 原样不动。worker 的四层/CMA/Rust SDK all-target Clippy 证据保留，未重复全套 lint 或 parent 已完成的广泛测试。
- 合并态 API 使用真实私有 `api/target`，Rust 1.96.1、`--offline --locked`、jobs 2、`RUST_MIN_STACK=33554432`、保留 cache cfg `--cfg cma_ui_ticket04`，清除 encoded flags 与所有 corpus recording/update selectors。仅私有 clean `layer1-public`、`layer2-core`（16,569 files / 17.7 GiB），随后日志明确显示从 integration 路径编译 Layer 3、2、1。SDK 使用独立真实 `sdk/rust/target`、unset Rust flags，并私有 clean `cma-sdk`（16,880 files / 3.3 GiB）。未写其他 target。
- API 命令基式为 `cargo +1.96.1 test --offline --locked --manifest-path api/Cargo.toml -p layer1-public --lib <filter> -- --nocapture`：`rest::answer_deliverables_tests::` 实际 5 passed、`message_feedback_reasons_` 5 passed、`public_files_` 2 passed，均零 failed/ignored。精确 `rest::public_contract_tests::shared_public_contract_matches_real_backend -- --exact --nocapture` 配合 `CMA_PUBLIC_CONTRACT_FILE` 五个文件依次执行，实际 5 个非零 driver tests / 214 real-handler cases：answer-deliverables 44、feedback-reasons 26、agent-browser 36、agent-filesystem 98、agent-environment 10；recording/update 均关闭。
- SDK `cargo +1.96.1 test --offline --locked --manifest-path sdk/rust/Cargo.toml --test suggestions --test agent_browser -- --nocapture` 实际两项 named tests 各 1 passed、零 failed/ignored。`--test public_contract shared_backend_corpus_replays_through_typed_sdk_methods -- --exact --nocapture` 配合 `CMA_SDK_CONTRACT_FILE` 执行 answer-deliverables 与 agent-browser，各 1 passed，共 77 typed HTTP + 3 real typed preflight cases。所有测试显式核验非零计数；worker 留存的零测试 feedback 尝试仍不计 proof。
- 合并态日志留在 `api/target/integration-quality-*.log`、`sdk/rust/target/integration-quality-*.log`。`git diff --check 9bc2f04e883c6619e65ad7f424994f333c385055 HEAD`、worktree/cached diff checks 通过。没有重新运行 full Web、全 SDK、native、Chrome、Mongo 或 production proof。
- 已读取外部 `concurrent-worktree-additions.json` 与 `check-preservation.mjs`，以 `GIT_OPTIONAL_LOCKS=0` 无输出文件参数运行只读 preservation check。原工作区 branch/HEAD、46 个原始文件 hashes 保持不变；当前精确文件集包含单独登记的 `docs/CMAEG_GitHub_Source_Tech_Design.md`，该并发文档 hash 保持 `7cf6ae4c7ca8d76c8993df7efbbcc68c4a8ecbfc09454771c5069254d7d8d0b7`，无未知新增文件，未纳入本任务或改写它。19 tickets / 149 原始 AC 文本逐字核对不变；13 仍 in-progress、一个 literal human-quality AC 未勾选。
- 16/17 仍在独立分支工作，未合并。原始 full-format、skill/Core fixtures、508-line service、LocalAgentV2 head-404 限制仍独立记录。仅写 integration source/ledger/private build artifacts，无原工作区或其他 worker 源码写入，无 push、PR、部署或共享运行环境操作；保留 worktree 供 parent cleanup。

## Ticket 15 E2E types repair 集成（2026-11-03）

- 合并前 integration 干净，分支 `codex/cma-ui-optimization`、HEAD `40b627b899f742ac7178a18622dd002fcb531254`，保留 parent 的 16/17 in-progress tracker 更新。已审阅 `E2E-TYPES-REPAIR.md` 和实际 diff；外部 `evidence/e2e-types-repair/manifest.json`、`files.sha256` 的两路径、source 字节数/SHA-256/git blob 以及 26 log/artifact hashes 全部核实。
- 普通 `--no-ff` merge 合入 `codex/cma-ui-e2e-types` 的 `f4c4b510b8f9b47eb671d6f6c4d5721ef02da14b`，merge commit 为 `c431f8466439c2e9191cbd9679daf4cfc436162a`。无冲突，合并树与候选完全一致，无额外源码修正。仅修改 browser-live-chain 的显式 runtime evidence assertions/类型收窄和相同 newest-receipt 查找；compiler/lib gate、原行为断言和 runtime feature 均未改。
- 合并态在 `web/` 执行 `npm run e2e:typecheck`、`npm run typecheck`、`./node_modules/.bin/eslint e2e/browser-live-chain.check.ts --max-warnings=0`，全部 exit 0。`git diff --check 40b627b899f742ac7178a18622dd002fcb531254 HEAD`、worktree/cached diff checks 通过。无新依赖安装、native/Chrome/VM/Mongo、全 suite 或广泛 gate 重跑，也不将类型检查当作新的 native pass。
- Web Style 和 19 ticket 原始 AC 内容未改（保留全部 149 条）；13 的 literal human quality signoff 仍 pending。16/17 feature 分支未合并，VM slot 和其 native 工作完全未触碰。
- 原工作区只读 preservation check 在 `2026-11-03T08:59:41.496Z` 返回 1：branch/HEAD 与 46 个原始文件 hashes 全部保持一致，19 ticket AC 检查也通过；但相对现有外部登记表发现 25 个额外 changed/untracked 路径，且已登记的 `docs/CMAEG_GitHub_Source_Tech_Design.md` hash 已变化。此为本次观察到的并发文件集漂移，已通知 parent；没有将失败称作完整 preservation pass，没有改写登记表、原文件或并发文档，没有把它们纳入本任务。
- 仅写 integration 的 merge/ledger；无 original/其他 worker source 写入，无 push、PR、部署、服务或 VM 操作。保留 worktree 供 parent cleanup；原有 baseline 限制不在本次修复范围。

## Ticket 17 完整浏览器控制交接（2026-11-03）

- 运行时在同一 mediator 队列仲裁 Agent navigation/input 与用户 lease/input/return。15秒 monotonic 租约、128条有界 receipt/audit、账号/credential/private-client-key/context/incarnation/token/revision 绑定；公开 GET/POST control、两套 typed SDK、真实 Web 点击/键入/返回与原操作恢复一并完成。没有新增 PTY、任意脚本/CDP 或服务授权。
- 原始八条 AC 原文保留并基于实际证据勾选。首个真实已知表单 red1→green1；父线程早期 P1 delayed-body 跨替换和 P2 无本地期限均通过实际回归关闭，Linux边界5/5。账户恢复及 acquisition/read 并发分别有 red/green；后者 focused3files/11tests。
- 独立 immutable17 runtime SHA6283b369b5bedcd8c8d742786f061cce45fbcbb3ead18a2241db473286f624e3，生产构建源556aef8ba，baked proof源23d92fdcc，最终 image4ebc67d07fef6c471b433df01723b277a2db51a432df47a9523a5f8e68d8ca37。精确 API source manifest 及仅测试采样差异证明保留。ACP保持84f967...原字节，修正0755/test-x。uid10001/caps-empty/NNP/read-only/network-none/2GiB/512pids，实际本次cpu.max100000/100000；不改变15旧观测结论。
- Actual runtime HTTP native1/1、公开handler1/1（0ignored）、Playwright1/1和native进程exit0。实际 USER-17→USER-1→USER-17→USER-17-ONCE→USER-17-ONCE-AGENT→USER-17-ONCE-AGENT-EXPIRED；两次native输入被拒绝，原页面一次加载，lost acquire/input/return、竞争client、旧token/incarnation/context、权限403、公开audit及零task输入均通过。1440/390/320截图实际查看，父线程独立review/hash保留；未把fixture背景连接错误包装成19完整产品成功。
- 合并16 feature626679227后仅Web Style append冲突，保留16前缀再追加17；再合并最终integration3aff828c1eec7e0ddb07969a0b5977955f1ec810（ledger-only）。Canonical shared filesystem_runtime 与browser control corpus均保留。Combined host Web338files/3018tests、TS SDK3754tests、types/fullE2Etypes/lint/build/SDK consumer和control37/browser36/environment10/desktop13公共case dispatch及OpenAPI inventory通过；更多精确命令/结果/commit绑定见外部handoff。
- 17:44已释放VM重槽，之后仅host合并/验证；Mongo、16证据、共享cache和daemon未改动。早期失败attempt完整保留。完整交付/安全边界/15和16 reader floor见17-EVIDENCE.md、17-CONTROL-CONTRACT.md；外部ticket17-proof/HANDOFF.md与manifest.json封存精确最终commit、changedpaths、逐文件SHA、各验证候选和证据。19整体发布、13人工验收、原baseline问题仍独立，不声明完成。

## 16 首页可选桌面创建完成（2026-11-03）

- 独立 worktree `/Users/liuyitian/code/cma-ui-worktrees/ticket-16`、分支 `codex/cma-ui-16`；起点9bc2f04，已合入最新 integration6722f9b（含质量修复和15 E2E类型修复）。完整实现/证据见 `16-EVIDENCE.md`；原始7条AC逐字保留，只在证实后勾选，未改17/19状态。
- Home默认关闭的桌面选项进入原请求/key/operation和durable first-message preparation；Core在admission前验证明确的immutable runtime、2GiB/1000m配置、retained home和existing boot/generation/active状态，不自动重启。暂时失败保留原Agent/task/browser，准备状态与task/真实Live区分；accepted replay不受之后关闭新admission影响。
- 首red是丢receipt刷新后缺桌面开关，随后U green；真实public+Mongo unsupported red→green、supported422→202、准备状态/retained draft red→green均保留。最终public/Mongo9tests证明首次任务恰好一次、process/worker重启、mutation-free refusal、原Workspace/runtime绑定；OAuth取消/刷新必须显式续同body/key/desktop。
- 实际native Home正式POST→丢202→刷新重放→同Agent/task→真实ACP/cma-browser/Chromium→Core/public/SDK/Web1280×720JPEG→刷新同context通过。native候选0437bd154；public driver1passed/0ignored，Chrome1passed。Agent655ms可进入、准备约4s、first Web Live31.083s；native全cgroup峰值910155776bytes/93PIDs/14.103CPU秒，具体quota/read-only/UID边界与外部fixture限制见证据。VM-heavy slot16:58明确释放，无daemon/VM重启。
- 真实启动发现15 proof image COPY丢ACP executable bit；新immutable image `sha256:da56a37266cd285f584ba4ecc3982102b20bbdaa7c0b5ba36842b5087e43fead`只恢复0755，runtime/bridge bytes不变。production Dockerfile保留`test -x`门禁；attempt01–04失败与最终绿均保留，未以截图或mock代替native链路。
- Full Web336files/3011tests、focused3files/32tests、Web/E2E/TS SDK types、Web/SDK lint、CSS、四层+CMA/RustSDK all-target Clippy通过；13新public corpus在真实handler回放，TS完整2852tests，Rust12typed HTTP+1closed-query shape case。inventory2847cases/239operations/1627HTTP named pairs/23protocol pairs；捕获的missing-task invalid_request已准确声明，无skip/allowance。
- 实际retained15可执行文件对新desktop journal读取503，旧omitted-field process-restart positive1passed；current process同Agent/operation恢复通过。rollback floor必须16-aware journal/first-message reader，且保留15 agent_browser-aware runtime decoder，不能仅关闭UI回滚。旧binary SHA和positive/negative日志已保留。
- 外部 `ticket16-proof/manifest.json` / `HANDOFF.md` 提供最终精确commit、每个changed path SHA、artifact/binary hashes、source-equivalence、line/format/style/AC/preservation核验。保留原工作区46hash及并行新增路径，未写原工作区、未派生agent、未push/PR/deploy；13人工质量与19整体验收仍独立。

## Ticket 16 independent integration verification (2026-11-03)

- Integration was clean on `codex/cma-ui-optimization` at `6722f9b0e5818d4337483804e896421a66da39a6`. Independently verified that clean worker candidate `f334234a0a5112af7cc496069a4f8fce61d8bf54` contains that exact tip. Normal `--no-ff` merge commit: `62667922736b7c54143fac91d98ae4718d6b395a`. No conflicts or integration source fixes; the merge tree equals the candidate tree exactly.
- Read the handoff, evidence, verifier and parent visual review without executing the manifest-rewriting verifier. Independently verified manifest SHA-256 `4f0529540c6e1fa9eeb1fc7b7292ee86b948161b59ce734cbaf474cb3757b562`, all 67 changed paths against Git and worker bytes, 311 artifact hashes, three executable hashes, and 77 recorded native-candidate source hashes. All 56 bounded source files satisfy their 500/2000-line limits; maximum production count is 500. The entire immediately preceding Web Style file is an unchanged prefix. All 149 original AC texts across 19 tickets remain verbatim, including ticket16's seven criteria; only completion state changes as documented.
- Reviewed shared Home/lifecycle/first-message/admission identity, public/typed SDK contracts, retained runtime fences, and the Docker executable guard. No added skip/only/ignore directives or lint/type suppressions were found. The optional native relay finish hook is not native proof by itself; the sealed native result, matching actual JPEG and successful public/Web logs remain the evidence. The unknown-query SDK case explicitly verifies its closed typed shape/error classification instead of claiming an impossible typed HTTP request.
- Merged-state Web command, from `web/`: `./node_modules/.bin/vitest run src/create/DesktopHome.test.tsx src/create/TaskHome.test.tsx src/create/AgentProvisioningProgress.test.tsx src/create/lifecycleIntent.test.tsx src/create/lifecycleTransport.test.tsx src/agent/BrowserPanel.test.tsx src/agent/OperationReplay.test.tsx src/ConsoleDeliverablePreview.test.tsx src/agent/SharedDeliverablePreview.test.tsx --configLoader=runner --no-cache --maxWorkers=2`. Actual result: 9 files / 64 tests passed, zero failed or skipped, covering frozen creation/recovery and browser/replay/preview preservation.
- Merged-state SDK command, from `sdk/typescript/`: `./node_modules/.bin/vitest run tests/public-contract.test.ts --testNamePattern=': desktop-(capability|create)-|requires every loaded fixture' --configLoader=runner --no-cache --maxWorkers=2`. Actual result: 14 passed (13 desktop cases plus typed-dispatch inventory); 2,838 unrelated cases excluded by the explicit name filter, not counted as executed proof. Web `npm run typecheck`, Web `npm run e2e:typecheck`, TypeScript SDK `npm run typecheck`, and Web `npm run check:css` all exit 0; CSS reports no collisions across 901 classes, three deliberately shared. Range, worktree and cached `git diff --check` pass. No full suite, Rust, native, Chrome, Mongo or VM workload was repeated.
- Retained compatibility evidence was inspected: the actual old15 binary fails the new desktop journal with 503; its legacy omission/process-restart positive has one passed/zero ignored, and the new reader recovers the recorded operation/Agent. Keep the 16-aware retained journal/first-message reader plus the 15-aware RuntimeStatus reader; disabling new desktop admission does not make an old binary rollback safe. Docker `test -x /usr/local/bin/codex-acp` remains present; recorded runtime SHA `364d71a3424476e5c7b27583b0df39cce1751e0d92be06347baa62a1e6dbf531` and bridge SHA `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c` remain unchanged. Production runtime source equivalence and mounted native-script bytes were independently checked.
- Read-only preservation checks retain the original branch/HEAD and all 46 protected hashes. The check at `2026-11-03T09:48:11.502Z` exits 1 because the existing external registry does not describe ongoing concurrent additions: 29 additional changed/untracked paths beyond the original46 and recognized concurrent document, whose hash also differs from the registry. An earlier check during this merge observed 27 such paths. These are time-specific observations, not an entire-file-set preservation pass; no original file, concurrent addition or parent registry was edited or reverted.
- Parent visual limits remain explicit: optional access404/feedback/SSE fixture notices are visible in retained screenshots; those screenshots prove bounded creation/browser observation, not complete conversation/feedback/replay composition or mobile answer/composer preservation. Ticket19 owns integrated follow-up. Ticket13 literal human-quality signoff remains pending; ticket17 is unmerged and owns the VM slot. Original unrelated skill/Core fixtures, full-format drift, 508-line service and LocalAgentV2 head404 remain separate. Only integration merge/ledger and the expressly requested external `merger-STATUS.md` were written; no other worker source, original source, orchestration manifest, push, PR or deployment changes.

## Ticket 17 independent integration verification (2026-11-03)

- Initial preflight was read-only while completion and the final seal were pending. Parent explicitly released the exact candidate before any integration mutation. Clean `codex/cma-ui-optimization` at `3aff828c1eec7e0ddb07969a0b5977955f1ec810` was an ancestor of clean worker final `7b08305d6ade26a1aeccbd8c3336838ca03b622c`. Normal `--no-ff` merge commit: `cf5e48bfa73cee46201a523020efb0359279bc03`. No conflicts or integration source fixes; merge tree equals the candidate exactly.
- Independently verified final manifest SHA-256 `753acf73b7b60c6be9667a50f36970283d27c2630a0a9825fc86cabe95dc4036`, its complete/final/base fields, `changedpaths.txt`, `files.sha256`, all 63 changed source paths/SHA-256/git blobs, 258 artifacts and two explicitly post-merge host test binaries. Both immutable source archives verify against their manifests and Git: 3,886 files each, 7,772 total. Historical parent review and exact desktop/320px screenshot hashes also verify. The optional `verify-handoff.mjs` is absent; independent verification did not require or generate it. All 149 original criteria remain verbatim, including ticket17's eight checked criteria. The entire preceding Web Style prefix is unchanged; 53 bounded source files pass, maximum production count 489. No new skip/only/ignore or lint/type suppression directives were found.
- Runtime identities remain distinct: production build `556aef8ba35bbcba8cb60e1eb122cde36eda7f7f`; baked proof `23d92fdcc6fdd430e5e8ed5a1f6c42a8b27ee294`; pre-merge host validation `630aba9e0c219dce64f33191770dba97162927e0`; combined host `60fbd952cc98eae6cb7e92547ae102fbae06f278` and ledger merge `d89945b6bfdf64545fa83389e53d357f3dafee7b`; final7b adds only evidence/ledger. Git confirms556→23d changes only native test-frame sampling,23d→630 API is identical, and23d→final changes only ticket16's separate `desktop-chain.cjs` proof driver in Layer3/4/Cargo. Final immutable image is `sha256:4ebc67d07fef6c471b433df01723b277a2db51a432df47a9523a5f8e68d8ca37`, runtime SHA `6283b369b5bedcd8c8d742786f061cce45fbcbb3ead18a2241db473286f624e3`, same ACP SHA `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c` with recorded0755/test-x. No later host binary is attributed to the earlier native invocation.
- Retained logs independently show Linux boundaries5 passed/zero skipped, fresh native HTTP1 passed/zero skipped, public1 named passed/zero ignored, and Playwright1 passed. P1 delayed native request replacement and P2 monotonic expiry are closed in current source and actual regression evidence. The retained full chain records same-page actual input, lost acquire/input/return recovery, competing identities, expiry, stale identities and zero conversation task inputs. These remain worker/native evidence; no VM, native, Chrome, Mongo, Rust or full-suite rerun was performed by this merger.
- Merged-state Web command from `web/`: `./node_modules/.bin/vitest run src/agent/BrowserControl.test.tsx src/agent/BrowserControlRecovery.test.tsx src/agent/BrowserPanel.test.tsx src/agent/OperationReplay.test.tsx src/agent/SharedDeliverablePreview.test.tsx src/ConsoleDeliverablePreview.test.tsx src/create/DesktopHome.test.tsx src/create/TaskHome.test.tsx src/create/AgentProvisioningProgress.test.tsx src/create/lifecycleIntent.test.tsx src/create/lifecycleTransport.test.tsx --configLoader=runner --no-cache --maxWorkers=2`. Actual result: 11 files / 71 passed, zero failed or skipped.
- Merged-state TypeScript SDK runs use the same runner/no-cache/two-worker flags: `vitest run tests/agent-browser-control.test.ts` passes1; `vitest run tests/public-contract.test.ts --testNamePattern=': browser-control-|: desktop-(capability|create)-|requires every loaded fixture'` passes51 (37control+13desktop+one typed-dispatch inventory), with2,838 unrelated cases excluded by the explicit filter. Web `npm run typecheck`, `npm run e2e:typecheck`, SDK `npm run typecheck`, and Web `npm run check:css` all exit0. CSS:904 classes, three intentionally shared, no collisions. Range/worktree/cached `git diff --check` pass.
- Original-worktree read-only preservation at `2026-11-03T10:04:55.326Z` confirms original branch/HEAD and all46 protected hashes unchanged. Checker exits1 for34 additional unregistered concurrent paths and the recognized concurrent document's changed hash; this is not a whole-file-set preservation pass. No original/concurrent path or parent registry was changed or reverted. The known508-line Agent operation service hash remains exactly `fdf19898a69c9f918a872372a2b99762d2c1fc4d40dbb1d77925f5ccad81ccdc`; original skill/Core/full-format/LocalAgentV2 limits remain separate.
- Keep15-aware RuntimeStatus and16-aware retained journal/first-message readers;17 adds no durable Mongo schema and runtime restart cannot promise the same browser context. Parent visual review's background fixture live-error/empty-state and mobile-overlay limits remain explicit;19 owns composed successful-path/restart/rollback acceptance. No production/GKE/IdP/Mongo availability, unrestricted same-uid isolation or screen-reader/44px certification claim. Ticket13 literal human-quality signoff remains pending. Only integration merge/ledger and the expressly requested external `merger-STATUS.md` were written; no other source/orchestration writes, push, PR, deployment or VM work. Integration is handed off for19; parent owns cleanup.

## Ticket 19 rollout component independent integration (2026-11-03)

- This records component integration only: ticket19 and ticket13 remain incomplete. Clean integration `b6993f69711c1b625e6de57a06b7525668d76810` is an ancestor of clean rollout candidate `e33dd0323bc224420ab3ead81e83ac4ac69038a1`. Normal `--no-ff` merge commit: `e9d7c7b0588c805ff66c2c06589c9a50e6fd15a1`. No conflicts or integration source fixes; merge tree equals candidate exactly. No `task_home_tests*`, primary19 E2E/native-driver or original ticket-status changes were made.
- Independently verified the final `ticket19-rollout-proof/SEAL.json` SHA-256 `ce24a7ca73cd73880029ac5bbbfe8cce4b34dda553e174ff955eec2c50e7e678`, all seven linked manifest/document hashes,86 source paths/bytes/SHA-256 against Git and worker,192 evidence hashes and40 executed host-binary hashes/sizes (1,142,975,912bytes). A preliminary stale AgentSidePanel line count was corrected by the helper before its final seal; the final406 count and all75 bounded files verify, maximum production count500. The whole previous Web Style prefix remains unchanged. All149 original AC texts and all ticket statuses remain unchanged; no new test skips or lint/type suppressions were added.
- Reviewed policy/admission/retained-recovery and readiness boundaries. Strict `CHRONO_TASK_EXPERIENCE_POLICY` defaults off; authenticated `GET /api/v2/ui/task-experience` returns only the current account's eligibility. Primary19 must pass the same explicit policy Arc through `AgentV2State::with_task_experience`, `AgentCommandService::with_task_experience`, `AgentFileService::with_task_experience` and `InlineAgentLifecycle::with_task_experience` before use. Its fixture must explicitly enable the required five capabilities for the actual subject. Existing authorization/environment fences, accepted recovery, manual input, browser return and draft restoration remain separate from new admission. The component's policy documentation and handoff retain the exact contracts.
- Merged-state Web command from `web/`: `./node_modules/.bin/vitest run src/agent/RuntimeReadiness.test.tsx src/agent/BrowserPanel.test.tsx src/agent/OperationReplay.test.tsx src/agent/BrowserControl.test.tsx src/agent/BrowserControlRecovery.test.tsx src/create/TaskRollout.test.tsx src/TaskExperienceIdentity.test.tsx src/create/TaskHome.test.tsx src/create/DesktopHome.test.tsx src/agent/AgentSurface.test.tsx src/AppCreation.test.tsx src/AppPublic.test.tsx --configLoader=runner --no-cache --maxWorkers=2`. Actual result:12files/87passed, zero failed/skipped.
- Merged-state SDK checks use the same runner/no-cache/two-worker flags. `vitest run tests/task-experience.test.ts tests/agent-browser-control.test.ts` passes2. Selected shared-contract command `vitest run tests/public-contract.test.ts --testNamePattern=': task-experience-|: browser-control-|: desktop-(capability|create)-|requires every loaded fixture'` passes51 (37control+13desktop+inventory;2,844 excluded by filter). Policy cases have different ids and were separately executed using `--testNamePattern='GET /api/v2/ui/task-experience'`:6passed,2,889 excluded. Total executed focused SDK checks59; exclusions are not executed proofs. Web/E2E/SDK typechecks, CSS904classes/three deliberately shared and range/worktree/cached diff checks pass. No full rebuild, Rust/native/VM/Mongo/full-suite or history scan was repeated.
- Retained worker checks are distinct overlapping suites: Web3036/341files, TS SDK3761 plus one existing opt-in skip, Rust SDK210named+34doctests with two existing external-fixture ignores; public policy5/config2 named tests, policy6/control37 real-handler corpus cases, and240-operation inventory. Full Core has1152passes plus the original skill-version fixture failure; full public corpus retains the original skill-version expected-code mismatch. Zero-test selector and failed attempts remain nonproof. The508-line service and original unrelated full-format/LocalAgentV2 limits remain separate.
- Attribution correction: missing `CHRONO_DESKTOP_VERIFIED_RUNTIME_IMAGE` documentation is a task16-introduced integration defect (e2e14b9cf/f334234a0), absent at originalc9. The helper's exactb699 reproduction proves only that it predates this rollout component. It is not an original-baseline exemption. Primary19 owns the actual docs/CMA_Operations check repair; historical failure logs remain and the final release gate stays open. Six synthetic UUID fixture replacements preserve runtime-reply/corpus parity; current-source green does not clear full history. Parent owns history sanitization; no actual refs/history were rewritten here.
- Read-only original preservation at `2026-11-03T11:39:15.607Z` retains original branch/HEAD, matches45of46 initial hashes, and reports the original public-contract inventory drift plus38 additional unregistered paths and changed concurrent-document hash. Checker exits1, not a complete preservation pass. Parent independently attributes the inventory delta to the concurrent lookup-route/count change, not task policy; this merger neither restores nor attributes authorship. No original or other-worker source was written.
- After all40 executed binaries verified, the final handoff releases helper ownership. Parent may remove only `ticket19-rollout-proof/target-api/debug/incremental` and `ticket19-rollout-proof/target-sdk/debug/incremental`; retain all manifested binaries, other debug/deps, source and evidence. This merger performs no cleanup and never touches active19 targets/VM/caches. Primary19 owns policy wiring and actual composed readiness/native/retained-reader/release validation. Only integration merge/ledger and requested merger status were written; no push, PR, deployment or whole-ticket acceptance.

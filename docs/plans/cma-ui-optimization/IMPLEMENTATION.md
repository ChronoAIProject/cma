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

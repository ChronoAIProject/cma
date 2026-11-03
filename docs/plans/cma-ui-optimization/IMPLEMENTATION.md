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
- 09 已验收合入，完成 worker 的 worktree 已清理；06 已从含 09 的最新集成状态启动。当前实施：01、03、06、07、11、14；其余票依赖满足后排入可用执行槽。

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

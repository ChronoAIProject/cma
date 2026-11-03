# 07：在回答下方展示可验证的成果并下载

集成修复交接：本票列出的八项旧 Web 断言已由
[INTEGRATION-REPAIRS](../INTEGRATION-REPAIRS.md) 修复，合并态完整 suite 通过。
另有独立新缺项：四条已公开的 delivery 操作尚未进入共享 JSON inventory/corpus
及其 typed SDK dispatch；修复分支保留全部路由，未用调整计数掩盖缺项。

**What to build:** 用户看到本次回答实际交付的文件或持久文档，直接下载，且不会把别的任务或未发布文件当作本次成果。

**Blocked by:** None（使用现有会话和资源入口）。

**Status:** done

**优先级:** P1

## Acceptance criteria

- [x] 给真实最终回答建立稳定资源关联，包含 Agent/turn/回答身份、资源类型、资源身份和版本证据；关联在重放和刷新后保持一致。
- [x] 复用已批准 Agent artifacts、持久文档和 owner 文件读取；将它们视为不同资源种类，不默认重复建库、不从当前目录全部文件或普通代码字体猜交付物。
- [x] 来源方案随本票明确并实施：使用实际发布 receipt 或经授权验证的显式文件/文档引用建立关联；没有通用写入追踪证据时，不声称按 turn 扫描写入记录即可实现。
- [x] 文件卡片在所属回答正文后显示名称、类型、实际大小/未知状态和下载入口；零成果不占空白区域，晚到关联不会打乱阅读位置。
- [x] 下载使用该资源自己的权限，字节数/digest 与可用版本元数据匹配；分享会话不自动授予文件访问，也不自动把私人文件发布成共享 artifact。
- [x] 对文件已变更/已删除、文档版本变化、结果尚未就绪、权限撤销和下载失败分别显示真实状态并可独立重试；不能把最新文件伪装成回答时快照。
- [x] 历史回答原有关联保留，新回答不能认领旧资源；同资源多次关联按稳定身份去重，故意多回答引用同一资源仍保持各自归属。
- [x] 不支持预览的类型仍可按既有下载能力交付；超过资源原有大小限制时明确说明，不通过卡片绕过限额。

## Testing boundaries（2026-11-03 已确认）

- **U/A：** 生成/登记关联→读取回答→卡片→授权下载；检验两个回答、多个 owner、刷新、删除和权限撤销，公开读回对应版本。不能证明每个 provider 都能产出结构化资源引用。
- **R：** 隔离 Agent 实际交付一个文件和一个持久文档，下载已知内容并核对对应回答；没有来源证明的资源不得在测试中硬塞关联冒充能力。
- **首个 red → green：** 两轮分别交付 A 和 B，查看第一轮仅有 A；刷新下载 A 得到已知内容，B 不串入。

## 范围与依赖说明

本票完成关联、卡片和下载；08 增加右侧预览。此票不开放新共享权限、不实现所有文件格式的编辑器。

## 实施决策与证据（2026-11-03）

- 使用已确认的 U/A/R seam，按单行为 red → green 推进。首个 A 行为通过真实发布接口取得 A/B 的不可变资源，再通过公开验证接口生成显式引用，分别进入两个最终回答；重建服务后读取第一回答并下载 A。
- 来源是经授权验证的显式结构化引用。引用随现有不可变回答正文保存，不新建文件库，不扫描目录或猜测普通代码字体。公开查询派生 Agent、turn、`event:<sequence>` 回答身份并再次检查资源授权；运行时工具生成同一引用。独立文档 revision、批准 artifact identity、实时文件 path + digest 保持不同语义。
- 只在真实已完成 turn 的最后一条 Agent 回答展示。私有文件引用不发布 artifact；会话访问不代替文件或文档权限。实时文件发生变化时禁止下载替代字节，文档下载保留原 source revision。
- UI 使用独立组件；与 09 的共享接点仅为 `StreamEntry` 的组件插入及必要属性，避免改反馈 footer。08 预览不在本票。
- Parent 性能审查已完成：已移除初版的完整 transcript fold / 每卡片两次历史重建 / 列表读取全部文件字节。使用有界的单回答投影及独立授权复核；已发布 immutable 元数据用于列表，实时文件只在下载或单卡 Retry 时验字节。前端仅为可见且带显式引用的回答查询，不对全历史逐回答预取。
- Rust 使用固定 1.96.1、`CARGO_BUILD_JOBS=2`、`RUSTFLAGS='--cfg cma_ui_ticket07'`。已从共享缓存 APFS-clone 到私有 `/Users/liuyitian/code/cma-ui-worktrees/cargo-target-ticket07`，仅在私有目录清理 layer2-core/layer1-public/cma 后重编译；不再等待共享 target 或复用其他工作树 binary。使用隔离 Docker context `colima-cma-ui-proof` 运行 Mongo 验证；固定 Codex 0.147.0 / ACP 1.4.0 八步补丁 bridge 的实际 native 证明见下方，普通 fixture 不算 R 证据。
- 已 fast-forward 合入 ticket09 后的 integration `6a606ebf0`，随后继续合并 integration 至 `d19c225bd`。有界读取复用 `AgentTranscriptTurn::final_answer()`，前端复用 `ConversationMessage.finalAnswer`，保留 09 footer 的 eligibility 共享 hunk。14 的 protected-path 文件检查及 computer props 在合并时一并保留。
- 单回答读取只查目标记录、至 terminal 的最多 128 条后续记录，以及最多 512 条记录的所属 turn；整次投影最多 4 MiB / 15 秒。区间缺失、额外 turn/conversation 边界、非 completed 或非最大非空 Assistant 消息都 fail closed。超出此范围明确返回 503，不退回完整历史 fold；授权复核不重读日志。API/SDK 文档已说明此界限。

### 当前 red → green 记录

- `layer1-public answer_deliverables_keep_two_published_results_separate_after_reconnect`：缺少关联服务/公开路由时编译失败；实现后通过。两个明确发布的不可变 artifact 分别进入两个已完成回答，重建服务后第一回答仍只下载 `Answer A\n`。
- `layer1-public answer_deliverables_download_the_original_document_revision_after_an_edit`：缺少文档接入时失败；实现后与第一项一起通过（2 passed）。文档改为第二 revision 后卡片报告 `version_changed`，下载仍为第一 revision 的 `# Original\n`。
- `layer1-public answer_deliverables_require_independent_document_access_and_honor_revocation`：初始 `unavailable` 与期望 `denied` 不符；明确独立权限状态后通过（1 passed）。分享会话不授予私人文档读取；单独授予文档 read 后可下载，撤销后再次 404；另一 owner 的个人读取 404。测试的组织 authority fixture 对所有成员允许，所以另一 owner 验证使用个人身份，不冒充未加入组织成员。
- 初期 L1 运行使用未提交的 baseline 编译补正：原 `instance_problem.rs:207` 的 `SkillVersionUpgradeUnsupported` 分支返回 `String`，其他 tuple 分支返回 `&str`，触发 `E0308`。该临时补正已删除；现在已合入 integration `60fc98ee5` 的独立两行 typing correction，不混入成果实现提交。
- Parent 独立在 feature code 之前运行 `cargo check --locked -p layer1-public` 同样 exit 101；原始证据 `/Users/liuyitian/code/cma-ui-worktrees/baseline-public-build.log`。后续测试使用 `RUSTFLAGS='--cfg cma_ui_ticket07'` 分离共享 target 内的 worktree fingerprints；错误复用其他工作树 binary 的 `0 tests` 运行不计入通过证据。
- `layer2-core answer_deliverables_live_file`：缺少 live-file 接入时 E0599；实现后真实运行 1 test 通过。通过外部 filesystem port 读取本地真实文件，原字节为 `original`；覆写后拒绝替代下载，删除后报告 missing，artifact registry 保持为空。随后增加原限额与 runtime 状态验证，`unavailable` 对 `too_large` 的断言先失败；映射后通过（1 passed），暂停时报告 `not_ready`。这是 U/A filesystem-boundary 证据，不算 native Agent 生成来源的 R 证据。
- 性能 red：私有 target 的 `answer_deliverables_live_file` 实际运行 1 test，10,006 event 历史导致“one delivery lookup must not reconstruct a 10,006-event conversation”失败。改为有界单 turn 投影后通过，DB 边界观察该旧回答查询不超过 128 行。默认 debug 测试线程发生 stack overflow；box 部分嵌套 future 后仍重现，使用 `RUST_MIN_STACK=33554432` 的明确环境才通过，不宣称默认栈通过。
- 列表懒读 red：期望 live-file `unchecked`，实际 `ready`，1 failed；将列表改为独立文件权限与 runtime/path 检查、Retry 只验单项后 1 passed。外部 filesystem 计数断言列表零 body reads；真正的下载仍验证原始字节，修改/删除/超限在单项 inspect 中明确报告。
- Immutable 元数据 red：`answer_deliverables_list_immutable_metadata_without_reading_bodies` 检测 body reads 从 2 增至 3，1 failed。新增经过 publication audit/identity/当前权限校验的 metadata-only store projection，内存和 Mongo 均不读取 body，现核心 deliverable 两项真实运行 2 passed；下载仍读取并核对 `abc`。文档列表只读独立授权 revision 元数据，不拉取 source blob。
- 描述符验证 red：非法路径、展示名、媒体类型及非 SHA-256 字符串被当作普通 unavailable 项返回；添加有界 resource-specific 验证后 `layer2-core answer_deliverables` 实际运行 3 passed。
- MCP 文档 receipt red：`publishes_large_private_document_replays_and_updates_with_cas` 的 `structuredContent.deliverable` 是 null，1 failed。通过独立 runtime author fence 读取真正保存的 source revision 元数据后 1 passed，幂等 replay 保留原 marker。
- MCP descriptor red：缺少新 router（并修正测试 fixture 的 crate 内访问），编译失败；`confirmed_runtime_describes_only_its_authorized_deliverable_and_loses_access_after_revocation` 后续实际运行 1 passed。它使用真实发布 registry 的 `abc`，拒绝任意 document selector，确认 capability 撤销后 401，未提交任何 consent proposal。
- 单项公开读取 red：新路径返回空 404；加入默认 metadata-only 读取及 `verify=true` 单项检查后公开 deliverable 三项实际运行 3 passed，包含重建服务后 A/B 归属不串、旧文档 revision、独立文档授权/撤销。
- OpenAPI red：缺少 `POST /api/v1/agents/{id}/deliverable-references`。注册 schema/4 条公开路由/Agent Manager allowlist 后相关 8 tests passed（包括实际 router 匿名探测及无悬空 schema refs），聚焦契约保留无隐式 artifact publication 的边界。
- `cargo check --locked -p cma --manifest-path api/Cargo.toml` 在私有 target/独立 cfg 下通过，已接入真实 composition、Mongo metadata projection、公开路由和 runtime MCP。一次局部格式处理误删重新排序的 module declarations 导致 E0432；已恢复并审计所有相关 module/export/test declarations，随后完整 composition check exit 0。未重排或格式化无关 baseline 文件。
- 后端 checkpoint 复核（固定 PATH、2 jobs、私有 target、独立 cfg、`RUST_MIN_STACK=33554432`）：`cargo test --locked -p layer2-core answer_deliverables` **3 passed**；`-p layer1-public deliverable` **5 passed**；`-p layer1-public mcp::agent_access::tests::document_tests` **2 passed**。全部来自当前 ticket07 的重新编译且明确非零测试数；仍未将 SDK/UI/浏览器/R 证明标记完成。
- 后端 checkpoint commit：`13e6f72b9`。TypeScript SDK 首个 red 为缺少 `answerDeliverables`；实现选择的回答版本、实际字节 SHA-256 与读取后授权复核后通过。descriptor red 为缺少 `describeDeliverable`；实现后 2 passed；非法 selector 在传输前拒绝的 red 为 ProtocolError 而非 ValidationError，实现后 3 passed。再补充损坏字节、超限、非 opaque 响应、下载期间撤销、取消及跨回答/重复项回归，实际 **9 passed**，typecheck 和 eslint 通过。
- SDK 运行环境：原本本票 `sdk/typescript/node_modules` 只有 production dependency symlinks，缺少 Vitest/.bin，初次命令未启动测试，不计代码失败。现从已准备的 web packages 建立本票独立 ignored package links；web 的 node_modules 顶层也改为本票实际目录，缓存留在本票内，不修改原始工作树的包目录。
- SDK checkpoint：TypeScript 全套 **68 files / 3566 tests passed**，typecheck、eslint、build、package:check（独立 npm consumer）及 docs:check（11 文档、49 编译示例）通过；path 1025 bytes 的 red 促使 TypeScript/MCP schema 与既有 1024-byte 文件边界一致，随后本票 9 tests 再次通过。Rust 各个缺少 list/download/describe 方法的编译 red 后依次 1/2/3 passed；损坏字节、响应混淆、非 opaque、超限和撤销回归后 **4 named deliverable tests passed**。Rust 全 suite（含 REST 42 tests、34 doctests）及 `cargo clippy --all-targets -- -D warnings` 通过；私有 target + 本票 cfg 保持独立。
- UI checkpoint：新组件首个 red 为找不到 Download first.txt；实现真实 SDK HTTP 边界读回后 1 passed。保存字节 red 为没有触发浏览器下载，接入 SDK 的 size/hash/读取后授权验证后 2 passed。单卡 Retry 缺失的 red 后状态回归通过；可见性 red 为更换 viewer 后对已不可见回答多读一次，改为持续可见性观察和作用域缓存后通过。6 个交付组件测试加历史渲染性能、09 footer、owner/shared views、REST adapter 共 **8 files / 81 tests passed**；typecheck、eslint、E2E typecheck 通过。
- Chrome UI 证据：`web/e2e/answer-deliverables.check.ts` 首次真实页面 red 为卡片未接入 owner surface。接入后 1440/390px、light/dark 的实际下载 `abc`/文件名/刷新 A/B 归属均通过；80-turn 历史只请求当前可见带引用回答。晚到元数据曾令阅读位置移动 **108px**，局部保留随后可见消息锚点后同测试保持 ≤2px；最终 **5 browser tests passed**。截图在本机 `/tmp/cma-ticket07-evidence/`，已目检 desktop/mobile dark，保留实现前截图；浏览器测试的网络边界 fixture 不单独冒充 R。
- UI shared hunks：`StreamEntry` 只替换最终回答的显示/复制正文并插入独立 `AnswerDeliverables`，09 的 `canVote` eligibility 原样保留；`MessageStream` 新增稳定 resource context，未改 timeline/fold/跟随底部逻辑。`AgentSurface`、`SharedAgent`、`Console` 传递 viewer/client/current authority 并在权限不确定时撤掉 scope；08 preview 尚未实现。
- 后续边界：超过单 turn 读取限额的真实最终回答原返回 404；公开测试 red 期望 503，改为明确 bounded-read unavailable 后 1 passed。流式 marker 的 red 证明半截协议文字会进入正文；在预先补充 Web_Style 后隐藏保留前缀的未完成 marker，仍不提前建卡，交付组件和长历史性能共 **9 tests passed**。
- 本轮完整 web suite 初次 **317 files / 2843 tests passed，1 个 EventGateway.flows 5 秒超时**；该未修改文件单独重跑 **4 passed**。保留真实日志 `/Users/liuyitian/code/cma-ui-worktrees/ticket07-web-tests.log`，不把该次整体退出 1 报成通过。
- R 证明现已真实执行：`native_answer_deliverables_prove_real_file_document_receipts_and_public_downloads` **1 passed**，其启动的 Chrome `NATIVE receipts reach answer cards and authenticated public downloads without invented associations` **1 passed**。macOS arm64 Codex **0.147.0**、ACP **1.4.0** 的精确八步 patch bridge（SHA-256 `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c`），使用无生产凭据的 loopback model。实际原生 `exec_command` 写 `report.txt`，原生 MCP 调用候选 `describe_deliverable` 和 `publish_document`；最终回答仅复制真正工具 receipt 中的引用，未硬塞资源关联。
- R 下载独立已知值：文件 `Native answer file\n`，**19 bytes**，SHA-256 `f6c042409c945c141af65759a07055f9589a486941a285d38d361db0bf3f8400`；持久文档 `# Native document\n\nKnown persistent content.\n`，**45 bytes**，SHA-256 `6f09a58c411f877c6926b5f4d711df6fb6c8229335bd77d3de54b95021130e92`。捕获实际 ACP 回答作为同一 native conversation 的 durable `event:5` / turn 3；真实公开列表/下载、重建 router、Chrome 实际文件下载和页面刷新均保持该关联。截图 `/tmp/cma-ticket07-evidence/native-api-cards.png`。候选 MCP/REST handlers 在显式 **2 MiB Tokio worker stack** 下执行并通过；大聚合 Rust 测试仍按记录使用 32 MiB 测试线程栈。
- R 边界限制：控制模型、认证/账户导航边界、测试数据库及 path-confined 文件适配器受控；文件确由原生进程写入磁盘，文档由真实候选 DocumentService 持久化，浏览器交付列表和下载代理至真实候选公开 API。此证明不声称生产 OAuth、部署、Linux runtime image packaging 等价或 live model 质量；没有运行生产资源，也未把 baseline layer4 binary 说成含本票改动。初次 native fixture 缺少完整 Responses 文本流事件，补齐 `output_text.delta` 后 ACP 才实际发出正文；初次浏览器证明因 fixture 固定 Agent ID 失败，改为当前测试 Agent 的导航/路由身份后通过。
- Mongo 元数据补充证明：仅本票容器 `cma-ticket07-mongo`，`colima-cma-ui-proof`，Mongo 8.0 replica set，loopback 27407，0.5 CPU / 512 MiB；`artifact_publication_replays_across_replicas_and_revocation_preserves_retained_bytes` **1 passed**。新增断言通过真实 AgentArtifactService 读回 Mongo 投影元数据，重开 store 后 owner 仍可读取，撤销的 recipient 不可读取。每次测试使用独立数据库并删除；无其他票或生产数据库写入。

下载时的状态也经 U seam red → green：下载后授权复核被撤销时原先只显示通用失败，现由 SDK 的 `DeliverableUnavailable.status` 保留具体状态，页面显示 Access unavailable 且不保存字节；文件真实下载路由返回 `deliverable_changed` 时原先丢失原因，现显示 File changed since this answer，单卡 Retry 仍可用。交付组件和原有历史性能共 **11 tests passed**。

### 可复现 R 命令

先在本 worktree `web/` 启动 `npm run dev -- --host 127.0.0.1 --port 4537 --strictPort`，再从本 worktree 根执行：

```sh
PATH=/Users/liuyitian/.cargo/bin:$PATH \
CARGO_BUILD_JOBS=2 \
CARGO_TARGET_DIR=/Users/liuyitian/code/cma-ui-worktrees/cargo-target-ticket07 \
RUSTFLAGS='--cfg cma_ui_ticket07' RUST_MIN_STACK=33554432 \
CMA_CODEX_TEST_BINARY=/Users/liuyitian/.npm/_npx/abfeab83ffa1581c/node_modules/@openai/codex/bin/codex.js \
CMA_ACP_TEST_BRIDGE=/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/bridge.mjs \
CMA_NATIVE_WEB_URL=http://127.0.0.1:4537 \
CMA_E2E_BROWSER_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
cargo test --locked --manifest-path api/Cargo.toml -p layer1-public \
  native_answer_deliverables -- --ignored --nocapture
```

### 合并后验证与交接（2026-11-03）

- integration 已合入 `d19c225bd`，包含 09 的最终回答身份及 14 的文件 scope。冲突保留两边文档追加段落、`StreamEntry` 的 `finalAnswer`/反馈 eligibility、`SharedAgent` 的 viewer 与 computer props，以及 runtime 文件请求新增的 Agent/path policy 参数。没有修改原始或 integration worktree。
- 最终公开边界：`layer1-public deliverable` **7 passed / 1 native ignored**；native 另行显式执行，不把 ignore 算通过。`rest::openapi` **19 passed**，MCP document **2 passed**，core deliverables **3 passed**。公开用例覆盖重复引用去重、跨回答有意复用、普通文件名/代码示例/中间回答/失败或未完成 turn 不生成交付。合并后的文件 scope 组合运行有一个 429；该命名用例单独实际运行 **1 passed**，其他两个 scope/environment 用例通过，未修改上游的限流实现或测试。
- 最终 Web：typecheck、eslint、E2E typecheck 均通过；交付及长历史行为包含在完整 suite 内，**324 files / 2931 tests passed，3 files / 8 tests failed**。8 个失败均在未修改 integration `d19c225bd` 的独立 archive 中重现（3 files，8 failed / 55 passed）：`AppPublic` 六个 task landing 断言、REST event URL 断言、TaskHome vocabulary 断言。完整日志 `/Users/liuyitian/code/cma-ui-worktrees/ticket07-web-final2.log`；基线复现 `/Users/liuyitian/code/cma-ui-worktrees/ticket07-upstream-failures.log`。未修复这些无关问题，也未把整套 Web 报成通过。
- 最终 TypeScript SDK：**68 files / 3572 tests passed**，typecheck、eslint、build、docs:check（11 文档 / 49 示例）、package:check（独立 consumer）通过。第一次 docs:check 读取合并前的旧 `dist` 导致 `initial_message` 类型失败；按脚本要求先 build 后通过，不修改该上游契约。Rust SDK 全 suite **240 passed**（含 REST 42 / delivery 4 / doctest 34），两个既有外部 CMAEG/Responses fixture 测试保持 ignored；clippy `--all-targets -- -D warnings` 通过。日志 `/Users/liuyitian/code/cma-ui-worktrees/ticket07-rust-sdk-final.log`。
- Chrome 页面 **5 passed**：1440/390px × light/dark 的真实下载、刷新，80-turn 可见性及晚到元数据 ≤2px 锚点。native proof 的视觉检查发现连接检查 fixture 的 owner 与认证 owner 不一致；新增断言实际 red（1 个 account-changed 提示），仅改外部连接 fixture 为同一 owner 后通过。保留 `/tmp/cma-ticket07-evidence/` 下前后对比与 native screenshot，最终 native 页面已目检。
- 新增 Rust 文件按所属 edition（API 2021、SDK 2024）rustfmt 检查通过，新增 Web/TS 文件 Prettier 通过，`git diff --check` 通过。未运行全库格式化，原有 `cargo fmt --all --check` 问题仍以 `/Users/liuyitian/code/cma-ui-worktrees/baseline-rust-format.log` 为准。合并后的 composition/OpenAPI 因本票增量越过 500 行，已将 delivery wiring/OpenAPI fragment 移入对应模块；文件只读检查也独立为子模块。全库 file-size gate 仍有三个上游已存在的超限文件：`agent_filesystem.rs` 上游 515 / 本票仅模块声明后 517，`agent_operation_service.rs` 508，`conversation.ts` 503；没有改写其 baseline 逻辑或降低门槛。日志 `/Users/liuyitian/code/cma-ui-worktrees/ticket07-file-size-final.log`。

08 可复用 SDK 的 `answerDeliverables`、`readAnswerDeliverable` 和 `downloadAnswerDeliverable`，保持 Agent + answer + delivery identity 与当前资源授权。`resource.kind` 的 `agent_artifact`、`document`、`live_file` 不能相互替换；文档使用精确 revision，实时文件需匹配回答时 digest，不能展示成快照。预览不在本票，不能直接使用最新路径绕过这些验证。UI 接入保持在独立 `AnswerDeliverables` 组件及 `AnswerDeliveryContext`；共享 hunks 为 `StreamEntry` 的正文/卡片插入、`MessageStream` context、owner/shared surface 的 viewer/authority props。

最终候选代码 `3f43c694f` 再次运行 Mongo replay **1 passed**、native R **1 passed** 及其 Chrome **1 passed**；实际 private L1 测试 executable 为 `cargo-target-ticket07/debug/deps/layer1_public-e79a961bfbedbe18`，SHA-256 `a7662f0a439d66edd61ef26371339016533a4b17c17800c365cf2a16b7b49c08`。最终日志为 `/Users/liuyitian/code/cma-ui-worktrees/ticket07-mongo-final.log`、`/Users/liuyitian/code/cma-ui-worktrees/ticket07-native-final.log`。完成前再次 merge integration，结果 Already up to date (`d19c225bd`)。以上通过证据均为命名用例的非零实际执行数；保留前述 macOS/受控边界、基线全套检查失败及 08 scope 限制，不声称生产或 Linux image 验证。

逻辑提交：`13e6f72b9` core/公开接口及来源；`55174483f` SDK；`8e6183a22` UI；`efe8163ff` 有界读取不可用状态；`fcae3ba5e` 流式 marker 隐藏；`39b4faee8` native 来源到浏览器下载证明；`af1bccfee` 精确下载错误；`09b5fb54a` native 账户 fixture；`3f43c694f` 合并后的模块边界。integration merge commits 为 `b575ad1f4`、`22491bb4f`、`777825163`。

主要文件入口：

| 范围 | 文件 |
| --- | --- |
| 关联、资源验证、有界回答 | `api/crates/layer2-core/src/sandbox/answer_deliverables.rs`、`sandbox/answer_deliverables/{types,runtime}.rs`、`sandbox/agent_reads/answer.rs` |
| 元数据/文件授权 | `api/crates/cma/src/store/resource_access/artifacts.rs`、`api/crates/layer2-core/src/sandbox/agent_artifacts/{service,types}.rs`、`sandbox/agent_filesystem/read_check.rs` |
| 公开 REST/MCP | `api/crates/layer1-public/src/rest/answer_deliverables.rs`、`rest/answer_deliverable_shapes.rs`、`mcp/agent_access/{deliverables,documents}.rs` |
| SDK | `sdk/typescript/src/agents/deliverables.ts`、`sdk/rust/src/agents/deliverables.rs`，各 SDK exports/指南/测试 |
| UI | `web/src/agent/AnswerDeliverables.tsx`、`AnswerDeliverables.css`、`answerDeliveries.ts`，上述小范围 shared hunks |
| U/A/R | `api/crates/layer1-public/src/rest/answer_deliverables_tests.rs`、`rest/public_contract_tests/native_deliverables.rs`、`tests/native_answer_deliverables.cjs`、`web/src/agent/AnswerDeliverables.test.tsx`、`web/e2e/answer-deliverables*.{check,config}.ts` |
| 契约 | `docs/API_REST.md`、`docs/API_Agent_Manager.md`、`docs/API_MCP.md`、`docs/Web_Style.md`，本票验收记录 |

# 07：在回答下方展示可验证的成果并下载

**What to build:** 用户看到本次回答实际交付的文件或持久文档，直接下载，且不会把别的任务或未发布文件当作本次成果。

**Blocked by:** None（使用现有会话和资源入口）。

**Status:** in-progress

**优先级:** P1

## Acceptance criteria

- [ ] 给真实最终回答建立稳定资源关联，包含 Agent/turn/回答身份、资源类型、资源身份和版本证据；关联在重放和刷新后保持一致。
- [ ] 复用已批准 Agent artifacts、持久文档和 owner 文件读取；将它们视为不同资源种类，不默认重复建库、不从当前目录全部文件或普通代码字体猜交付物。
- [ ] 来源方案随本票明确并实施：使用实际发布 receipt 或经授权验证的显式文件/文档引用建立关联；没有通用写入追踪证据时，不声称按 turn 扫描写入记录即可实现。
- [ ] 文件卡片在所属回答正文后显示名称、类型、实际大小/未知状态和下载入口；零成果不占空白区域，晚到关联不会打乱阅读位置。
- [ ] 下载使用该资源自己的权限，字节数/digest 与可用版本元数据匹配；分享会话不自动授予文件访问，也不自动把私人文件发布成共享 artifact。
- [ ] 对文件已变更/已删除、文档版本变化、结果尚未就绪、权限撤销和下载失败分别显示真实状态并可独立重试；不能把最新文件伪装成回答时快照。
- [ ] 历史回答原有关联保留，新回答不能认领旧资源；同资源多次关联按稳定身份去重，故意多回答引用同一资源仍保持各自归属。
- [ ] 不支持预览的类型仍可按既有下载能力交付；超过资源原有大小限制时明确说明，不通过卡片绕过限额。

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
- Parent 性能审查已纳入：当前初版的完整 transcript fold / 每卡片两次历史重建 / 列表读取全部文件字节必须替换后才可完成。使用有界的单回答投影及独立授权复核；已发布 immutable 元数据可用于列表，实时文件只在下载或单卡 Retry 时验字节。前端仅为可见且带显式引用的回答查询，不对全历史逐回答预取。
- Rust 使用固定 1.96.1、`CARGO_BUILD_JOBS=2`、`RUSTFLAGS='--cfg cma_ui_ticket07'`。已从共享缓存 APFS-clone 到私有 `/Users/liuyitian/code/cma-ui-worktrees/cargo-target-ticket07`，仅在私有目录清理 layer2-core/layer1-public/cma 后重编译；不再等待共享 target 或复用其他工作树 binary。已提供隔离 Docker context `colima-cma-ui-proof`，以及固定 Codex 0.147.0 / ACP 1.4.0 的八步补丁 bridge；真实 runtime 证明仍须执行并记录，普通 fixture 不算 R 证据。
- 已 fast-forward 合入 ticket09 后的 integration `6a606ebf0`。有界读取复用 `AgentTranscriptTurn::final_answer()`，前端将复用 `ConversationMessage.finalAnswer`，保留 09 footer 的 eligibility 共享 hunk。
- 单回答读取只查目标记录、至 terminal 的最多 128 条后续记录，以及最多 512 条记录的所属 turn；整次投影最多 4 MiB / 15 秒。区间缺失、额外 turn/conversation 边界、非 completed 或非最大非空 Assistant 消息都 fail closed。超出此范围明确视为不可用，不退回完整历史 fold；授权复核不重读日志。此界限仍需在最终 API/SDK 文档中说明。

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

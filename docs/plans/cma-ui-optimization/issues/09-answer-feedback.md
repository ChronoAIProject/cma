# 09：评价一条已完成的最终回答

**What to build:** 用户在最终回答底部一键表达有帮助/需改进，刷新后恢复评价，操作不会启动新一轮任务。

**Blocked by:** None（已有消息级 vote 契约可复用）。

**Status:** done（8 项验收均有 U/A 证据；全仓 contract 的无关失败单独记录）

**优先级:** P0

## Acceptance criteria

- [x] 仅已记录、所属 turn 已结束且被确认为最终回答的 Assistant 消息提供评价；用户消息、思考、工具日志、流式片段和只有失败提示的消息不提供入口。
- [x] “最终回答”的身份由会话的稳定消息/turn 语义确定；重新生成的不同回答各自保留记录，不依赖数组位置或内容相同判断。
- [x] 点击有帮助/需改进保存 up/down；再次点击撤销，点另一项切换；成功后才显示已记录，同一回答保存期间避免重复提交。
- [x] 保存失败保持最后确认的选择及可见重试；接收结果不明先核实/精确重试原动作，不直接允许相反写入掩盖未知结果。
- [x] 重开与刷新恢复本人评价，换号/换 Agent 立即隔离旧请求与状态；分享或只读视图保留复制但不提供可保存评价。
- [x] 评价不新增消息、不改变 Agent turn、不自动重新生成；服务端同样验证评价对象和身份，不能只在按钮处限制。
- [x] 桌面 hover/focus 可访问，选中及反馈状态始终可见；移动端入口常显且热区至少 44px，成功/失败可被读屏播报。
- [x] 兼容历史 up/down/null 客户端和既有已存评价，不能因收紧前端入口而删除历史记录。

## Testing boundaries（2026-11-03 已确认）

- **U/A：** 最终回答→评价→公开读取恢复，核对原会话未新增 turn；验证 streaming、中间消息、只读及换号。不能判断回答本身正确性。
- **真实浏览器：** 选中状态、触控热区、键盘焦点和状态播报；不会把点击次数作为持久化成功断言。
- **首个 red → green：** 同一轮含一条中间消息和一条已提交最终回答，仅后者能评价，刷新后恢复其 vote 且会话轮数不变。

## 范围与依赖说明

复用现有 vote 服务与失败恢复；原因面板属于 10，基础版本不展示无法保存的表单。

## 实现与验收证据（2026-11-03）

后端复用现有 owner/message vote 持久化与 `up/down/null` 请求、响应及分页契约。`AgentTranscriptTurn::final_answer()` 仅接受成功完成 turn 内 sequence 最大、内容非空的 Agent 消息；先比较所有角色的消息，再判断角色，不能把中间回答或随后有 owner 消息的回答误认为最终回答。公开写入服务验证归属、授权和该身份，拒绝 active、failed、interrupted、存在 conversation gap、空回答及其他日志目标。历史已存评价仍可列出，不迁移或删除；对非最终回答的新写入（包括撤销）返回拒绝。

前端 `ConversationMessage.finalAnswer` 是同一规则的展示投影，绑定 `runId` 与 `event:<sequence>`；transcript 与 durable `turn_completed` 均建立该标记，live delta 不建立。已完成的不同 turn 即使正文相同也各有独立反馈。沿用基线的 memo、scope、abort 和 pending guards，保存期间每条回答只允许一个请求；服务端保留原有绝对赋值、幂等重试、跨客户端最后成功写入生效的契约，没有引入 CAS。

仅成功响应显示 `Vote recorded` / `Vote removed`。失败保留已确认值并锁定到原动作的 `Retry vote`；初次读取失败显示 `Retry feedback`。发送前按 viewer/Agent 将原请求值写入 sessionStorage，重开或刷新先公开读取核实，未核实则精确重试。存储写入被拒绝时内存回退仍保护同一页面生命周期内的重开；浏览器无法写入存储时不能承诺跨整页刷新保存未确认意图。换号、换 Agent、权限关闭立即隔离旧状态和延迟响应。只读保留 Copy，隐藏 vote。

| 边界与验收 | 实际结果 |
| --- | --- |
| U 首个 red → green | 同 turn 中间消息原先也显示 Helpful，新增公开组件行为用例先失败；完成 final-answer 标记后只保留最终回答入口，重开恢复 vote、原有消息数不变。 |
| A 首个 red → green | 真实 `MessageFeedbackService` 原先接受中间消息，新增用例先失败；现仅最终回答可保存。重建服务后公开读取恢复；原 transcript 相等。 |
| U 失败与并发 | `AnswerFeedback.test.tsx` 14 项通过：确认后切换/撤销、pending 锁定、未知结果精确重试、读失败恢复、存储写入失败重开、相同正文重新生成、换 viewer/Agent 延迟响应隔离、只读、未完成/失败/中断/gap。存储失败新增一项先 red（重开丢失 Retry vote）再 green。 |
| A 业务服务 | `layer2-core` 的 `message_feedback_tests` 实际执行 2 项通过；含历史记录保留、身份/权限拒绝和服务重建恢复。 |
| A 认证公开路由 | `layer1-public message_feedback` 实际执行 5 项通过；含最终回答校验、切换/撤销、公开读取恢复、真实共享授权与撤销权限、分页及原 transcript 不变。 |
| 浏览器 U | Chrome 1440×1000、390×1000 共 2 项通过；桌面隐藏→键盘 Tab/focus 可见、失焦后选中仍可见、移动常显且 44×44、role=status/alert、丢响应后整页刷新核实、撤销后刷新、无新任务写入。现有 Agent settings 浏览器回归另 6 项通过。 |
| Web 回归 | Agent 及反馈 REST 套件 108 文件、865 项通过（含保留的性能测试）；typecheck、e2e:typecheck、全量 ESLint、production build、CSS collision check、改动文件 Prettier 通过。 |
| SDK 兼容 | TS feedback + public-contract 2 文件、2713 项通过；TS build、docs check 48 示例/11 文档通过。Rust SDK public contract 1 项通过：2269 typed HTTP、171 typed preflight、239 unrepresentable wire shapes、27 browser-owned contracts。 |
| 后端 feedback public contract | 使用 `CMA_PUBLIC_CONTRACT_FILE=message-feedback.json`，1 项测试实际经真实 handler 回放 18 个契约案例通过。 |
| 后端完整 public contract | 默认测试线程栈发生 stack overflow；提高到 `RUST_MIN_STACK=33554432` 后实际运行在无关 `agent-skills-install-version-conflict` 失败：expected `agent_operation_in_flight`，actual `agent_skill_version_upgrade_unsupported`。未修改该技能契约，也未把全套标记为通过。 |

浏览器截图在 `/tmp/cma-ticket09-evidence/{before,after}-{1440,390}.png`，已人工检查最终两张：无截断，移动反馈状态自然换行。浏览器使用批准的本地网络边界，持久化另由 A 边界证明；未声称运行生产数据库或真实 Agent runtime，也未判断回答正确性。

### 可复跑命令与本地日志

所有命令在本 worktree；Rust 使用 `PATH=/Users/liuyitian/.cargo/bin:$PATH`、`CARGO_BUILD_JOBS=2`、`CARGO_TARGET_DIR=/Users/liuyitian/code/cma-ui-worktrees/cargo-target`、`RUSTFLAGS='--cfg cma_ui_ticket09'` 和 `cargo +1.96.1`。独立 cfg 避免共享 target 中其他 worktree 的测试产物被误用；早期共享缓存显示 0 tests 的运行不计入证据。

- `npm --prefix web test -- --maxWorkers=2 src/agent src/api/rest/messageFeedback.test.ts`：`/tmp/cma-ticket09-web-tests-final.log`。
- `cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p layer2-core message_feedback_tests -- --nocapture`：`/tmp/cma-ticket09-domain-regression.log`。
- `cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p layer1-public message_feedback -- --nocapture`：`/tmp/cma-ticket09-rest-tests.log`。
- cargo 共用 target 持续等待其他 worker 的构建锁，故直接运行刚由本分支编译、已实际通过 5 个认证路由测试的隔离产物：`CMA_PUBLIC_CONTRACT_FILE=message-feedback.json RUST_MIN_STACK=33554432 /Users/liuyitian/code/cma-ui-worktrees/cargo-target/debug/deps/layer1_public-55aa1992f41e05a9 shared_public_contract_matches_real_backend --nocapture`，工作目录 `api/crates/layer1-public`，反馈通过日志 `/tmp/cma-ticket09-feedback-contract.log`。重新构建时可用相同筛选环境变量执行 `cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p layer1-public shared_public_contract_matches_real_backend -- --nocapture`。
- 去掉 `CMA_PUBLIC_CONTRACT_FILE` 的完整实际运行失败见 `/tmp/cma-ticket09-backend-contract-direct.log`；默认栈溢出见 `/tmp/cma-ticket09-backend-contract.log`。重复排队的 cargo 进程已取消，不计为验证。
- 在 `web`，`CMA_E2E_BROWSER_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' node_modules/.bin/playwright test --config=e2e/answer-feedback.config.ts`，Vite 本地端口 4599：`/tmp/cma-ticket09-browser-final.log`。现有 settings 配置另设 `CMA_STARTUP_PREVIEW_URL=http://127.0.0.1:4599`，限定 `agent-settings.check.ts`：`/tmp/cma-ticket09-browser-settings.log`。
- 在 `sdk/typescript`，`../../web/node_modules/.bin/vitest run --maxWorkers=2 tests/message-feedback.test.ts tests/public-contract.test.ts`：`/tmp/cma-ticket09-sdk-tests.log`；`../../web/node_modules/.bin/tsc --project tsconfig.build.json`、`node scripts/check-docs.mjs` 均通过。
- `cargo +1.96.1 test --manifest-path sdk/rust/Cargo.toml --test public_contract -- --nocapture`：`/tmp/cma-ticket09-rust-sdk.log`。

只检查本票 6 个 Rust 文件的 rustfmt；未运行全仓格式化覆盖。预存 bindings 格式失败仍见 `/Users/liuyitian/code/cma-ui-worktrees/baseline-rust-format.log`。production build 通过但保留既有大 chunk 提示。原先阻塞 L1 的 E0308 已通过合入集成分支 `60fc98ee5` 的最小修复解除。

本票未解决验收项：无。额外回归限制：完整后端 public contract 的技能错误码失败仍存在，尚未在本票之外的分支独立复现，不将其宣称为已验证基线问题。反馈专属真实 handler 案例、认证路由、业务服务、SDK 和浏览器验证全部通过。

### 共享文件交接

- 07：`StreamEntry.tsx` 仅把 `canVote` 加上 `message.finalAnswer === true`，未修改 artifact card 或 `MessageStream`；保留此唯一共享 hunk。
- 07/11/14：统一最终回答 eligibility 为成功 completed turn、最大 durable message sequence、非空 Assistant、无 gap。绑定用 `runId`/turn sequence + `event:<sequence>`，后端复用 `AgentTranscriptTurn::final_answer()`，前端复用 `ConversationMessage.finalAnswer`；不要用数组末项或正文相同判断。
- 11：若维护 `completedAnswer {turnSequence, answerSequence, completedSequence}` 摘要，应从上述同一身份派生；本分支未修改其他 worker 的 worktree，其他票尚未合入本票的改动不声称已完成集成。
- 14：保留 `conversation_gap` 的 `runId` 关联及 `turn_completed` 标记逻辑。基线 `useMessageFeedback` 的 viewer/Agent scope、请求失效保护与性能优化均保留。
- 10：原因与说明仍属独立票；本票不增加无法保存的原因表单。

### 本票改动路径（不含上游合并带入文件）

```text
api/crates/layer1-public/src/rest/agents_tests/message_feedback.rs
api/crates/layer1-public/tests/fixtures/public-contract/message-feedback.json
api/crates/layer2-core/src/sandbox/agent_read_types.rs
api/crates/layer2-core/src/sandbox/agent_reads.rs
api/crates/layer2-core/src/sandbox/agent_reads_tests.rs
api/crates/layer2-core/src/sandbox/agent_reads_tests/message_feedback_tests.rs
api/crates/layer2-core/src/sandbox/message_feedback.rs
docs/API_Agent_Manager.md
docs/API_REST.md
docs/Web_Style.md
docs/plans/cma-ui-optimization/issues/09-answer-feedback.md
sdk/rust/docs/agents.md
sdk/typescript/docs/agents.md
web/e2e/agent-settings.check.ts
web/e2e/answer-feedback.check.ts
web/e2e/answer-feedback.config.ts
web/src/agent/Agent.css
web/src/agent/AnswerFeedback.test.tsx
web/src/agent/MessageFooter.style.test.ts
web/src/agent/MessageFooter.test.tsx
web/src/agent/MessageFooter.tsx
web/src/agent/MessageIdentity.test.tsx
web/src/agent/StreamEntry.tsx
web/src/agent/conversation.ts
web/src/agent/feedbackRecovery.ts
web/src/agent/model.ts
web/src/agent/transcript.ts
web/src/agent/useMessageFeedback.ts
```

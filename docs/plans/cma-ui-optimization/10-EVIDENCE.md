# Ticket 10：负向原因验收与交接

日期：2026-11-03。状态：本票验收完成；全仓门禁的上游例外单独列出。

工作区 `/Users/liuyitian/code/cma-ui-worktrees/ticket-10`，分支 `codex/cma-ui-10`。初始基线 `7739c431f`，包含 01/02/03/06/09/11；`262a2c6fc` 先合入 14 所在集成，最终 `e5f5e952b` 合入最新集成 `d19c225bd2a373f0e4861e95ebbbb5a5bca8adc8`，同时保留 04/05/14。样式冲突通过保留完整集成文档为原样前缀、追加本票 33 行解决；OpenAPI 复用 04 抽出的 composition，保留其 repository 扩展并增加反馈契约。原工作区、集成工作区及其他 worker 源码未编辑；没有推送、部署、PR 或 worktree 删除。

## 验收矩阵

| 标准 | 实现与可观察证据 |
| --- | --- |
| down 立即保存，原因独立 | 原 vote PUT 保持 `{vote}` 形状。面板立即打开，待已确认 down 和详情回读后才允许保存；投票尚未结束时关闭面板也不会被迟到的成功重新打开。U 测试保留输入，浏览器确认没有 Agent 命令写入。 |
| 六选多选、500 码点 | `inaccurate/unanswered/unusable_steps/incomplete/unclear/other`；前端 `Array.from`、Rust `chars`。500 个非 BMP 字符成功，501 明确拒绝；枚举、重复项、请求 ID 与未知字段有拒绝契约。 |
| 独立返回与回读、再次修改 | 新 GET feedback 详情和 PUT feedback/reasons 返回 viewer/Agent/answer、version、reasons、comment、updated_at、request_id。再次编辑仅在确认后替换已保存内容；新、旧 REST 契约及双 SDK 使用相同公开语义。 |
| 草稿、关闭与失败恢复 | 草稿保留在现有 `feedbackRecovery` 的 viewer/Agent/message scope；关闭与 Agent 导航保留，失败只重试原原因 body。丢失成功回执可通过公开回读确认；版本冲突保留草稿，明确回读后才使用新版本和新请求 ID。 |
| 原子清理与迟到拒绝 | Mongo 单记录 version + vote CAS、majority read/write；up/null 原子清除原因和回执，null 留版本墓碑而不进入旧非空 vote 列表。原因操作不能设置 vote；down→up/null→down 的 ABA 也拒绝旧版本。 |
| 精确重试与权限隔离 | 同一请求重试返回相同版本和时间。真实 Mongo 的 12 路并发初次 vote 与 12 路原因重试保持一个结果；401/403/404、非 owner、非最终回答拒绝；换号、权限丢失与迟到响应不会泄露或恢复草稿。 |
| 兼容与迁移 | schema-v1 vote-only 行按 version 0、空原因、空说明回读并惰性迁移；18 个旧 vote 契约案例通过。公开旧客户端 up/null 同样清理。严格旧二进制不兼容扩展记录，部署/回滚须保留新版 reader 和清理/version 语义，见 API Agent Manager 文档。 |
| 桌面与手机 | 1440/390px Chrome 通过默认、已赞、展开、保存中、失败、成功、撤销、重新打开和刷新；44px 触控、Unicode 超限提示、无横向溢出。已检查截图中的答案、面板、输入区和 14 电脑侧栏。 |

沿用 09 的 `MessageFooter` 规范回答 ID、`finalAnswer` eligibility、投票恢复和 scope 防护；没有新增最终回答身份缓存。原因只在打开或投票确认时读取，不产生每条历史回答的详情请求。完成请求及时释放 AbortController；流式 Markdown、反馈 controls 稳定性和旧 owner 回调失效继续通过性能回归。

U 使用正常 REST client 与浏览器网络 fixture；A 使用真实 Axum handler、AuthenticatedCaller 和业务服务，存储 fixture 与真实 Mongo 并发分别验证。Mongo 断言走 store 的 get/list 公共边界，原始数据库写入仅用于构造旧格式 fixture。本票没有新增 runtime 能力，未声称运行真实模型；既有测试 runtime registry 拒绝解析时反馈仍成功，证明反馈没有发送依赖。反馈内容不参与模型训练。

## 首条 red → green

第一条行为始终是 `message_feedback_delayed_reasons_after_up_never_restore_down_or_reasons`：保存 down，up 后送达旧版原因，再公开回读仍为 up、空原因和说明。

- `/tmp/cma-ticket10-first-red.log`：1 个命名测试失败，原因端点尚不存在（404）。
- `/tmp/cma-ticket10-first-green.log`：同一命名测试 1 passed、0 failed。
- 后续 red → green：`unicode-red.log`（501 曾错误接受）、`validation-red.log`（重复原因）、`reuse-red.log`（请求 ID 改内容）、`ui-red.log`（无面板）、`close-red.log`（迟到 vote 重新展开）、`projection-red.log`（精简 OpenAPI 遗漏新路由），均位于 `/tmp/cma-ticket10-*`；最终对应测试通过。

## 复验结果与日志

| 验证 | 结果 | 日志 |
| --- | --- | --- |
| 最终 d19c225bd 合并态反馈 A + OpenAPI 投影 | 11 个真实认证路由测试 + 1 个公开 schema 投影测试通过 | `/tmp/cma-ticket10-final-integration-api.log` |
| 合并态 Core | 2 个命名测试通过 | `/tmp/cma-ticket10-merged-core.log` |
| 最终合并态真实 Mongo | 4 个命名测试通过；up/null 各 24 轮竞态，12 路精确重试，重开、旧格式与 ABA | `/tmp/cma-ticket10-final-integration-mongo.log` |
| 最终合并态新原因实际 handler 契约 | 1 个测试，26 个案例通过 | `/tmp/cma-ticket10-final-integration-reasons-contract.log` |
| 最终合并态旧 vote 实际 handler 契约 | 1 个测试，18 个案例通过 | `/tmp/cma-ticket10-final-integration-vote-contract.log` |
| 14 合并态相关 Web | 115 文件、950 tests 通过，含 14 的七项折叠回归 | `/tmp/cma-ticket10-merged-web.log` |
| 最终合并态聚焦 Web/性能 | 7 文件、44 tests 通过，含新增冲突回读与最终请求清理 | `/tmp/cma-ticket10-final-integration-focused-web.log` |
| 最终合并态 Chrome | 1440/390px 各 1 项通过，截图已检查 | `/tmp/cma-ticket10-final-integration-browser.log` |
| TypeScript SDK | 2 文件、2740 tests 通过 | `/tmp/cma-ticket10-merged-ts-sdk.log` |
| Rust SDK 全部共享契约 | 1 个测试通过；2292 typed HTTP、173 preflight、240 typed-shape、27 browser-owned 契约 | `/tmp/cma-ticket10-merged-rust-sdk.log` |
| Web 类型、e2e 类型、lint、build | 最终合并态类型/lint 通过；14 合并态 build 通过，保留既有大 chunk 提示 | `/tmp/cma-ticket10-final-integration-{typecheck,e2e-types,lint}.log`、`/tmp/cma-ticket10-merged-build.log` |
| SDK 构建和文档 | TypeScript 构建通过，48 示例/11 文档通过 | `/tmp/cma-ticket10-merged-sdk-{build,docs}.log` |
| 最终合并态 REST 文档 inventory | 1 个非零命名测试通过；独立于下述共享 JSON inventory 门禁 | `/tmp/cma-ticket10-final-integration-doc-inventory.log` |

浏览器图像在 `/tmp/cma-ticket10-evidence/`：`before-{1440,390}.png` 为初始 09 界面；`default/helpful/expanded/saving/failed/saved/removed-{1440,390}.png` 为本票状态。最终桌面含 14 的电脑侧栏；手机面板在正常布局内，输入区保持可用。真实浏览器 fixture 明确声明了 14 新增的 files/tree 和 environment 读取，不把这些样例当作 runtime 能力证明。

所有 Rust 命令使用 `PATH=/Users/liuyitian/.cargo/bin:$PATH`、`CARGO_BUILD_JOBS=2`、`cargo +1.96.1 --locked`、`RUSTFLAGS='--cfg cma_ui_ticket10'`。API target 为 `/Users/liuyitian/code/cma-ui-worktrees/cargo-target`，SDK 独立 target 为 `/Users/liuyitian/code/cma-ui-worktrees/cargo-target-ticket10-sdk`。逐个核对命名非零测试数；没有把共享目录中的其他二进制或 0-test 运行计为证据。

可复跑命令（均在本 worktree，先设置上述 Rust 环境）：

```sh
cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p layer1-public --lib message_feedback -- --nocapture
cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p layer2-core --lib message_feedback_tests -- --nocapture
CHRONO_TEST_MONGODB_URI='mongodb://127.0.0.1:27027/?directConnection=true&replicaSet=ticket11' cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p cma message_feedback -- --ignored --nocapture
RUST_MIN_STACK=33554432 CMA_PUBLIC_CONTRACT_FILE=feedback-reasons.json cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p layer1-public --lib shared_public_contract_matches_real_backend -- --nocapture
RUST_MIN_STACK=33554432 CMA_PUBLIC_CONTRACT_FILE=message-feedback.json cargo +1.96.1 test --locked --manifest-path api/Cargo.toml -p layer1-public --lib shared_public_contract_matches_real_backend -- --nocapture
npm --prefix web test -- --configLoader=runner --no-cache --maxWorkers=2 src/agent src/api/rest/messageFeedback.test.ts src/resources/SharedAgent.test.tsx
```

SDK：在 `sdk/typescript` 使用 `../../web/node_modules/.bin/vitest run --configLoader=runner --no-cache --maxWorkers=2 tests/message-feedback.test.ts tests/public-contract.test.ts`；Rust 使用 `cargo +1.96.1 test --locked --manifest-path sdk/rust/Cargo.toml --test public_contract -- --nocapture`。依赖只在本 worktree 的 SDK node_modules 补齐软链接，未安装或改写原工作区依赖。

浏览器：在 `web` 以隔离 Vite cache `/tmp/cma-ticket10-vite-cache` 启动本地 4610 端口，再设置 `CMA_FEEDBACK_PREVIEW_URL=http://127.0.0.1:4610` 和 `CMA_E2E_BROWSER_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'`，运行 `node_modules/.bin/playwright test --config=e2e/feedback-reasons.config.ts`。本票 Vite PID 26476 已关闭，4610 无监听；共享 `colima-cma-ui-proof` VM 与 `cma-ticket11-mongo` 容器保持运行。每次 Mongo 测试用 UUID 独立数据库并在成功后删除；最终查询 `cma_ticket10_` 前缀数据库为 `[]`。

## 明确的上游门禁与边界

- 初始基线上的全量 Web：319 文件/2905 tests 通过，6 文件/17 tests 失败，日志 `/tmp/cma-ticket10-web-all.log`。其中 ActiveThinking 4、MessageIdentity 1、SharedAgent 2 为用户指定 14 的折叠回归，合并 14 后的 950 项相关验证已全部通过。合入最新 04 后，AppCreation/Console 与本票反馈共 9 文件/57 tests 通过；AppPublic 的 6 项旧首页断言仍失败，见 `/tmp/cma-ticket10-final-integration-web.log`。TaskHome 文案扫描此前 1 项失败，未纳入本次聚焦重跑；这些首页/旧导航断言和 14 文件隔离分析不由本票修改，未提交 skip 或把全量称为绿色。
- 合并态 CSS gate 失败：`.agent-files-panel` 同时在 `AgentComputer.css` 和 `AgentFiles.css` 声明，两个文件均来自 14。本票初始 CSS gate 通过；通过 `git archive a426b87ee` 的独立临时源码快照运行同一脚本，复现完全相同错误，见 `/tmp/cma-ticket10-upstream-css.log`。最终合并态再次复现，日志 `/tmp/cma-ticket10-final-integration-css.log`。
- 全局共享 contract inventory 在最终合并后报告实际 230、登记 229，见 `/tmp/cma-ticket10-final-integration-inventory.log`。14 新增 `GET /api/v1/agents/{id}/environment`（`agent_filesystem::environment::read`）未进入上游 JSON inventory/共享 corpus；`d19c225bd` 的 JSON inventory 仍为 227。最新集成修正的是 `docs/API_REST.md` 的文档 inventory，这是独立检查，不能替代共享契约登记。10 的两个新增操作登记完整，合并前 229-operation gate 通过，新/旧反馈 44 个真实 handler 案例在合并后仍通过。未通过改数字、删路由或放宽 gate 掩盖缺失；此项交回 14/集成补齐，不宣称重跑了上游完整二进制。
- 原始格式、skill-error public contract/Core fixture、508 行 Agent service 未改。04 的 Console/导航和首页浏览器修复仅随集成合入，本票没有追加修正。只格式化本票变更 Rust/TS/CSS；本票触及源文件均不超过 500 行、测试不超过 2000 行，OpenAPI 主文件 496 行。没有运行覆盖全仓的格式重写。

## Changed manifest（相对合入的 d19c225bd，不含上游文件）

共 51 路径；包含本验收文档和 tracker 更新。独立 merger 应以本票分支为输入，保留集成样式前缀与 OpenAPI 组合函数的行为等价性。

```text
api/crates/cma/src/store/message_feedback.rs
api/crates/cma/src/store/message_feedback_mongo_tests.rs
api/crates/layer1-public/src/rest/agents.rs
api/crates/layer1-public/src/rest/agents_tests/message_feedback.rs
api/crates/layer1-public/src/rest/agents_tests/message_feedback_reasons.rs
api/crates/layer1-public/src/rest/message_feedback.rs
api/crates/layer1-public/src/rest/message_feedback_reasons.rs
api/crates/layer1-public/src/rest/mod.rs
api/crates/layer1-public/src/rest/openapi.rs
api/crates/layer1-public/src/rest/openapi/agent_manager.rs
api/crates/layer1-public/src/rest/openapi/agent_manager_tests.rs
api/crates/layer1-public/src/rest/openapi/composition.rs
api/crates/layer1-public/src/rest/public_contract_tests/inventory.rs
api/crates/layer1-public/tests/fixtures/public-contract-inventory.json
api/crates/layer1-public/tests/fixtures/public-contract/feedback-reasons.json
api/crates/layer2-core/src/sandbox/agent_reads_tests/message_feedback_tests.rs
api/crates/layer2-core/src/sandbox/message_feedback.rs
api/crates/layer2-core/src/sandbox/message_feedback/reasons.rs
docs/API_Agent_Manager.md
docs/API_REST.md
docs/Web_Style.md
docs/plans/cma-ui-optimization/10-EVIDENCE.md
docs/plans/cma-ui-optimization/IMPLEMENTATION.md
docs/plans/cma-ui-optimization/issues/10-feedback-reasons.md
sdk/rust/docs/agents.md
sdk/rust/src/agents/feedback_reasons.rs
sdk/rust/src/agents/mod.rs
sdk/rust/tests/public_contract/classification.rs
sdk/rust/tests/public_contract/resources.rs
sdk/typescript/docs/agents.md
sdk/typescript/src/agents.ts
sdk/typescript/src/agents/feedback-reasons.ts
sdk/typescript/src/index.ts
sdk/typescript/tests/message-feedback.test.ts
sdk/typescript/tests/public-contract/expectations.ts
sdk/typescript/tests/public-contract/resources.ts
web/e2e/feedback-reasons.check.ts
web/e2e/feedback-reasons.config.ts
web/src/agent/AnswerFeedback.test.tsx
web/src/agent/FeedbackReasons.css
web/src/agent/FeedbackReasons.test.tsx
web/src/agent/FeedbackReasons.tsx
web/src/agent/MessageFooter.tsx
web/src/agent/MessageStream.performance.test.tsx
web/src/agent/feedbackRecovery.ts
web/src/agent/useFeedbackReasons.ts
web/src/agent/useMessageFeedback.ts
web/src/api/client.ts
web/src/api/feedbackTypes.ts
web/src/api/rest/feedbackReasons.ts
web/src/api/rest/messageFeedback.ts
```

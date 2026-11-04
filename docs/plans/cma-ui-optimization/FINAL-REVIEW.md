# 最终审查与修复记录

日期：2026-11-03。技术修复和独立合并验收已完成；建议质量仍待实际人工评审。

两路独立审查均针对基线 `c9e2d7b91a0c49c68d3cd2968e61dd8568ed3706`
与集成提交 `28970cf01d429b1924b79c4b6d17b2da9c8d8e84`。
下列行号指向该受审版本；保留原始发现，不将修复后的结果改写为最初没有问题。

## Standards

Fixed `c9e2d7b91a0c49c68d3cd2968e61dd8568ed3706` → `28970cf01d429b1924b79c4b6d17b2da9c8d8e84`. **7 findings: 6 documented violations; 1 judgment call.**

- **P2 — CORS:** [browser control:98](/Users/liuyitian/code/cma-ui-integration/sdk/typescript/src/agent-browser-control.ts:98), [snapshot:73](/Users/liuyitian/code/cma-ui-integration/sdk/typescript/src/agent-events-v2/index.ts:73)/[events:117](/Users/liuyitian/code/cma-ui-integration/sdk/typescript/src/agent-events-v2/index.ts:117), and [deliverables:194](/Users/liuyitian/code/cma-ui-integration/sdk/typescript/src/agents/deliverables.ts:194) send `Prefer`, `X-CMA-Browser-Client`, or `X-CMA-Document-Share-Token`, absent from [grants:230](/Users/liuyitian/code/cma-ui-integration/api/crates/cma/src/main.rs:230)/[validation:286](/Users/liuyitian/code/cma-ui-integration/api/crates/cma/src/main.rs:286). Configured cross-origin control, default snapshots, output replay and token-backed deliveries fail. Admit those headers; omit empty Prefer. Rule: [SDK.md:24](/Users/liuyitian/code/cma-ui-integration/docs/SDK.md:24), cross-origin SDK support.
- **P2 — Return blocked:** [BrowserControl.tsx:132](/Users/liuyitian/code/cma-ui-integration/web/src/agent/environment/BrowserControl.tsx:132) gates return on fresh video. With a valid lease but stale observation, the enabled button dispatches nothing. Exempt return from frame freshness. Rule: [Web_Style.md:4404](/Users/liuyitian/code/cma-ui-integration/docs/Web_Style.md:4404), freshness gates input; Return stays reachable.
- **P2 — Oversized preview read:** [previewRead.ts:41](/Users/liuyitian/code/cma-ui-integration/web/src/agent/previewRead.ts:41) requests byte verification before checking size; [read_item:191](/Users/liuyitian/code/cma-ui-integration/api/crates/layer2-core/src/sandbox/answer_deliverables.rs:191) consequently reads/hashes oversized live files. Authorize metadata, check size, then read bytes. Rule: [Web_Style.md:4311](/Users/liuyitian/code/cma-ui-integration/docs/Web_Style.md:4311), 1 MiB bound before reading.
- **P2 — Incomplete answer accepted:** [answer.rs:78](/Users/liuyitian/code/cma-ui-integration/api/crates/layer2-core/src/sandbox/agent_reads/answer.rs:78)/[136](/Users/liuyitian/code/cma-ui-integration/api/crates/layer2-core/src/sandbox/agent_reads/answer.rs:136) skip ConversationGap, accepting deliverables from a turn canonical history marks failed. Reject in-turn gaps. Rule: [API_REST.md:1875](/Users/liuyitian/code/cma-ui-integration/docs/API_REST.md:1875), gaps fail closed.
- **P2 — False completion display:** [durable:190](/Users/liuyitian/code/cma-ui-integration/web/src/agent/conversation.ts:190)/[AG-UI:438](/Users/liuyitian/code/cma-ui-integration/web/src/agent/conversation.ts:438) record completed after recorded loss, rendering “Completed in…” and successful plan state. Preserve gap outcomes in both reducers. Rule: [Web_Style.md:4014](/Users/liuyitian/code/cma-ui-integration/docs/Web_Style.md:4014), recorded successful completion only.
- **P2 — False navigation completion:** [task_status.rs:92](/Users/liuyitian/code/cma-ui-integration/api/crates/layer2-core/src/sandbox/task_status.rs:92) accepts the latest completion, while [Mongo:13](/Users/liuyitian/code/cma-ui-integration/api/crates/cma/src/store/agent_task_status.rs:13), [hierarchy memory:125](/Users/liuyitian/code/cma-ui-integration/api/crates/layer2-core/src/sandbox/store/hierarchy_memory.rs:125), and [inventory memory:95](/Users/liuyitian/code/cma-ui-integration/api/crates/layer2-core/src/sandbox/store/memory/provenance_inventory.rs:95) discard preceding gaps. Retain bounded current-turn gap evidence. Rules: [API_REST.md:901](/Users/liuyitian/code/cma-ui-integration/docs/API_REST.md:901), recorded gaps; [Web_Style.md:3858](/Users/liuyitian/code/cma-ui-integration/docs/Web_Style.md:3858), durable successful last task.
- **P3 — Judgment, Duplicated Code:** [browser service:219](/Users/liuyitian/code/cma-ui-integration/api/crates/layer2-core/src/sandbox/agent_browser_service.rs:219) duplicates [AgentEnvironment::incarnation:53](/Users/liuyitian/code/cma-ui-integration/api/crates/layer2-core/src/sandbox/agent_environment.rs:53), including `digest.update(b"cma-agent-environment-v1\0");`. Reuse the helper so identity-format changes cannot split observation/control fences. Supporting baseline: Fowler Duplicated Code; no hard repository prohibition.

Coverage: all 713 paths/1,370 hunks screened; targeted standards/code traces and bounded source counterexamples, not exhaustive execution. Bounded SDK/store/projection/harness/documentation follow-up completed. Existing evidence was context, not rerun proof. No builds, VM, Spec/fixer reports, or repository mutations; known open acceptance gates remain separate.

## Spec

Fixed comparison: `c9e2d7b91a0c49c68d3cd2968e61dd8568ed3706...28970cf01d429b1924b79c4b6d17b2da9c8d8e84`. All 713 changed paths remain in scope; the preexisting baseline is excluded.

1. **[P2] Next-operation pagination loses overlapping commands.** Spec `/Users/liuyitian/code/cma-ui-integration/docs/plans/cma-ui-optimization/issues/18-operation-replay.md:14`: 「面板提供可键盘操作的回放进度/上一步/下一步，以及“返回实时”；明确标记“操作记录回放”，未录屏时不显示伪历史浏览器画面。」 Source `/Users/liuyitian/code/cma-ui-integration/web/src/agent/environment/replayIndex.ts:59`, with the discarded-update branch at line 78. Reproduce: A starts at cursor 3, B starts at 4, A finishes at 5, B finishes at 6. Restore selection 5, then choose Next. Filtering out both earlier starts leaves no records to receive B’s later completion; the UI reports “No newer operations recorded,” although Live contains B’s output at 6. The existing pagination tests use sequential commands. Preserve overlapping operation identity across the cursor boundary.

2. **[P2] Returning control silently fails when observation becomes stale.** Spec `/Users/liuyitian/code/cma-ui-integration/docs/plans/cma-ui-optimization/issues/17-browser-takeover.md:16`: 「归还控制权后用户输入立即失效，Agent 在原浏览器上下文继续；不重建会话、不丢页面状态，也不把归还按钮当用户新任务消息。」 Source `/Users/liuyitian/code/cma-ui-integration/web/src/agent/environment/BrowserControl.tsx:132`; `/Users/liuyitian/code/cma-ui-integration/web/src/agent/environment/BrowserPanel.tsx:210` supplies frame freshness as `active`. After acquiring a valid lease, let frames become stale while control transport remains available. “Return to Agent” remains enabled, but clicking it sends no return request because the input-readiness guard also rejects return. Agent input stays blocked until frames recover or the lease expires. Separate release eligibility from frame-dependent input eligibility.

Both are implemented-but-incorrect requirements, reproduced against the fixed source using in-memory probes. The existing three replay/control suites pass all 20 tests. No additional concrete missing requirement or unrequested behavior established.

Coverage: all 19 ticket specifications, cross-layer implementation tracing, relevant tests, and supporting per-ticket/integrated evidence; not an exhaustive assertion about every line. No agents, Standards review, repository edits, heavy builds, or native/VM reruns. Ticket 13 human quality, outstanding formal gates, controlled external-service boundaries, and declared accessibility/deployment/scale limits remain separate prerequisites or limitations.

## 修复处置

全部发现由同一名实现者在 `codex/cma-ui-review-fixes` 修复，最终源码提交为
`a3fe953f2ee275ccdef4dc000e2226e6be18e7cc`。独立源码复核没有发现新的具体缺陷；
最终证据核对、集成合并和合并后检查均已通过。归还控制权共用一次实现，
两个审查轴保留各自记录。

| 审查项 | 修复提交 | 修复及已执行证据 |
| --- | --- | --- |
| Standards1 | `a3a6dd57c` | 仅放行三个必需请求头；默认快照不发送空 Prefer。两个 CORS 测试通过，原 origin/未知请求头拒绝保留。 |
| Standards2 | `704fc2f62` | Return 不再依赖画面新鲜度；租约、用户、Agent、环境及授权检查保留。实际发出归还请求及失效身份用例包含在最终 Web 测试中。 |
| Standards3 | `8f2792376` | 元数据授权和大小门槛在读取字节前；已保存小文件后来变大时，实际读取也受原大小限制。保留空文件、大小/摘要变化和初始捕获上限；增长边界先 RED 后三个 Core 用例 GREEN。 |
| Standards4 | `8f2792376` | 回答所属 turn 有记录缺口时拒绝成果列表、检查和下载，下一完整 turn 不受影响。Core 缺口六例、公开成果五例通过。 |
| Standards5 | `8f2792376` | durable、Responses 和 AG-UI 均保留不完整结果，完成摘要/计划不再显示成功；相关四套测试 54 项通过。 |
| Standards6 | `8f2792376` | Mongo 与两个内存存储保留当前 turn 的缺口证据；查询有记录数和时间上限。实际隔离 Mongo 场景通过。 |
| Standards7（判断项） | `a3fe953f2` | 浏览器观测复用既有 incarnation 方法；独立核对身份哈希字节不变，无新抽象。 |
| Spec1 | `704fc2f62` | 分页保留跨游标的操作身份，并修正前一窗口游标；重叠操作和 53 个操作的双向窗口用例通过。 |
| Spec2 | `704fc2f62` | 与 Standards2 共用实现和证据，需求轴独立保留该处置。 |

最终受影响源码检查：Web 343 个文件、3057 项通过；TypeScript SDK 3765 项通过，
保留一个原有 opt-in 跳过。Web/SDK 类型、Web E2E 类型、lint、格式、构建、
CSS 和设计规范检查通过；SDK 文档检查通过 49 个示例。增长修复后重跑 Core
缺口6、成果3、文件系统4、公开成果5及三包 Clippy，均通过。历史扫描在修复
提交和独立合并提交上均检查2582个扫描器报告的提交，无发现。最终文档提交后的
历史门禁与保护核对记录在工作区外 `review-final/FINAL-VERIFICATION.json`。

以上是明确范围内的实际结果，不声称全仓 CI 已全部通过。原有全仓格式、
skill fixture、508行服务和 LocalAgentV2 基线限制见实施记录。早期零测试选择、
构建/测试配置失败和默认线程栈溢出的日志保留；不是通过证据。栈溢出的准确
用例在32MiB线程栈设置下单独通过，不由此声称完整 Core 套件通过。

本次没有重跑旧 ticket19 的完整 native 组合旅程。旧37/38证据对应其记录的
源码和固定运行时；本次修复后的源码由这里列出的针对性回归及 Web/SDK 套件覆盖。

## 独立合并验收

合并提交 `1ddf3bb0e8770e99c2186089ffa8562bb40b27a9` 位于
`codex/cma-ui-optimization-clean`，与实现者 READY 提交的树
`d97f49363cd5de91c1225fee91d9ca0f80c9c572` 完全相同，无冲突或额外源码修改。
独立核对30个修改路径、94份证据和三个当前可执行文件，检查前后摘要不变；
149条原始 AC 文字、既有 WebStyle 全文前缀和27个源码/测试行数边界通过。

合并态实际执行：Web六文件81项、SDK一文件8项、Core/public共18个具名测试
全部通过；Web、E2E和SDK类型检查通过。没有重复全量套件或重建 Rust 二进制。
最终 Core/public 可执行文件对应增长修复后的源码；当前 CMA 可执行文件对应
较早的验证快照，CORS2/Mongo1沿用该明确边界的证据。较早的 public-browser
可执行文件已在实现阶段重编译覆盖，其历史日志/哈希不是当前可执行文件证明。

详细记录位于 `/Users/liuyitian/code/cma-ui-worktrees/review-fixes-proof/HANDOFF.md`
及 `/Users/liuyitian/code/cma-ui-worktrees/review-merge/MERGE-HANDOFF.md`。
修复 manifest 的 SHA-256 为
`76c3b2987c7c03cf91546f8f75d35eef147c3bbdb0f1d48597f1df89beb25d40`。

## 尚待人工验收

[建议质量人工验收](13-HUMAN-REVIEW.md) 汇总五个实际生成并保存的结果。
在收到实际人工结论前，ticket13 的人工条款和 ticket19 的前置验收条款保持未完成。
已有 AI 评审不代替人工签字；此工作没有发布或部署。

Standards：7项均已处理，最高为P2；Spec：2项均已处理，最高为P2。
两轴均无已知未处理发现；人工质量验收单独保留。

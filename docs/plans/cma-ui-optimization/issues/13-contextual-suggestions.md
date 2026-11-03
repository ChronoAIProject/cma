# 13：生成并保存与当前回答匹配的后续建议

**What to build:** 用户完成任务后看到结合目标、当前回答和已确认成果的后续问题，刷新后建议稳定，生成失败也能继续手动对话。

**Blocked by:** 11（建议数据身份、发送与失效语义）。

**Status:** in-progress

**优先级:** P1

## Acceptance criteria

- [x] 在最终回答确认后异步生成，正文先显示；读取历史/GET 不启动新 Agent turn，不把建议文本作为用户指令执行。
- [x] 使用既有可授权模型服务或受控生成能力，输入限于该 owner/Agent/回答上下文和已确认资源；记录超时、用量和费用，不让生成占住主回答或无限重试。
- [x] 采用服务端保存结果方案：同一回答和生成策略版本最多一个有效建议集，稳定 ID/text/rank，刷新与返回读取同一结果；服务重启恢复生成状态或明确失败，不重复计费重生。
- [x] 最多三条、每条一个明确意图，去重并过滤空泛“继续”、矛盾内容及不存在的文件/能力；无法确认适用性时返回零条，不强行凑满。
- [x] 新消息已受理、回答被替代、账号或权限变化时旧结果失效；完成很晚的生成任务不能把建议写到另一回答或重新打开旧组。
- [x] 生成返回数据包含明确 answer scope，服务端校验身份，前端读取/发送再次校验；数据无效、生成失败和权限不足不阻塞正文与 composer。
- [x] 生成不可用时只可采用 11 中明确适用且标注来源的模板，或隐藏；动态能力完整验收必须有实际生成样本，不能用模板通过其验收。
- [ ] 典型样本覆盖比较、文档修改、需澄清、无合适建议及错误成果声明；质量由独立人工准则检查，不能以模型自己的评分作为唯一依据。
- [x] 编辑入口继续支持本组建议；指标区分有效曝光、发送确认、编辑后发送和后续任务结果，失败生成不计成功曝光。

## Testing boundaries（2026-11-03 已确认）

- **A/U：** 完成回答→异步生成状态→读取→展示→点击原文；用外部模型边界可控响应验证合法/重复/过期/错误资源、重试与刷新。不能证明真实模型的相关性。
- **真实生成样本：** 固定配置下跑有界任务样本，记录延迟/用量与人工判定，再经公开读取核实保存结果；不承诺所有任务都有好建议。
- **首个 red → green：** 回答 A 的生成尚未完成时用户已发送下一条消息，随后 A 的结果到达；公开读取及页面都不能显示它为当前可发送建议。

## 范围与依赖说明

12 与本票可并行。没有成果时仍生成基于回答的建议，因而不依赖 07；接入成果时只能使用既有授权事实。选择独立异步生成与持久读取，收敛设计中尚未决定的生成/持久化方式。

## Delivery checkpoint（2026-11-03）

Implementation, persistence, bounded real generation and focused A/U/browser evidence are delivered; see [13-EVIDENCE](../13-EVIDENCE.md) and [actual model samples](../13-MODEL-SAMPLES.md). The fixed rubric preceded sampling. Literal independent human/user quality signoff remains pending; implementation-agent judgments and any parent AI review are not human review. Actual provider billing cost is unknown and persisted as null; token usage and measured latency are retained without an invented monetary estimate.

Ticket 12's editor and ticket 10's feedback are merged through integration `f793a41518fa969af068d7913ff6310dee7e040b`. Generated sets use the existing append/replace/undo/cancel and normal-send recovery. Ticket 13 observes actual edited acceptance, uncertain-request recovery and receipt-bound outcomes through the shared metric seam; no duplicate editor or message protocol was introduced. The combined edit criterion is complete.

The [independent parent AI review](../13-PARENT-AI-REVIEW.md) passes all five actual v3 samples. This is explicitly AI review. Machine implementation and proof work are complete; status remains in-progress solely for literal human/user quality acceptance at the full-delivery review. Unknown actual billed cost remains an explicit evidence limitation, not a fabricated estimate or a reason to stall the remaining tickets.

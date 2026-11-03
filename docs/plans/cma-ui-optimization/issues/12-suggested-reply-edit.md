# 12：编辑建议时保留已有输入草稿

**What to build:** 用户可把建议放入输入框修改，已有文字与附件不会被静默覆盖，修改后仍走正常发送。

**Blocked by:** 11（建议身份、资格校验和正常发送集成）。

**Status:** in-progress

**优先级:** P1

## Acceptance criteria

- [ ] 每条建议提供可区分的“编辑后发送”动作；动作只编辑并聚焦输入框，不自动提交。
- [ ] 输入框为空时填入建议完整文案；已有草稿时显示保留原文追加、明确替换文字或取消的选择，取消不改变任何输入。
- [ ] 追加保留原文与换行；明确替换保留可撤回的旧文字，附件不因替换文字被静默删除；未决发送的原请求快照不被编辑动作覆盖。
- [ ] 用户修改后的正文作为普通消息发送，使用 11 复用的实际 admission 和 recovery；确认后才收起建议，不以放入 composer 作为发送成功。
- [ ] 发送失败保留实际编辑后的内容，晚到回执不能清空随后新输入；切 Agent/账号不会把建议或旧草稿注入另一个会话。
- [ ] 编辑期间来源回答失效时撤下建议关联，已输入文本保留为普通草稿；下次发送仍重新校验权限。
- [ ] 桌面/移动端的空草稿、有文字、有附件、冲突选择、发送失败都有清楚焦点路径和可访问状态。

## Testing boundaries（2026-11-03 已确认）

- **U：** 真正 composer 与建议的联合操作，覆盖追加/替换/撤回/取消及发送恢复；不能只 mock onEdit/onSend 回调证明内容保留。
- **A（复用 11）：** 最终发送在公开历史中等于编辑后的正文且归属正确；本票不新增消息传输协议。
- **首个 red → green：** 已有“保留预算上限”草稿和附件时点编辑建议并选择取消，文字、附件、焦点返回均保持原状。

## 范围与依赖说明

不等待动态生成，模板与动态建议共用同一编辑行为。原因表单草稿与 composer 草稿分开，不互相覆盖。

## Implementation checkpoint

2026-11-03: `codex/cma-ui-12`, isolated worktree `ticket-12`, starts at
integration `ef5e20c1f`. Approved seams are U (real AgentSurface and composer),
A (existing public normal admission/history/recovery), and browser interaction.
Ticket 13 owns generation/fetching; ticket 12 adds only the shared set's edit action
and composer draft handling. No new suggestion identity or analytics path.

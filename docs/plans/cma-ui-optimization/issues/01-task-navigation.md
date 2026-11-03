# 01：用任务名称与真实状态浏览 Agent

**What to build:** 用户在默认侧栏中找到任务、看懂是否正在工作或需要处理，并能从设置路径进入原有 Sandbox 管理。

**Blocked by:** None（评审确认后可独立开始）。

**Status:** in-progress

**Implementation:** `codex/cma-ui-01`, isolated `ticket-01` worktree. Approved U/A
seams; each behavior follows red → green. First U red observed 2026-11-03:
`TaskNavigation.test.tsx` could not find visible “Ready to start”; both ready
Agents previously exposed only the hidden lifecycle word “Ready”.

**优先级:** P0

## Acceptance criteria

- [ ] 默认导航以 Agent 任务为主，移除常规 Agents/Sandboxes 切换；新增设置中的环境管理入口，原有详情、深链接和上下文创建仍可访问。
- [ ] 使用现有标题优先级，空标题显示人类可读的“新任务/未命名任务”；内部 ID 留在详情，不作为正常任务名。
- [ ] 工作中、等你审批、准备中、暂停、停止、失败分别可辨；只有最后一轮明确成功且无新工作才显示“完成”，从未执行但 ready 的 Agent 显示“可开始”。
- [ ] 对非当前 Agent，列表需要的最后一轮结果通过已授权的列表投影提供；不逐条启动对话读取，不用本地猜测补齐状态。
- [ ] 保留 Personal/组织/共享范围、状态过滤、选中项、键盘导航与返回位置；父 Sandbox 不可用时，操作资格仍受原生命周期约束。
- [ ] 状态同时有文字与现有状态色；窄屏、长标题、空列表和读取失败可操作，失败不把已显示任务清空。

## Testing boundaries（2026-11-03 已确认）

- **U：** 任务列表→选择任务→设置中的 Sandbox→返回；验证名称、状态、过滤、可访问名称和焦点。不能证明服务端状态新鲜度。
- **A（若扩展列表投影）：** 不同 owner 的列表读取及最后一轮结果；验证权限和真实终态。不能证明页面视觉。
- **首个 red → green：** 一个从未运行的 ready Agent 和一个已成功完成的 ready Agent 同时出现时，页面分别显示“可开始”和“完成”。

## 范围与依赖说明

复用当前任务标题，自动命名补齐由 05 负责，两者没有先后依赖。视觉规范和新增导航规则随本票交付；不移除底层 Sandbox API 或管理能力。

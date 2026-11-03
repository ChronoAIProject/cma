# 15：观察 Agent 浏览器的真实实时画面

**What to build:** 用户在电脑面板看到 Agent 正在访问的浏览器，能够区分实时、断线和不支持状态。

**Blocked by:** 14（环境面板、能力作用范围与授权数据生命周期）。

**Status:** in-progress

**优先级:** P3

## Acceptance criteria

- [ ] 先验证固定 runtime 版本支持的浏览器观察方式，再实现对应公开能力；一条完整链路覆盖 runtime 帧/截图→受授权服务→前端画面，不能只做 UI fixture。
- [ ] 帧来源绑定实际 Agent、浏览器上下文和 runtime incarnation；同一 Sandbox 的另一 Agent 或重启后的旧环境不能继承画面访问资格。
- [ ] 对已启用浏览器的 Agent 默认选择 Browser tab；画面显示安全处理后的当前位置/观测时间，收到足够新鲜的真实帧才显示 Live。
- [ ] 明确定义帧新鲜度阈值、大小/频率上限和断线重连策略；超过阈值显示“最后画面/重连中”，不继续标 Live；恢复不会重开任务或重置浏览器页面。
- [ ] 无能力、未启用、启动中、已暂停、访问拒绝、流故障都有准确状态；切账号/Agent/权限撤销后立即停止读取，旧帧不能重新出现。
- [ ] 控制平面只转发经过授权的数据，不执行 Agent 浏览器工作；不向前端暴露运行时内部连接地址或凭据。
- [ ] 在正常、隐藏 tab、关闭面板和多个观察者下测量 CPU/内存/带宽及订阅数量，记录版本和有界使用预算；默认不为所有 Sandbox 增开桌面。
- [ ] 使用至少两个明确页面的实际导航证明画面随 Agent 行为改变，重复和延迟帧不能把画面倒退成伪实时。

## Testing boundaries（2026-11-03 已确认）

- **A/U：** 授权帧读取/订阅到页面，验证作用范围、断流、过期、乱序及禁止访问；可控帧只证明协议处理，不能证明真浏览器。
- **R：** 固定版本的隔离浏览器由 Agent 导航到已知页面，观察实际变化和重连，并记录资源成本；未通过则票保持未完成，不能以隐藏 tab 当交付成功。
- **首个 red → green：** Agent 将页面从 A 导航到 B，用户面板显示 B 的实际内容；断流后 Live 消失，恢复仍是同一浏览器上下文。

## 范围与依赖说明

本票使用一个明确已启用的测试环境验证观察能力；16 再把能力带入普通首页创建，17 再加入控制权。没有录像存储或操作执行入口。

## Implementation evidence (2026-11-03)

- Isolated branch `codex/cma-ui-15`, worktree `ticket-15`, starting at integration
  `826de20e95cf02aad86673003ca813279d33d9dd`.
- Approved seams: U (normal Agent computer panel), A (authenticated environment
  and browser observation), R (pinned native Agent navigation through the runtime
  browser mediator). Controlled frames prove protocol behavior only.
- First slice: native Agent navigates two local pages through a mediated browser
  command; real bounded frames follow that navigation and observation reconnection
  retains the same browser context. Production sandbox prerequisites are checked
  before claiming the capability.
- Detailed current evidence and exact immutable candidate identities:
  [`15-EVIDENCE.md`](../15-EVIDENCE.md). The actual native development navigation
  is preserved; all Mac CPU values are now explicitly unknown, with the invalid
  earlier artifact retained as superseded. Aggregate RSS is not browser working
  memory. Required Linux live-resource acceptance remains pending.
- 36 shared browser cases run through real public handlers and both typed SDK
  corpus dispatchers. The three browser operations are registered in the shared
  JSON inventory. The parent's separate 14/07 inventory repair is still pending
  integration; no missing operations are exempted.
- An own immutable Linux 15 runtime binary now proves actual native Agent
  command refusals through runtime HTTP/ACP with the pinned provider. Docker's
  available namespace policy is incompatible with Chromium startup; this is not
  a conclusion about production gVisor. Parent owns the independent runsc probe.
- Ticket 08 integration is merged, preserving its previews, the single computer
  panel selection structure, and both style appendices. Ticket remains
  **in-progress** until the complete Linux/public/Web chain and resource budget
  pass; no control/takeover capability is claimed.

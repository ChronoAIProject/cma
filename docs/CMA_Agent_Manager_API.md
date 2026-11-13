# CMA Agent Manager API 草案

状态：待定稿的目标契约，2026-10-12；尚未实现。服务边界见
[service split](CMA_Service_Split.md). Here, `/v1` is the new Agent Manager service root;
不修改既有 CMA `/api/v1` 或 `/api/v2` 的语义。统一 `web/` 的 Agent Manager 入口
通过 `/api/agent-manager/v1/...` 访问本服务；本服务独立注册 NyxID 和管理自己的
roles/permissions and does not access the Event Gateway database. See
[服务隔离方案](CMA_Service_Isolation.md)。

## 1. 最常用的三个请求

创建 Agent：

```http
POST /v1/agents
Authorization: Bearer <user-token>
Idempotency-Key: <create-request-id>
Content-Type: application/json
```

```json
{
  "name": "我的开发助手",
  "repository": {
    "url": "https://github.com/the-omega-institute/trureturing"
  }
}
```

返回 `202 Accepted`，`Location: /v1/agents/agt_example`：

```json
{
  "id": "agt_example",
  "name": "我的开发助手",
  "status": "creating",
  "revision": 1
}
```

发消息：

```http
POST /v1/agents/agt_example/messages
Idempotency-Key: <message-request-id>
Content-Type: application/json
```

```json
{
  "text": "阅读这个项目，告诉我怎么运行。"
}
```

返回 `202 Accepted`：

```json
{
  "id": "msg_example",
  "turn_id": "turn_example",
  "status": "queued"
}
```

收回复及状态：

```http
GET /v1/agents/agt_example/events
Accept: text/event-stream
```

以上请求使用同一用户认证。用户无需传 sandbox、workspace、worktree、pod、runtime
或 ACP 会话 ID。Agent 还在创建或处于暂停状态时，可以先接收消息并排队；暂停状态
下显式发送新消息会触发恢复。创建/恢复失败必须在 Agent 和关联 turn 中可见。

## 2. API 一览

| 方法 | 路径 | 用途 | 成功响应 |
|---|---|---|---|
| POST | `/v1/agents` | 创建 Agent | 202 Agent |
| GET | `/v1/agents` | 列出我的 Agent | 200 Page |
| GET | `/v1/agents/{id}` | 查看状态、有效配置和当前工作 | 200 Agent |
| PATCH | `/v1/agents/{id}` | 更新名称或 Agent profile | 200/202 Agent |
| POST | `/v1/agents/{id}/pause` | 保留工作并暂停 | 200/202 Agent |
| POST | `/v1/agents/{id}/resume` | 恢复原会话 | 200/202 Agent |
| DELETE | `/v1/agents/{id}` | 终止 Agent 并回收执行资源 | 200/202 Agent |
| POST | `/v1/agents/{id}/messages` | 开启下一轮；忙时排队 | 202 MessageReceipt |
| GET | `/v1/agents/{id}/messages` | 查看会话历史 | 200 MessagePage |
| GET | `/v1/agents/{id}/events` | SSE 状态与回复流 | 200 event stream |
| POST | `/v1/agents/{id}/steer` | 补充当前指定 turn 的指令 | 202 ControlReceipt |

以下两个辅助接口支持停止工作和 `guarded` 模式，普通聊天不需要调用：

| 方法 | 路径 | 用途 |
|---|---|---|
| POST | `/v1/agents/{id}/interrupt` | 取消指定的当前 turn，保留 Agent |
| POST | `/v1/agents/{id}/approvals/{approval_id}` | 回答一次执行审批 |

不增加供普通用户管理 command ledger、lease、runtime generation 的接口。
异步动作通过已有 Agent 状态和事件观察；SDK 自动管理幂等键与流重连。
统一前端通过 `GET /v1/me` 获取本服务的 subject、roles 和 effective permissions；
这份权限视图只代表 Agent Manager，不代表另外两个服务的授权。

## 3. 创建和配置

只有 `name` 是创建必填字段。以下是包含所有配置类别的示例，标记为 `<...>` 的值
需要替换成部署支持的模型、skill 和服务目录标识：

```json
{
  "name": "我的开发助手",
  "repository": {
    "url": "https://github.com/example/project",
    "ref": "main"
  },
  "profile": {
    "model": "<supported-model>",
    "reasoning_effort": "medium",
    "permission_mode": "guarded",
    "system_prompt": "按项目约定开发，每次改动后运行相关检查。",
    "skills": [
      {"id": "<ornn-skill-id>", "version": "<immutable-version>"}
    ],
    "permissions": {
      "services": ["<github-service-id>"]
    }
  }
}
```

| 字段 | 省略时行为 | 契约 |
|---|---|---|
| `name` | 必填 | 1–80 个 UTF-8 字节的可读名称 |
| `repository` | 创建空 git repo | GitHub HTTPS URL；`ref` 省略则使用默认分支；记录实际 resolved commit |
| `profile` | 使用服务默认配置 | 内联配置，无需创建三种 Profile 资源 |
| `model` | 固化创建时的默认模型 | 经过当前 provider 能力验证的模型名 |
| `reasoning_effort` | 固化当前默认值 | 沿用现有 `minimal` / `low` / `medium` / `high` / `x_high`；所选模型需支持 |
| `permission_mode` | `guarded` | `guarded` 或 `autonomous`，保持现有模式含义 |
| `system_prompt` | 空字符串 | 最多 32 KiB；作为 Agent 常驻指令，不覆盖仓库指令文件 |
| `skills` | 空数组 | 最多 32 项；省略 version 时在创建阶段解析并固定版本；不静默丢弃失败项 |
| `permissions.services` | 空数组 | 用户授权的额外 NyxID 服务目录 ID，最多 64 项；最小运行所需授权另外明确记录 |

`permission_mode` 控制 Codex 的执行审批和内层执行限制；`permissions.services`
控制外部服务访问。选择 `autonomous` 不会授予额外 NyxID 服务权限。

权限以当前用户实际授权为上限；缺少授权返回 `connection_required` 和需要连接的
服务信息。创建请求不接收原始 NyxID key、GitHub token 或 Kubernetes 凭证。
实例 GET 返回解析后的 profile 和 `effective_permissions`，让调用者看到最终生效范围。

私有 GitHub repo 属于目标能力，需要通过 Agent Manager 的 GitHub/NyxID 绑定
验证后启用。现有实现只支持 public GitHub clone，不能在尚未实现时接受私有仓库
请求并声称创建成功。

环境配置归 Agent Manager：它直接把 CPU、memory 和持久卷挂载转成 agent-sandbox
Sandbox CRD 的 podTemplate。默认配置服务普通 Agent 创建；高级配置属于可选的
`environment` resource settings, without introducing a Sandbox Manager. New creation
uses only the system default image. The public `image` field remains for compatibility,
but custom image selection is rejected with `422 image_selection_unsupported` before
persistence or side effects. The compatibility API still selects CPU/memory through Sandbox
Profiles and the default image and persistent home PVC through deployment policy.
The simplified creation facade remains a target contract, not a deployed API.

## 4. Agent 读取与状态

完整 Agent 响应的形状如下；创建受理时可返回第 1 节的最小形状，GET 返回完整对象。

```json
{
  "id": "agt_example",
  "name": "我的开发助手",
  "status": "working",
  "revision": 8,
  "repository": {
    "url": "https://github.com/example/project",
    "ref": "main",
    "resolved_commit": "<resolved-commit>"
  },
  "profile": {
    "model": "<resolved-model>",
    "reasoning_effort": "medium",
    "permission_mode": "guarded",
    "system_prompt": "",
    "skills": [],
    "permissions": {"services": []}
  },
  "profile_revision": 1,
  "effective_permissions": {"services": ["<required-model-service-id>"]},
  "active_turn_id": "turn_example",
  "pending_messages": 0,
  "created_at": "2026-10-12T03:00:00Z",
  "updated_at": "2026-10-12T03:01:00Z",
  "error": null
}
```

`status` 使用：`creating`、`ready`、`working`、`waiting_for_approval`、`updating`、
`pausing`、`paused`、`resuming`、`terminating`、`terminated`、`failed`。
空仓库的 `repository` 为 `null`。没有当前 turn 时 `active_turn_id` 为 `null`。
调用者必须容忍未来新增的状态值；未知状态不能当作 ready。

`GET /v1/agents?limit=50&cursor=...` 返回 `{items, next_cursor}`，默认 limit 50，
上限 200，可按 `status` 过滤。权限来自认证身份，不允许用 `owner` 查询参数越权。
具体 agent 的不存在或无权访问统一返回 404。

## 5. 更新、暂停、恢复、终止

更新示例：

```http
PATCH /v1/agents/agt_example
Idempotency-Key: <profile-update-id>
Content-Type: application/json
```

```json
{
  "profile": {
    "reasoning_effort": "high",
    "system_prompt": "先运行测试，再开始修改。"
  }
}
```

这是按字段更新：缺省字段保持原值，数组整体替换，未知字段拒绝。`repository` 是
创建来源，不允许借 PATCH 切换并覆盖工作目录。只改名称可立即生效。

profile 更新只在 `ready` 或 `paused` 状态受理，且不能有已排队消息；否则返回
409 `agent_busy`。从受理到完成期间显示 `updating`，不投递新 turn。先验证所有字段、
skills 和 NyxID 授权，再用新 profile revision 准备运行时；成功后整体发布。
出现失败不报告新配置已生效；若无法安全恢复旧授权/进程，保持停止并显示错误。
不能把已撤销的权限通过“回滚”重新打开。每个新 turn 记录其生效的 profile revision。

更新要求原会话可继续使用；需要新建会话才能支持的变更明确返回
`profile_update_unsupported`。当前仓库尚无完整 profile 更新能力，实施时须补齐
runtime 配置、skill 替换、授权变更及失败恢复的验证。

生命周期动作无请求体，行为如下：

| 动作 | 行为 |
|---|---|
| `pause` | 持久化暂停意图，停止派发排队消息，等待当前 turn 安全结束，再保留文件和原会话并暂停；排队消息留待恢复 |
| `resume` | 恢复原文件、profile、会话身份并继续排队工作；已 ready/working 时返回当前状态 |
| `DELETE` | 持久化终止意图，取消待执行消息，显式取消当前 turn，停止进程并清理该 Agent 独占的执行资源；完成后 `terminated` |

暂停时若正在等待审批，仍允许回答该审批或 interrupt；状态中保留 active turn 和
待审批事件，不能把“暂停已受理”显示成“已暂停”。`pausing`、`updating`、
`terminating` 阶段的新消息返回 409，不接收可能无法投递的输入。

终止后不能 resume。删除 pod/PV/snapshot 的范围只限该 Agent 拥有的资源，并遵循
明确的删除保留策略；已有共享 sandbox 需要按所有权保留其他 Agent 的资源。
消息和审计记录保留期按服务策略执行，终止不是立即清除全部历史的别名。
GET 可在历史保留期内读取 terminated Agent；之后返回 404。

服务重启后继续未完成的动作；单纯更换镜像默认值不能重启正在运行的 Agent。

## 6. 消息、steer、interrupt、approval

`POST .../messages` 的 `text` 必须非空，首版最多 32 KiB，其他内容类型待能力验证
后以可选字段扩展。空闲时启动新 turn；工作中按受理次序排队。一个 Agent 同时最多
一个执行中的 turn，queue 上限默认 32，超限返回 429 `queue_full`。每条新消息受理时
分配稳定 message ID 和 turn ID，重试返回原标识。

steer 使用当前 turn 的 ID，防止在网络延迟后误作用到下一轮：

```http
POST /v1/agents/agt_example/steer
Idempotency-Key: <steer-request-id>
Content-Type: application/json
```

```json
{
  "turn_id": "turn_example",
  "text": "先只看认证模块。"
}
```

返回 `202 {"id":"ctl_example","turn_id":"turn_example","status":"accepted"}`。
只补充该 turn，不创建下一轮；指定 turn 已结束则返回 409 `turn_not_active`。
provider 不支持 steer 时返回 422 `steer_unsupported`，不得静默退化为新消息。

`POST .../interrupt` 接受 `{"turn_id":"turn_example"}`，返回同形状的控制收据。
取消仅作用于该 turn，完成状态通过事件确认；后续已排队消息仍按顺序执行。
如果希望所有工作暂停，先 pause，再 interrupt 当前 turn。

`POST .../approvals/{approval_id}` 接受 `{"decision":"allow"}`；decision 可为
`allow`、`deny`、`abort`，分别映射现有批准、拒绝和中止语义。审批必须属于该 Agent
当前有效请求；过期或已经以其他答案解决返回 409。相同幂等请求返回原结果。

## 7. 历史与 SSE

`GET .../messages?limit=50&cursor=...` 使用默认 50、上限 200，返回：

```json
{
  "items": [
    {
      "id": "msg_example",
      "turn_id": "turn_example",
      "role": "user",
      "text": "阅读这个项目，告诉我怎么运行。",
      "status": "completed",
      "created_at": "2026-10-12T03:01:00Z"
    }
  ],
  "next_cursor": null,
  "event_cursor": "42"
}
```

首次无 cursor 请求返回最近一页，页内按会话顺序排列；`next_cursor` 用于读取更早
历史。客户端可用首页返回的 `event_cursor` 接上实时流；此 cursor 和该历史投影必须
来自同一一致性位置，避免漏掉历史读取期间的新输出。

SSE 示例：

```text
id: 43
event: message.delta
data: {"agent_id":"agt_example","turn_id":"turn_example","message_id":"msg_reply","delta":"项目的入口是"}

id: 44
event: turn.completed
data: {"agent_id":"agt_example","turn_id":"turn_example"}
```

事件类型包括 `agent.updated`、`message.created`、`message.delta`、`message.completed`、
`turn.started`、`turn.completed`、`turn.failed`、`turn.cancelled`、
`approval.requested`、`approval.resolved`。正文至少包含 `agent_id`，turn/message
相关事件另带对应 ID。`message.completed` 包含完整最终文本，可替换客户端累计文本。
审批事件包含 approval ID、turn ID、请求说明和允许的决定。

首版目标是对外带 `id` 的事件均先持久化再推送，包括合并后的文本 delta。每个 Agent
的事件 ID 单调递增；重连使用 `Last-Event-ID` 或 `after`，两者同时出现且不同返回
400。无游标从当前保留窗口起点回放。采用至少一次投递，客户端按事件 ID 去重；
keepalive 使用无 ID 的 SSE 注释。断开观察不会取消工作。

游标已过保留窗口返回 410 `event_cursor_expired`，客户端重新读历史后接流。
不得在断线后无提示跳过缺失事件。实现时需要给当前临时 delta 增加持久化路径，
现有 WebSocket acknowledgement 本身不等同于这项新公开回放保证。

## 8. 幂等、认证与错误

所有写操作支持 `Idempotency-Key`，建议 SDK 默认必带。命令的 key 范围为服务端
授权身份、目标资源和操作；同 key 同规范化请求返回原受理结果，同 key 不同请求返回
409。独立 API bearer 的授权身份绑定具体凭据的不可变 custody 记录；相同用户换用
另一把 Key、改用 OAuth 或刷新访问令牌后，不能假设仍处于原命令的幂等范围。结果
未知时应保留原凭据、原内容和原 key；原凭据已不可用时先读取并核对现有结果，
不要用新凭据直接重发。此规则不承诺跨凭据去重。
同一请求的已受理结果跨重启保留至少到关联资源/消息的历史保留期结束；返回
`Idempotency-Key-Expires-At` 标明最早淘汰时间。没有 key 的写请求只能被视作新操作，
SDK 不能在结果未知时生成新 key 重试。GET Agent 提供 ETag；PATCH 可带 `If-Match`
防止覆盖旧视图，失败返回 412。没有 If-Match 时按服务受理顺序更新所提交的字段。

认证支持用户 NyxID token 和明确委派的 API 身份。浏览器使用既有登录会话时，SSE
沿用同源认证；token 客户端使用 Authorization header，不把 token 放在 URL 中。
所有写入必须完成鉴权、参数校验和持久化受理后才能返回 2xx。

错误沿用 RFC 9457 `application/problem+json`：

```json
{
  "type": "urn:cma:problem:agent-busy",
  "title": "Agent is busy",
  "status": 409,
  "code": "agent_busy",
  "detail": "Pause the agent and finish or cancel its current work before updating its profile.",
  "request_id": "req_example"
}
```

| HTTP | 典型 code | 调用者行为 |
|---|---|---|
| 400 | `invalid_request` | 修正格式或冲突的游标 |
| 401 | `unauthenticated` | 重新认证 |
| 403 | `permission_denied` | 申请必要授权 |
| 404 | `agent_not_found` | 资源不存在或不可访问 |
| 409 | `agent_busy` / `turn_not_active` / `idempotency_conflict` | 读取当前状态后决定下一步 |
| 410 | `event_cursor_expired` | 重新加载历史 |
| 412 | `revision_conflict` | 重新读取后更新 |
| 422 | `unsupported_profile` / `profile_update_unsupported` / `connection_required` / `steer_unsupported` | 调整配置或完成所指服务连接 |
| 429 | `queue_full` / `rate_limited` | 按 Retry-After 重试 |
| 503 | `temporarily_unavailable` | 结果未知时用原幂等键恢复 |

异步失败记录在 Agent/turn 及事件里，使用同样的 code/detail，不把后台错误伪装成
成功回复。不需要让用户轮询内部 operation 表。

## 9. 兼容与实施边界

AG-UI、Responses、CMAEG provider binding 继续作为同一 Agent domain 的协议适配器。
它们不能绕过新 API 所用的鉴权、命令排序和幂等机制。已有 Agent ID、历史、权限和
native conversation 原地保留，旧 URL 通过兼容层映射，不依赖重新创建 Agent。

本文包含尚未完成的能力：一请求创建完整 Agent、profile 更新、独立 Agent 暂停、
新 SSE 契约及私有仓库绑定。API 定稿后应由实现生成 OpenAPI 与 SDK，再通过实际
provider/runtime 的创建、恢复、steer、权限变更及重连测试验证；不能把本文当作
当前服务的能力声明。

## Live Agent skills

Owner-scoped `GET` and idempotent `POST /api/v1/agents/{id}/skills` list confirmed
installations and install one exact Ornn skill version into an idle Agent.
See [Live Agent skill installation](CMA_Agent_Skills_Runtime.md) for request/response
shapes, runtime support gating, durable confirmation, and rollout order.

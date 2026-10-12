# CMA Sandbox Manager API 草案

状态：待定稿的目标契约，2026-10-12；尚未实现。见
[服务拆分](CMA_Service_Split.md) 和 [身份、数据库及统一前端](CMA_Service_Isolation.md)。
本 API 面向统一 CMA 前端的 Sandbox Manager 入口和任意获得授权的应用、CLI。
Agent Manager、Nyx Machine 是普通调用方，不是服务的前置依赖。服务根路径为 `/v1`。

## 1. 资源模型

Sandbox 是稳定的运行环境资源，Pod 是它某一代运行实例。暂停恢复、更换损坏的 pod
不能改变 Sandbox ID。Sandbox Manager 独占 pod、PV、snapshot、运行环境元数据及
操作收据的写入权，不能读取或写入 Agent Manager 的任何 collection。

一个 sandbox 是通用容器运行环境。用户可选自己的 OCI 镜像及启动命令，不要求
镜像包含 Agent、CMA supervisor、WebSocket server、NyxID CLI 或 Chromium。
服务不默认克隆仓库、创建 git workspace 或安装 Agent；这些属于调用方的用例。
请求不包含 Agent ID、Codex model、prompt 或 conversation。
Agent Manager 在自己的数据库中保存 Agent 到 sandbox 的映射；
Sandbox Manager 可以保存调用方给出的 opaque `client_ref`，只用于关联和审计。

## 2. 创建

```http
POST /v1/sandboxes
Authorization: Bearer <sandbox-manager-authorized-token>
Idempotency-Key: <create-request-id>
Content-Type: application/json
```

```json
{
  "name": "python-environment",
  "image": "python:3.13-slim",
  "entrypoint": ["python", "-m", "http.server", "8080"],
  "resources": {"cpu_millis": 500, "memory_mib": 512},
  "env": {"APP_ENV": "development"},
  "client_ref": "example-application"
}
```

响应为 `202 Accepted` 的 Sandbox 对象，带 Location、稳定 ID 和 `creating` 状态。
长步骤在持久化 worker 中执行，HTTP 超时不等于创建失败。重试原幂等键必须恢复
原创建，不再创建另一个 pod/PV。

| 字段 | 契约 |
|---|---|
| `name` | 必填的可读名称 |
| `image` | 必填；用户自己的 OCI 镜像，可指定 tag 或 digest；实际运行实例记录解析后的 digest，恢复不静默跟随 tag 漂移；不限制为 CMA/Agent 镜像 |
| `entrypoint` | 可选；容器启动 argv，省略时使用镜像默认命令；不自动注入 supervisor 或 Agent |
| `resources` | 必填；正数 CPU millicores 和 MiB 内存，受服务配额约束 |
| `storage` | 可选；显式声明新卷或已有 `volume_id` 及挂载路径；省略时不隐式创建 `/home/cma` 或 git 仓库；不能传任意 PVC 名 |
| `install_commands` | 可选的通用初始化命令，默认空；执行结果可查询，不隐式安装 Codex、NyxID CLI 或 Chromium |
| `env` | 默认空；调用者的普通容器环境变量；允许 HOME/PATH 等用例配置；敏感值通过独立授权 secret 引用交付 |
| `client_ref` | 可选，opaque 审计关联值，不授予权限 |

已有卷的 storage 通过 `volume_id` 引用，与创建新卷的 `size_gib` 互斥，须验证所有权
及写入租约。挂载路径由用户显式指定并经校验。创建参数不可直接指定 Kubernetes
namespace、service account 或 pod UID。

普通容器的启动不依赖 NyxID CLI、GitHub 连接或 Agent 握手。需要这些能力的用例
由调用方通过镜像和初始化/exec API 显式配置。Agent Manager 在自己的编排中保持
无凭证安装、验证授权、交付凭证、准备 repository、启动 Agent 的顺序。
Agent Manager 不向这里提交其他用户的 owner ID；
代用户创建需要服务身份和可验证的所有者委派，详见隔离方案。

## 3. API 范围

| 方法 | 路径 | 用途 |
|---|---|---|
| POST / GET | `/v1/sandboxes` | 创建/列出可访问的 sandbox |
| GET / DELETE | `/v1/sandboxes/{id}` | 读元数据/终止并回收资源 |
| POST | `/v1/sandboxes/{id}/pause` | 安全停止运行，保留可恢复文件 |
| POST | `/v1/sandboxes/{id}/resume` | 恢复同一 sandbox |
| GET | `/v1/sandboxes/{id}/events` | SSE 生命周期、资源、操作进度 |
| POST / GET | `/v1/sandboxes/{id}/snapshots` | 创建/列出一致性文件快照 |
| POST | `/v1/sandboxes/{id}/restore` | 从该资源可访问的已完成 snapshot 恢复 |
| GET / DELETE | `/v1/snapshots/{snapshot_id}` | 查看/清理可访问的快照 |
| POST / GET | `/v1/sandboxes/{id}/exec` | 创建命令执行/查询该环境的执行记录 |
| GET | `/v1/sandboxes/{id}/exec/{exec_id}` | 命令状态、退出码和输出位置 |
| GET | `/v1/sandboxes/{id}/exec/{exec_id}/stream` | WebSocket：stdin/stdout/stderr/exit；TTY 模式支持 resize |
| POST | `/v1/sandboxes/{id}/exec/{exec_id}/cancel` | 停止该次命令执行 |
| GET / PUT / DELETE | `/v1/sandboxes/{id}/files?path=...` | 列目录或读文件/创建替换/删除文件或空目录 |
| GET / PATCH | `/v1/sandboxes/{id}/env` | 读/修改后续进程使用的非敏感环境变量 |
| PUT | `/v1/sandboxes/{id}/secrets/{binding_id}` | 交付该环境可访问的授权 secret 引用，不要求容器使用某种身份 CLI |
| POST | `/v1/sandboxes/{id}/connections` | 授权连接容器中显式开放的端口，可承载 HTTP/WebSocket/TCP 等用例 |
| POST / GET | `/v1/warm-pools` | 管理者创建/列出环境预热池 |
| GET / PATCH / DELETE | `/v1/warm-pools/{pool_id}` | 查看/调整/停止预热池 |

`exec`、files 和 env 对应绿色卡片的 interaction。SSH 保留待定。
所有读取和写入均检查 Sandbox Manager 自己的 permission、role 及资源访问权。
委派创建的 sandbox 按对应的委派边界开放控制权；管理者操作也必须经过运行租约/
fence，不得越过正在使用环境的调用方直接暂停或恢复底层文件。

## 4. 元数据、状态和异步结果

```json
{
  "id": "sbx_example",
  "name": "python-environment",
  "status": "ready",
  "revision": 5,
  "generation": 1,
  "image": {"selector": "python:3.13-slim", "digest": "<resolved-digest>"},
  "resources": {"cpu_millis": 500, "memory_mib": 512},
  "mounts": [],
  "env_revision": 1,
  "capabilities": {"exec": true, "terminal": true, "files": true, "snapshot": true},
  "created_at": "2026-10-12T03:00:00Z",
  "error": null
}
```

示例中的 capabilities 是目标实例响应；实际值必须来自所选后端/镜像已验证能力。
不能仅因 API route 存在就返回 true。
状态包括 `creating`、`ready`、`pausing`、`paused`、`resuming`、`restoring`、
`terminating`、`terminated`、`failed`。代数递增表示运行实例变更，并用于阻止旧请求
作用到新 pod。普通 metadata 不包含可重用 secret 或上游 provider 管理地址。

异步子操作返回 `202 {"operation_id":"op_example","status":"pending"}` 和
Location，使用 `GET /v1/operations/{operation_id}` 查询。terminal 状态为 `succeeded`、
`failed` 或 `cancelled`；必须区分“命令已提交”和“效果已确认”。这些是 Sandbox
Manager 的操作记录，不能读取 Agent Manager 的操作表。

## 5. exec、terminal、files、env

普通命令：

```json
{
  "argv": ["git", "status", "--short"],
  "cwd": "/workspace",
  "tty": false,
  "timeout_seconds": 60
}
```

terminal 使用 `tty:true`、明确的 shell argv 和初始 `cols`/`rows`。通过 Kubernetes
exec/attach 与目标容器通信；运行进程始终是 sandbox 用户。非 TTY 使用独立 stdout/
stderr 通道，TTY 输出合并。stream 子协议及 frame 格式需在实现前另行固定，不暴露
Kubernetes 原始凭证。exec_id 绑定 owner、sandbox、generation、命令参数及幂等请求。
普通非交互执行在观察连接断开后继续，结果有界保存；交互 TTY 断线按明确返回的
detach policy 管理，不能假装拥有未实现的无限重连能力。

files 的 path 是允许工作根目录内的路径。GET 文件返回字节和 ETag；GET 目录返回
`{entries:[{name,type,size}], next_cursor}`。PUT 用原始 bytes 写文件，使用临时文件
加原子替换；可选 `If-Match` 防止覆盖变更，目录创建使用专门的 JSON
`{"type":"directory"}` 和 `Content-Type: application/json`，通过查询
`kind=directory` 区分原始文件内容。DELETE 默认只允许文件/空目录，首版不提供递归
删除。绝对越界、`..` 和通过 symlink 逃逸的路径必须拒绝。文件大小/目录分页限制
由契约给出并在请求前校验。文件写入同样受 writer fence 控制。

env PATCH 使用 `{"set":{"APP_ENV":"test"},"unset":["OLD_NAME"]}`，返回
`{revision, values}`。unset 删除、set 替换，重复冲突的键拒绝。更改只影响后续启动
的进程；操作系统不能从外部修改已运行进程的环境。运行中进程重启必须由该进程的
所有者单独编排，不能因 env PATCH 隐式重启容器内应用。敏感凭证始终通过 secret
绑定交付，GET env 不返回其原值。

## 6. 快照、终止与通用运行时通道

snapshot 是一致性的持久化文件/PV 快照，不承诺进程内存 checkpoint。请求可选 name，
返回 snapshot ID、状态、来源 sandbox/generation、校验值和创建时间；只有完成的
snapshot 可以 restore。restore 请求体为 `{"snapshot_id":"snp_example"}`，必须
验证 snapshot 归属、完整性和资源兼容性，并在运行写入者已停止的状态执行。

快照需要冻结容器内进程、exec、terminal 和 API 文件写入等全部写入者；无法取得有效
fence 就返回冲突并继续保留原工作，不能发布不一致快照。只在新快照完整提交后清理
旧快照。保留策略、S3/object-store 生命周期及删除任务由本服务独立管理。

终止操作由有效控制权持有者发起，先阻止新写入，再停止进程、撤销身份及清理资源。
明确区分停止 pod 和删除 PV/snapshot 的策略；响应提供已受理的保留/删除策略，
完成状态必须等到对应清理有收据后才能发布。其他调用方的共享资源不得被删除。

connections 创建请求示例是 `{"port":8080,"protocol":"http","generation":1}`，
仅连接该 sandbox 显式开放的端口。返回代理 URL、连接 ID、有效期和认证方式；URL
不包含 bearer secret。Sandbox Manager 校验调用身份、委派权限、generation 和端口
范围，不解释目标应用协议。HTTP server、WebSocket bridge、Chromium 调试端口等
都通过同一通用连接机制使用；不能接受指向其他环境或集群服务的任意内部 URL。
现有 Agent 用例选择自己的 supervisor 端口并由 Agent Manager 处理 ACP 内容。

## 7. Warm pool 和运行租约

pool 配置固定 image digest、资源、存储初始化及无凭证安装配方。创建或 PATCH 接受
`{name, template, target_size}`。prepared sandbox 保持无所有者凭证，领取时原子地
绑定 owner/request，再执行调用者显式指定的初始化和 secret 交付。禁止将带有上一个用户数据
或 token 的 sandbox 放回公共 pool。

包括 Agent Manager、Nyx Machine 在内的调用方通过使用租约协调恢复、写入和暂停。
租约由 Sandbox Manager 自己的 collection 保存，包含 holder、generation、fence 和
到期时间。控制面连接丢失或续约未知要先恢复/确认实际进程状态，不能把超时直接当成
可强杀工作环境的证明。租约、snapshot fence 和收据的内部接口需要与独立服务实现
一起验证；公开用户无需管理它们。

## 8. 绿色卡片覆盖与现状

| 卡片要求 | 本契约位置 | 当前仓库依据/新增工作 |
|---|---|---|
| image selector、CPU/memory、PV | 创建字段 | 现有 provider/PV 代码可抽出；需独立身份、schema 和 API |
| workspace：GitHub、pod 目录 | 调用方用例，通过通用 exec/files/volumes 组合 | 仓库准备归 Agent Manager 或调用方；Sandbox Manager 不默认创建 git workspace |
| software installation | install_commands | 保留安装收据及先安装后交付凭证的顺序 |
| NyxID scoped identity、gh binding | 调用方用例，通过镜像、secret 引用和 exec 组合 | 管理 API 使用 NyxID 鉴权；容器内 NyxID CLI/gh 安装及使用不是基础服务要求 |
| env vars / env CRUD | env | 创建已有；动态修订和后续进程行为需要新增实现 |
| pod snapshots、metadata、termination | snapshots/read/delete | 现有恢复/finality 代码需保留其身份和 fencing 证明 |
| warm pool | warm-pools | 从现有 warm sandbox worker 中拆出 |
| Kubernetes exec、terminal | exec/stream | 当前公开接口明确不支持 terminal，属于新增能力 |
| filesystem CRUD | files | 需要通用、受授权及 fencing 的文件接口 |
| SSH | 待定 | 不作为首版已支持能力 |

错误使用 RFC 9457 和稳定 code；写操作要求调用方提供可重试的幂等键。新增能力须对
真实 Kubernetes/runtime 做集成验证，包括 DB 跨服务拒绝、过期 generation、连接
恢复、授权撤销、writer fence 及部分清理失败。

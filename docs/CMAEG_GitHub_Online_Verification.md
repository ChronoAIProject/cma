# CMAEG GitHub 首轮线上验收

日期：2026-11-04，Asia/Singapore。

**接入方案已重设计；第 1–3 切片已在本地完成，尚未发布。** 以 [动态 GitHub App integration 设计](CMAEG_GitHub_Integration_Design.md) 为准。此前要求创建 `cmaeg-github` Kubernetes Secret、填写 `CMAEG_GITHUB_CONFIG_FILE`、配置静态 proof/routes 的操作已撤回。不要据此修改集群 ConfigMap、Secret、volume 或为试点 channel 添加环境变量。安装绑定与执行资格的动态控制面仍待实现，这一缺口不能靠部署配置绕过。

这份 runbook 验证首轮闭环：GitHub App webhook → CMAEG 原生接收 → installation/tenant 路由 → 专用 goal agent → chrono-goals 完整同步 → 状态 PR。它不验证 installation token broker，也不把组织安装自动转换成 `projects.yaml` 项目。

**主聊天实施进度（2026-11-04）：** 已实现动态注册/列表/读取、版本化名称更新、write-only webhook secret 轮换、管理员 App 身份核实，以及签名 ping 与持久回执查询。测试通过实际编译的服务进程、真实临时 Mongo 副本集和外部身份服务 fixture 验证跨实例、重启、幂等冲突、租户隔离和组织撤权。App 身份核实只证明控制面管理决定，不能触发 agent。App signing credential、长期 installation 绑定、自动核验、动态路由和 UI 仍待后续切片。此次没有部署，也没有重查线上版本。

2026-11-04 此前检查时，公开线上 OpenAPI 可以读取，但没有 GitHub 路径或 `github` tag；该结果不能作为后续部署状态的证明。部署后先重新检查 OpenAPI，再执行下面的带凭证验收。

## 第一个线上验收点：第 3 切片后

这一轮验证“平台能安全配置 App、接住签名 ping 并查回结果”。真实 issue/PR → agent → 目标板 PR 属于后续闭环；当前不启用业务自动处理。

用户于 2026-11-04 补充发布契约：schema 与必要的数据变更跟随功能代码，由 feature flag 控制启用；开关打开后，服务启动逻辑自动完成数据库更新。不建立独立迁移脚本目录，不要求 Shining 手动执行 `cmaeg --migrate-automation`。此前手动迁移交接步骤撤回。新增 App 仍通过管理接口完成，不需要改部署或重启。

**启动发布适配已在本地实现，尚未部署：** 全局功能开关 `CMAEG_GITHUB_ENABLED` 默认 `false`，只接受 `true`/`false`，在服务启动时读取。开启后自动准备缺失或版本 1 的 integration 存储，最后写入版本 2 标记并验证运行权限及非 TTL 索引；已有版本 2 不重复执行 DDL。它不是逐 App/channel 配置，也不是管理 API 热切换。沿用现有发布流程让开关在 CMAEG 进程启动时生效；不新增 GitHub 专属 Kubernetes manifest。原显式迁移命令仅保留为运维工具，不是正常发布前置步骤。

本轮本地验证通过 70 项相关测试，包括 23 项 integration 进程测试（其中六项新增启动测试），以及接收、Mongo 权限隔离、OpenAPI 和配置结构兼容回归。测试证明默认关闭不初始化、开启自动初始化、旧版本升级、重启保留 App/receipt、并发启动、部分迁移失败恢复和未知版本拒绝；没有验证线上账号权限或实际部署开关状态。

### 按现有 develop → Shining 上线流程交接

用户于 2026-11-04 确认：代码合并到 `develop` 后，由 Shining 周期性发布上线，沿用现有 feature flag 和服务启动自动更新数据库的流程。仓库内的镜像发布 workflow 不等于 Shining 的线上部署执行器；镜像发布成功也不能证明功能已启用或数据库初始化成功。

- 用户：将本次改动按现有流程合入 `develop`，把本节交给 Shining；不需要自行获取数据库迁移权限、进入 Pod 或提供数据库密码。
- 开发侧：把版本化、可重复执行的数据库初始化接入功能开关控制的服务启动逻辑，提供已验证的开关使用方式、失败语义和验收契约。迁移逻辑留在服务代码，不新增脚本目录、另一套部署系统或 App 专属 Kubernetes 配置。本次没有代用户向 Shining 发送消息。
- Shining：确认部署包含新版 **CMAEG**，按既有流程管理功能开关并使启动配置生效，观察服务自动初始化结果，回传实际部署版本。正常上线不新增手动迁移步骤。

启动迁移沿用既有数据库连接和授权方式；程序不自动创建或授予数据库角色。线上现有账号的有效权限尚未核实，不能把“自动执行”当成“自动获得权限”。只有实际报告权限不足时，才交由现有数据库管理流程处理；不预设每次发布都要单独新建迁移账号或授权。仓库 CMAEG 的 `shared-schema` 与 `isolated` 权限契约仍需保留，不能为启动迁移删掉权限校验或放宽所有服务的权限。

开关关闭时，不执行本功能的 schema/data 更新、不接收新的 GitHub 工作；保留已有记录。开关开启后，初始化成功并通过权限、schema 和索引检查，才能报告 GitHub 可用。初始化失败保持 GitHub 不可用并记录脱敏原因，不影响原有 channel；修复原因后再次启动可继续，不能手改版本标记绕过。回关不删除数据或降级 schema，重新启用后应能读取原 App/receipt。上线后使用获准身份检查 `/v1/github/capabilities` 的 `store_ready: true`，再做 ping 验收。发布和线上初始化均尚未发生；App secret 上线后通过管理接口配置。

可转交给 Shining 的简短说明：

> 本次功能沿用现有发布方式：部署包含启动适配的新版 CMAEG，通过 `CMAEG_GITHUB_ENABLED=true` 启用，并让该配置在服务启动时生效。服务自动初始化数据库，无额外手动迁移步骤。请回传实际部署版本和开关状态；开启后确认获准查询 `/v1/github/capabilities` 时 `store_ready` 为 true。初始化失败查看脱敏日志、修复原因后重新启动，不手改版本标记。本轮只验收 App 管理和签名 ping/回执，尚不启动 goal agent。

### 上线后的接口验收

| 步骤 | 操作与通过条件 |
| --- | --- |
| 1. 确认发布契约 | 读取线上 OpenAPI，存在 App 的 POST/GET/PATCH、`webhook-secret` PUT、`verification` POST、webhook 和两种 receipt GET 路径。查询 `/v1/github/capabilities`：`store_ready`、`dynamic_app_registration`、`native_app_webhook`、`webhook_secret_rotation`、`app_identity_verification` 为 true；installation `ownership_verification` 和 `automatic_reconciliation` 仍为 false。 |
| 2. 注册试点 App | 使用获准的 CMAEG owner 身份，以稳定 `Idempotency-Key` 注册 App `5606750` 和显示名称，保存返回的 `app_ref` 与 `webhook_url`。初始状态必须是 `pending_verification`。可先不带 secret，通过独立 PUT 配置。 |
| 3. 安全配置 webhook secret | 在受保护的本地 HTTP 客户端中向 PUT 提交 `expected_version` 和 `webhook_secret`，将同一个值填入 GitHub App 设置。凭证不进入聊天、模型、命令参数、仓库或日志；这里不是上传 GitHub App private key。响应只能包含 presence/version/overlap 元数据。原生表单到第 21 切片才提供。 |
| 4. 发送并查询 ping | GitHub 使用返回的 URL 投递 ping；记录 GitHub delivery ID。期望 HTTP 200、`status: ignored`、`task_ids: []`。使用 receipt ID 或 App reference + delivery ID 查询，结果一致。ping 不要求 installation 绑定。 |
| 5. 验证重放和恢复 | GitHub 重投同一 delivery 返回同一 receipt；另一个在线实例和服务重启后仍可查到。错误签名返回 401，不创建 receipt；同一 delivery 的不同内容返回 409，不覆盖原回执。 |
| 6. 验证管理边界 | 其他 owner 读/改/轮换返回 404；普通 manager 核实 App 身份返回 403。获准的 `cma-eg:admin` 人员独立核实 App 管理归属后，携 `expected_version`、`evidence_ref` 和幂等键提交 verification；App 变为 `verified`，仍不能执行。组织操作必须通过当前组织 admin 身份核验。 |
| 7. 验证轮换 | PUT 默认保留旧版本 600 秒，最长可指定 3600 秒；响应显示旧版本退休时间。切换 GitHub 设置后，新 secret 可验签，旧版本到期后被拒绝；`overlap_seconds: 0` 明确立即替换。有效重叠期间不能继续增加第三个有效版本。重复旧操作不会恢复已退休凭证。 |

保留发布版本、开关状态、非秘密 `app_ref`、receipt/delivery ID、HTTP 状态和核实结果作为验收证据。若 `store_ready: false`，先检查开关是否生效、启动自动初始化结果和数据库权限；不退回静态 GitHub 配置。验收通过后继续第 4 切片的长期 installation 绑定，后续再接自动核验和业务事件执行。

业务 owner 已确认 `aevatarAI/GodGPT-Landing-Page` 停用并授权移出同步清单，本地
`chrono-goals/projects.yaml` 已移除该条目。修改后的本机 `gh` 预检查为源仓库 `14/14`
可读，目标板 `ChronoAIProject/chrono-goals` 元数据报告 `push` 权限，`npm run check`
通过；这不证明线上 CMA agent 的 NyxID 连接已经具备相同权限，后者必须在实际运行环境复验。
此前的 GitHub 404 本身不能证明仓库已删除；移除依据是业务 owner 的确认。

## 当前试点信息

本轮先安装到 `ChronoAIProject/cma`，通过单个仓库的事件验证接入；完整业务同步仍按
`projects.yaml` 执行。以下 App、组织和仓库公开元数据已通过本机 GitHub API 只读查询核实；installation ID 来自用户提供的安装页地址，仍需部署管理员核验关联关系：

| 字段 | 已确认的值 |
| --- | --- |
| App 名称 / slug | `ChronoAI CMA Events` / `chronoai-cma-events` |
| App ID | `5606750` |
| App 持有账号 | `yitianliu-ruby` |
| 试点组织 / account ID | `ChronoAIProject` / `253878174` |
| 试点仓库 / repository ID | `ChronoAIProject/cma` / `1330597394` |
| App 权限 | `issues: read`、`metadata: read`、`pull_requests: read` |
| App 事件订阅 | 查询时为 `[]`；启用投递前需配置订阅 |
| installation ID | `178446930`，来自用户提供的安装配置页地址 |

用户已确认仓库访问获批；本机凭证查询组织 installations 时返回需要 `admin:org` 的
404，因此尚未独立核实该安装与 App、组织的关联及仓库范围。用户提供的安装页为
`https://github.com/apps/chronoai-cma-events/installations/178446930`。平台管理员仍需在
获准会话中核验 installation `178446930` 属于 App `5606750`、
组织 `253878174`，且授权仓库包含 `1330597394`，再出具绑定材料。URL 本身不构成租户授权证明。
App 归属、组织安装范围和 agent 的执行凭证分别配置，不相互替代。

旧代码只支持最长一小时的静态安装核验材料，这个限制必须被替换。新方案一次性核实
租户归属并持久保存绑定，由生命周期事件和后台 GitHub API 查询维护当前执行资格；
不要求每小时人工续期。暂时核验失败按新设计进入自动重试/恢复，真实撤权或凭证失效才
提示具体处理事项。保持业务 webhook 关闭，直到动态注册、自动核验、持久化、channel 和
owner 绑定能力就绪。App 接入不再要求用户提供或操作 Kubernetes 配置。

### 当前控制面地址和 Agent 创建请求

2026-11-04 只读核验发现，`https://bot.chrono-ai.fun/api/v1/agents` 返回 301 到
`https://cma.nyxid.ai/api/v1/agents`。写请求直接使用当前地址，不能依赖重定向保留 HTTP 方法。
本次通过 NyxID 发现并选择现有 CMA UserService
`04523f1d-248e-4508-b5bc-4855fab7ad1f`，其 endpoint 为 `https://cma.nyxid.ai`。
旧 UserService `0fe0a533-b90a-4405-9229-581509a81d47` 的创建调用返回了 Agent 列表，
不构成创建成功证明。此前该次检查中 CMAEG OpenAPI 未出现 GitHub 路径；发布后必须重新验证。

专用 Agent 创建请求保存在 [chrono-goals-agent-create.json](examples/chrono-goals-agent-create.json)，
名称为 `chrono-goals-sync`，沿用服务端模型默认值，显式选择 `api-github`，初始化只准备仓库。
请求复用幂等键 `chrono-goals-github-pilot-178446930-v1`；丢失响应时重放原请求与原键，
不能换键重复创建。`202` 只表示已接受，须查询 lifecycle operation 到 `succeeded`，再记录
真实 Agent ID。创建成功仍不等于 GitHub source 已绑定或业务同步已验收。

通过 `nyxid proxy request -d @file` 发送此请求时，必须显式传
`-H 'Content-Type: application/json'`，否则 CMA 返回 415 `json_content_type_required`。
恢复同一次创建时保留原请求体和幂等键：

```sh
nyxid proxy request cma /api/v1/agents --method POST \
  --via-service 04523f1d-248e-4508-b5bc-4855fab7ad1f \
  --header 'Content-Type: application/json' \
  --header 'Idempotency-Key: chrono-goals-github-pilot-178446930-v1' \
  --data @docs/examples/chrono-goals-agent-create.json --output json --envelope
```

2026-11-04 16:31:58（Asia/Singapore），用户批准后创建接口返回 202，已取得下列持久身份：

| 资源 | 已取得的 ID |
| --- | --- |
| Lifecycle operation | `101814c7901a4df68028e53b741e6857` |
| Agent | `agt_9371e4ef09aa4f88aaac8a823b342b14` |
| Sandbox | `sbx_ac6fee64e1e94297b680e6c6e3b60246` |
| Workspace | `wks_14ba26413a874a5b802f517c8f1cddea` |

2026-11-04 16:38:55（Asia/Singapore）读取为 lifecycle `queued`、phase
`sandbox_ready`、Agent `starting`，Sandbox 仍为 `provisioning`，尚未确认就绪。
公开事件仅显示资源记录已创建，没有可归因的启动错误；不能据此猜测镜像、权限或容量故障。
后续需查询同一操作的自动恢复结果，或由运维结合上述 Sandbox/Agent ID 检查服务端启动日志。
后续直接查询
`GET /api/v1/agent-lifecycle/101814c7901a4df68028e53b741e6857` 和该 Agent，不能重新创建。
Agent 的 `reach` 包含 `api-github` 及平台基线 `chrono-llm-public`、`ornn-api`；
仓库获取的初始消息仍在排队，完整同步与 PR 发布尚未执行。

NyxID `per_request` 审批的有效期不保证代理 HTTP 连接能等待同样长时间。此前已出现审批
通过但代理返回 524 的情况；审批通过不等于 CMA 已接受创建。丢失回执时先查已有实例，
必要时使用原请求和原幂等键恢复，按当前策略由用户审批。不要通过切换凭证、直连 API
或取消审批策略绕过此门禁。

本机通过 NyxID GitHub UserService `ae73c551-4a36-431b-9d02-7210dfb1d06f` 读取
`ChronoAIProject/chrono-goals` 返回 200，默认分支为 `main`，仓库元数据报告 `push` 权限。
这证明该连接的读取可用，但 Agent 内的 `gh` 执行和 PR 发布仍需另行验证。

发布链需要单独处理：仓库的 `Publish release` workflow 只构建并发布镜像，不执行生产 rollout；本次动态能力需要经既有服务版本发布和数据库迁移流程交付。只看到镜像发布成功不能作为 CMAEG 已上线的证据。App 注册、secret 轮换、安装核验和 source 激活全部通过控制面完成，不附带 App 专属 Kubernetes 配置，也不要求每次接入重新发布服务。

## 上线前条件

CMAEG 的动态 GitHub capabilities 必须报告管理、凭证轮换、初次归属验证、自动资格核验和持久执行已就绪。用户通过受保护控制面完成：

- 注册 App `5606750`，提交 write-only webhook secret，取得服务端返回的 `app_ref` 和 webhook URL。secret 不进入 issue、PR、Agent prompt 或日志。
- 通过独立的受保护字段上传 App private key，供 CMAEG 后台识别 App、读取安装状态并签发 metadata-only token 查询仓库资格；不交给 agent，不写入 Kubernetes。凭证健康状态必须可查询。
- 用经认证的 CMA owner 身份请求 installation `178446930` 的绑定，由获准管理员独立核验 App/account/owner 归属并记录初次授权证据。绑定无按小时过期时间；自动核验成功后才开放当前仓库的执行资格。
- 创建/更新持久 channel 的 GitHub source，设置本轮 CMA repository allowlist、event/action filters 和目标 goal agent consumer；控制面持久化激活及恢复资格，不要求修改 record-ID 环境变量。
- 授权 goal agent 读取 `projects.yaml` 中全部允许的源仓库，并在 `ChronoAIProject/chrono-goals` 的固定 automation 分支创建或更新状态 PR。

CMA agent 首轮仍使用现有获准 NyxID GitHub connection 执行脚本，与 CMAEG 的控制面核验凭证分开。`SYNC_GITHUB_TRANSPORT=gh` 只表示通过受管 `gh api` 访问 GitHub；NyxID Agent Key 不是 GitHub token，不能写入 `SYNC_TOKEN`。如果线上 Agent 不能通过受管 `gh api` 读取目标仓库或写目标板 PR，先修复连接授权，不扩大 webhook 权限。

## Webhook secret 的保存

Webhook secret 是独立生成的高熵共享密钥，GitHub 用它对原始请求 body 计算 HMAC-SHA256，
CMAEG 用同一个值验签。它不是 GitHub App 的 PEM private key；后者在新设计中单独托管，
仅供后台自动核验使用，不参与 webhook HMAC，也不替代 agent 的现有 NyxID GitHub 连接。

使用密码管理器生成并保管独立随机值（例如 32 字节安全随机数编码成 64 个十六进制字符），
不要把值发到聊天、Agent prompt、日志或提交到 Git。新配置位置如下；对应控制面尚待实现：

| 位置 | 保存内容 |
| --- | --- |
| GitHub App 的 Webhook secret 字段 | 同一随机值；接入能力就绪前保持 webhook Active 关闭 |
| CMAEG GitHub integration 的 secret 表单 | 提交同一值；后续查询只返回存在状态/版本，不回显秘密 |
| CMAEG 内部 | 复用已有 keyring，将 App 专属 credential 加密持久化；运行时读取，不依赖启动文件 |

不要创建 GitHub 专属 Kubernetes Secret，不设置配置文件环境变量，不要求 Agent 提取密钥。
当前代码的文件解析器必须被动态实现替换；直接填入一个 deployment secret 不能通过新版验收。
正常注册和轮换应在多副本上生效，无需 rollout。轮换按控制面返回的有界过渡期配置 GitHub，
再确认新 credential 版本收到有效投递。App private key 不参与本轮 webhook 验签。

后台核验的 App private key 通过独立 write-only 表单提交，使用同一既有加密基础设施、不同凭证记录和版本。CMAEG 自动刷新由它签发的短期核验 token；token 到期不要求重新绑定 installation，也不会要求人工每小时操作。

官方契约参考：
- https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries
- https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/managing-private-keys-for-github-apps

## 验收顺序

### 1. 检查配置和租户绑定

先确认服务实例已经加载包含 GitHub 路径的版本：

```sh
curl -fsSL "$CMAEG_BASE/openapi.json" | jq '[.paths | keys[] | select(contains("/github/"))]'
```

预期出现动态 App 注册、两类 credential 管理、capabilities、初次 installation verification、自动 reconciliation，以及既有 webhook/delivery/task 路径；只有旧接收接口也不算就绪。capabilities 或实际管理请求报告缺少能力时停止验收，不通过 ConfigMap/Secret 补配置。

用真实 owner 身份注册 App，保存返回的 `APP_REF` / `WEBHOOK_URL` 和操作版本；由独立管理员通过控制面核验 App/安装归属。绑定 GET 应分别显示长期归属和当前 eligibility，并给出最近成功核验、freshness 截止、下一次检查及安全错误原因；account/repository/authority revision 应与当前证据一致。重复原意图返回原资源，跨 owner 查询应返回 `404`，普通 owner 不能自行批准归属或填写新的核验截止时间。

通过现有 channel/source check 确认 source 和 consumer 已持久化且激活。至少在两个实例之间验证注册、secret 轮换、暂停和恢复资格一致，过程中不修改 Kubernetes 配置、不重启 Pod。然后将注册接口返回的 URL 和同一 secret 填到 GitHub，启用投递。此前示例中的 `chrono-goals` 不是已注册 app_ref。

### 2. 验证 GitHub 原生接收

在 GitHub App 页面发送 ping，或用真实 App delivery。预期 ping 返回 `200`、receipt 为 `ignored`，不会产生业务 task。真实 issue/PR 事件使用 App 实际发送的原始 body 和 `X-Hub-Signature-256`；不要重新序列化 body：

```sh
curl -sS -X POST "$WEBHOOK_URL" \
  -H "Content-Type: application/json" \
  -H "X-GitHub-Delivery: $DELIVERY_ID" \
  -H "X-GitHub-Event: issues" \
  -H "X-Hub-Signature-256: $SIGNATURE" \
  --data-binary @issue-event.json
```

预期：合法、已绑定且命中 channel source 的事件返回 `202`，receipt 为 `accepted` 并带稳定 `receipt_id`/`task_ids`；相同 delivery 原样重发返回 `200` 和相同 receipt。改动 body、重新计算有效签名但保留 delivery ID 应返回 `409`；错误签名应返回 `401` 且没有可执行 receipt。

### 3. 验证任务排队和合批

第一次 `accepted` 只证明 delivery 和 group membership 已持久化，不代表 Agent 已经运行。对 receipt 和 task 分别查询：

```sh
curl -sS "$CMAEG_BASE/v1/github/deliveries/$RECEIPT_ID" \
  -H "Authorization: Bearer $CMA_OWNER_TOKEN"
curl -sS "$CMAEG_BASE/v1/github/tasks/$TASK_ID" \
  -H "Authorization: Bearer $CMA_OWNER_TOKEN"
```

在 30 秒窗口内对同一 channel record 再制造一个获准 issue/PR 事件，预期它们共享一个稳定 group/task ID，并在任务数据中形成一个 `com.github.batch` 批次。任务状态依次可能是 `queued`、`running`、`recovering`，最终是 `succeeded`、`no_change`、`failed` 或 `owner_action`。chrono-goals 返回成功时，task 的 `result` 应包含 `status: "succeeded"`、分支、commit 和 PR；没有变更时应为 `status: "no_change"` 且不创建新 PR。CMAEG 不承诺端到端 exactly-once；重启或超时后应继续查询原 task，不新建 delivery key。

### 4. 验证 goal agent 和 GitHub 脚本认证

从 task 响应取得 `response_id` 后，通过现有 owner 可见的 CMA Responses 查询查看 Agent 执行是否结束；不要向模型索取托管凭证。Agent 工作副本必须按 sync 技能执行：

```sh
SYNC_GITHUB_TRANSPORT=gh npm run preflight:github
SYNC_GITHUB_TRANSPORT=gh npm run sync -- --list-unmapped
npm run check && npm run build
SYNC_GITHUB_TRANSPORT=gh npm run publish:status -- --json \
  --title "同步工作 YYYY-MM-DD" \
  --body-file /tmp/chrono-goals-sync-pr.md
```

预期：preflight 能读取 `projects.yaml` 列出的所有仓库，并确认目标板可写；完整同步和检查通过；最后 JSON 返回 `status: succeeded`（含唯一状态 PR）或 `status: no_change`。脚本不能把 `SYNC_TOKEN`、Agent Key 或任何托管 GitHub secret 打印到输出。

### 5. 验证状态 PR 和现有发布链

确认 `ChronoAIProject/chrono-goals` 只有固定 automation 分支上的状态 PR 发生变化，diff 仅包含 `status/`。按现有仓库策略运行 checks 并人工合入；合入后确认既有 Pages workflow 生成页面。task 的 `succeeded` 只表示 Agent 任务和受管发布结果完成，不代表 PR 已自动合入或 Pages 已发布。

### 6. 验证回环、撤权和不确定结果

逐项验证：

- 首轮仅路由 CMA 仓库，目标板状态 PR 事件不能匹配该路由；若后续也订阅目标板，使用目标仓库和已记录的本自动化 PR/分支身份识别自身输出。不能因所有 bot 或共同的 `automation/` 前缀而一律忽略，其他 bot 的有效业务工作仍按 allowlist 处理。
- 删除试点 repository 的 channel allowlist 或使 installation `suspend`，新事件不能进入执行；已排队但未开始的 group 变为 `owner_action` 或 `recovering`，不能沿用旧 authority revision。
- 发送 `repository.transferred` 或 `repository.deleted` 后，旧 repository ID 从 binding 中消失；`repository.renamed` 保留原 ID，`repository.created` 不自动加入 allowlist。
- 在 webhook 返回前模拟网络断开或在任务运行中重启 CMAEG，恢复后查询原 delivery/task；不因 `404` 或进程重启就换 key 重跑可能已经提交的任务。

### 7. 验证自动核验和长期运行

- 健康情况下观察多轮自动检查，确认无需人工续期；超过原来一小时的边界后，同一个绑定仍能处理事件。聚焦测试用外部时钟推进，线上验收以实际检查记录为准。
- 不投递某次仓库移除生命周期事件，下一轮后台查询仍应识别移除、更新 authority revision 并阻止相关任务执行。迟到的 added/unsuspend 不得直接恢复权限。
- 模拟临时 GitHub API 失败。新鲜度窗口内按既有有效资格处理，超过截止后停止 dispatch，并将既有获准范围的新投递持久记录为 `pending_verification`；绑定仍保留，后台自动重试。
- 恢复查询后，原 receipt/task 在当前权限下自动恢复或结束为无资格，不生成新身份，不要求 owner 重新批准。重启及多副本不能延长失败观测的新鲜度或重复执行。
- 分页未完成和不明确的 404 不能刷新权限。短期 token 过期应自动刷新；App private key 确认失效时显示“修复核验凭证”，不能显示“每小时续期绑定”。

## 失败判断

以下情况停止试点并保留证据：

- webhook 已 `202`，但 receipt 查询不到；
- 同一 delivery 的 body/action 冲突没有 `409`；
- 未绑定 installation、跨 owner 或 removed repository 仍产生 Agent task；
- Agent 能运行 `gh api` 读取但不能完成目标板状态 PR，或脚本要求把 NyxID Agent Key 当 `SYNC_TOKEN`；
- 同一 group 在恢复后产生两个并发状态 PR；
- task 显示 `succeeded` 但没有可查询的 `response_id` 或 `result`；`no_change` 不要求 commit/PR。
- 健康自动核验仍要求每小时人工批准，或临时故障恢复后必须手工重新绑定才能继续；
- 遗漏撤权事件后后台始终发现不了变化，或不完整查询仍延长了可执行权限。

这些失败分别属于接收持久化、租户授权、Runtime/NyxID GitHub 连接、执行恢复或业务发布权限问题，应先定位边界再扩大试点。

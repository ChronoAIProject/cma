# R1：恢复出的 home 落后于 Mongo 的处理设计（最小版）

- 所属：#1473 独立 home 备份与恢复，方案见 [IMPLEMENTATION.md](IMPLEMENTATION.md) §0.2 R1、§5.5.1
- 状态：**已批准用于开发（2026-11-16）**；§7 的事项须先实测
- 日期：2026-11-16，基于 `develop@4507dc7ba` 的代码复核
- 修订：同日第二版。第一版声称恢复会“违反 7 处现行表述”，逐条核对原文与上下文后确认**不成立**（§5）；并按“只做 #1473 要求的部分”删减了第一版中 7 项非必需内容（§9）

---

## 0. 结论

恢复做成一次显式的、由管理员准入并记录的换代（新 runtime generation、新 PVC），方式是**保留用户内容，重置 runtime 内部状态**：

1. 新 PVC 从快照建出后、runtime 启动前，把 runtime 内部状态移入隔离目录，只保留已完成的安装记录，并清点 home 中实际存在的 agent 会话、workspace、worktree。
2. runtime 以全新的内部状态启动，相当于一个新 runtime 接管一个有内容的 home。
3. 控制面按清点结果结算：**沿用现有 runtime 丢失和历史 checkpoint 恢复的结算语义**，只补三条 home 回退特有的规则。

这复现了仓库里唯一一个“home 落后于 Mongo”先例的关键前提：**只有 home 子树来自过去，runtime 自己的状态永远是新的。**

---

## 1. 问题：现有代码会静默接受旧状态，并重做操作

| 事实 | 依据 |
|---|---|
| 控制面在 boot 时只给 runtime sandbox id、generation、home identity、控制 token、`TMPDIR`，不传任何期望的序号或 ledger 标识 | `layer3-adapter/src/hosted/deployment.rs` |
| runtime 的检查只防 generation 太高，不防状态太旧；遇到较低的已存 generation 视为正常替换 | `layer4-runtime/src/persistent_home.rs:134-144`、`ledger_format.rs:71-97` |
| 启动时 `recover_journals` 在接受控制面任何请求之前，**按 ledger 中的 fence 重做 t_s 时处于 Started 的记录**：AddWorktree、RemoveWorktree、InstallAgentSkill 被重新执行 | `layer4-runtime/src/supervisor_mutations.rs:235-299`、`mutation_store.rs:162-183` |
| mutation fence 只拒绝 `incoming < latest`，恢复后 high-water 变低，runtime 察觉不到自己跳过了什么 | `layer4-runtime/src/mutation_authority.rs:201-206` |
| 新 PVC 带来新的 home identity，`bind()` 拒绝不同的回执 | `layer2-core/src/sandbox/home.rs:68-77` |
| reopen 只核对会话 id；retained 分支以 Mongo 是否有 `conversation_id` 判定 agent 可续 | `cma/src/sandbox/worker/lifecycle/conversations.rs:114-128`、`layer2-core/src/sandbox/runtime_recovery_agents.rs:55-69` |

**把旧卷直接交给 runtime，它会通过所有检查，然后重做一部分已完成的操作，且无人察觉。** 这直接违反 #1473 的验收：“封旧writer且不重做不确定效果”，以及 QA 任务“任何恢复依赖……盲重发均FAIL”。所以 R1 不是可选项。

---

## 2. 为什么选“重置 runtime 内部状态”

1. **多数 runtime 状态在控制面没有副本，无法逐项对账。** ledger 的 `revision`、`store_id` 只作遥测上报；fence high-water 控制面从不回读。
2. **重做发生在控制面能介入之前**，只能在 runtime 启动前处理。
3. **有先例**：历史 checkpoint 恢复只把 archive 解回 home 子树，state root 和 transfer 属于新 runtime（`layer4-runtime/src/snapshot_export.rs:141-153`）。
4. **runtime 能接受“有内容的 home + 空内部状态”**：
   - 启动 agent 只检查磁盘：agent home 是目录、cwd 是带 `.git` 文件的 worktree（`layer4-runtime/src/supervisor_agent.rs:300-330`）；
   - 安装记录只绑定 `sandbox_id`，不绑定 home identity，可原样保留（`install_state.rs:30-72`）；
   - 安装以 home 为 cwd、`HOME` 指向 home 执行，结果随快照恢复（`install.rs:51`、`config.rs:129-135`）。这与现有同一 PVC 上的暂停/恢复不重装的行为一致。

**评估过但不可行的更小做法：只改恢复 template 的 subPath，让 runtime 看到空的 state 目录，从而完全不改 runtime。** 不可行，因为安装记录放在 transfer 的 `runtime-state/` 下（`install_state.rs:15,475`），ledger 的格式屏障也在 transfer 下（`ledger_format.rs:21-26`）。换掉 transfer 的 subPath 会丢掉安装记录、触发重装；只换 state 的 subPath，又会留下指向旧 ledger 的屏障。两者都必须由 runtime 代码有选择地处理。V1 实测若发现屏障不在 transfer，这一结论要重新评估。

---

## 3. 恢复准备：runtime 启动前的 init 步骤（Layer 4）

在恢复专用 template 的 init container 中，用 runtime 自己的二进制执行一个新模式，例如 `layer4-runtime --prepare-restored-home <restore_id>`，在 `initialize-home` 与 supervisor 之前运行。

| 步骤 | 内容 |
|---|---|
| 1 | 以 `restore_id` 为幂等键，已完成则直接返回 |
| 2 | 把 state root 整体、transfer 下的 `runtime-state/`（安装记录除外）与 `checkpoint-writer-state/` 用 `rename` 移入 `/var/lib/cma-transfer/restore-quarantine/<restore_id>/`。同一块 PVC，原子操作，不复制数据。旧 home 绑定随之移走，runtime 启动时以新 identity 新建 |
| 3 | 安装记录仅当为 Completed 且 `sandbox_id` 一致时保留，否则准备失败，返回 typed `restore_install_incomplete` |
| 4 | 清点：agent home 目录、native 会话 id（会话文件第一行的 `payload.id`，`conversation.rs:149-165`）、workspace id（`.git/info/cma-workspace-id`）、worktree 目录；写 `restore-inventory.json` 及其 sha256 |
| 5 | 写完成标记 |

- **不修改用户数据。** 会话文件尾行若因快照撕裂而不完整，不做修复；该 agent reopen 失败时按 §4.2 单独处理。
- **能力声明**：runtime 只有在实际演示过这一模式后，才在 `RuntimeStatus` 的能力标志中声明 `home_restore_v1`，并返回清点结果。控制面在准入时校验恢复所用镜像声明了该能力，否则 typed 拒绝（CLAUDE.md §3：能力必须被证明）。
- 隔离目录不在恢复路径中删除，保留到被替换的 home 处置时（恢复验证通过后 ≤7d），期间可供 TC-087 比对恢复前的 runtime 回执。

---

## 4. 控制面结算（在 revive 的 HomeRestore 阶段）

时机：`CredentialDelivery` 之后、`ConversationReopen` 之前。输入是 runtime 返回的清点结果，一次 fenced 提交。

### 4.1 规则表

**沿用现有语义（不新增代码路径，只把恢复接到非 retained 分支上）：**

| 对象 | 规则 | 依据 |
|---|---|---|
| 进行中的 turn | `TurnInterrupted`，pending approval 随之清空 | `runtime_recovery_agents.rs:72-75`、`pending_approvals.rs:10-51` |
| 写出结果不确定的操作 | 以 `runtime_lost` 退休，保持不确定，**不重发** | `agent_operation.rs:401-413` |
| 已尝试过的排队消息 | 不重放；从未尝试过的 `Queued` 消息保留并在 reopen 后投递 | `runtime_recovery_agents.rs:99-102`。历史 checkpoint 恢复同样如此 |
| 未覆盖的 agent | `Stopped(RuntimeLost)`，保留 id、历史与层级位置 | `runtime_recovery_agents.rs:135-143` |
| 已覆盖的 agent | `RuntimeGenerationRetired`，转 Paused，由 reopen 打开 | `runtime_recovery_agents.rs:117-134` |
| agent 的 work receipts 与 cleanup debt | 清空，它们指向已被隔离的旧 ledger | `runtime_recovery_agents.rs:174-177`（非 retained 分支） |
| 输出检查点 | 新 generation 从 cursor 1 开始 | `conversation_output.rs:111-128` |
| 凭据 | CredentialDelivery 下发当前 key，先撤销旧 runtime 的凭据；凭据只在 supervisor 内存中，不在 home 里 | `layer4-runtime/src/upstream.rs:19-86` |
| capacity recovery 等绑定旧 home 的工作 | 自动被拒（目标携带旧 boot id 与 home identity） | `layer4-runtime/src/supervisor_agent_recovery.rs:30-36` |

**新增的三条规则（home 回退特有）：**

| 对象 | 规则 | 为什么必须 |
|---|---|---|
| **agent 是否已覆盖** | 以清点结果为准：会话 id 出现在恢复出的 home 中才算覆盖 | 否则 t_s 之后新建的 agent 会被判为可续，reopen 失败并拖垮整个恢复。历史 checkpoint 分支本来就以 archive 中的会话清单判定，这里是用清点替代清单 |
| **home 身份重绑** | 以新的 `pvc_uid`、`home_identity`、`source_generation` 重新绑定；旧身份写入 `superseded_homes`，校验器永不重新挂载 | `bind()` 拒绝不同回执；没有显式重绑，恢复无法完成；没有 `superseded_homes`，旧盘恢复后可能被重新挂载 |
| **t_s 之后被删除的子资源** | Mongo 记为已删除、但清点显示仍在磁盘上的 agent home 与 worktree，在 reopen 前用现有 `RemoveWorktree`/`RemoveAgentHome` 在新 fence 下重新删除 | TC-065：恢复后不得有资源复活。这些目录虽然在 API 中不可见，但同一 Sandbox 中的其他 agent 能在磁盘上读到 |

**磁盘上缺失的 workspace/worktree**：不改变其状态（`Ready → Failed` 不是合法转换，`status.rs:101-149`），只把其中的 agent 判为未覆盖，并记入恢复回执。owner 可照常删除它；在其中新建 agent 会得到现有的 typed 拒绝 `agent_worktree_invalid`（`supervisor_agent.rs:322-330`）。

### 4.2 reopen 的逐 agent 隔离

现在任何一个 agent reopen 失败都会让整个 revive 判为 `restore_failed`（`revive.rs:61-72`）。只在带恢复钉的 revive 中改为：失败的 agent 转 `Stopped(RuntimeLost)`，revive 继续；`PublishActive` 只要求成功 reopen 的 agent 处于 Ready。普通 revive 不变。

这一条兜住所有清点之外的异常，包括撕裂的会话尾行，所以 §3 不需要修复用户数据。

### 4.3 记录

- **一条 Sandbox 事件** `HomeRestored{restore_id, backup_id, restored_to}`。CLAUDE.md §5：“Lifecycle transitions are never silent. The record names the runtime version and progress.”
- **恢复回执**，写在 Sandbox 的 `home_restore` 记录中：清点 sha256、覆盖与未覆盖的 agent、磁盘缺失的 workspace/worktree、重新删除的子资源、恢复到的时间点。这是 TC-087 比对 owner、native id、receipt 的依据。
- 历史中 t_s 之后的 turn 保持可见（Mongo 不回退），满足 CLAUDE.md §5“Everything already shown remains visible”。

---

## 5. 与现行文档的关系（第一版判断有误）

第一版说恢复会“有意违反 7 处现行表述”，并提出 D13 请负责人批准例外。逐条核对原文与上下文后，**一处都不构成违反**，D13 撤回：

| 文档 | 原文上下文 | 结论 |
|---|---|---|
| `CLAUDE.md:457-468`（§5） | 标题是“No **deployment, upgrade, restart or rollout** may interrupt a Sandbox”，三条要点约束的是部署与升级 | 不适用于存储丢失后的恢复。即便按字面看也满足：id 保留；历史可见；agent 继续的是同一个记录在案的会话，不是“看起来相似的新会话” |
| `docs/CMA_Runtime_Recovery.md:141-143` | 自动替换 runtime 的恢复中，原卷缺失“remains a refusal”，不得用**空卷或新会话**替代 | 不违反，而且是本设计的前提：自动恢复照旧拒绝；备份恢复由管理员显式准入（D10），用的是备份而非空卷，会话 id 不变 |
| `docs/CMA_Native_Runtime_Authority.md:48-54` | 快速校验路径必须拒绝**在状态读取与最终 home 检查之间**发生的 PVC 替换 | 不违反。它防的是静默替换；恢复是持久化重绑之后的显式替换，此后的校验针对新 PVC |
| `docs/CMA_Ledger_Recovery.md:88-90`、`CMA_Ledger_Integration.md:124`、`CMA_Ledger_Completion.md:132` | 指 ledger 维护（retirement/collection）中**恢复 ledger** 需要“无更新效果”的证明 | 不违反。本设计**不恢复 ledger**，而是隔离并重新开始，正是因为这个证明无法取得 |
| `docs/CMA_Runtime_Ledger_Storage.md:59-63` | 回滚 **runtime 镜像**时不得移除格式屏障、不得用切换前的快照启动旧镜像 | 不违反。恢复使用当前镜像，新 ledger 会先建立新屏障。**请 ledger 负责人确认**：把旧屏障随旧 ledger 移入隔离目录，在当前镜像下是否安全 |
| `docs/CMA_Agent_Log_Segments.md:212` | capacity-recovery 准入必须拒绝**已被替换的 home/native identity** | 不违反，是预期结果：恢复后，绑定旧 home 的 capacity recovery 会被拒绝 |
| `docs/CMA_Operations.md:1488-1489` | 恢复切换前的备份是“separate destructive epoch transition, never an ordinary rollback” | 与本设计一致 |

仍需要的文档工作只是澄清，不是例外：在 `docs/CMA_Home_Backup.md` 写明“home 备份恢复不恢复 ledger、不是镜像回滚、只能由管理员显式准入”，并在上述 ledger 文档中加一处交叉引用。CLAUDE.md §5 只在能力被证明后由 PR-6 补充一句（原计划不变）。

---

## 6. 现有可参考的恢复逻辑

| 现有逻辑 | 位置 | 本设计如何使用 |
|---|---|---|
| **历史 checkpoint 恢复**（runtime 丢失后从 archive 恢复）——仓库中唯一的“home 落后于 Mongo”先例 | 控制面：`layer2-core/src/sandbox/runtime_recovery.rs:79-147`、`runtime_recovery_agents.rs:55-178`；revive：`cma/src/sandbox/worker/lifecycle/restore.rs:149-187,353-368`；runtime：`layer4-runtime/src/checkpoint.rs:169-184`、`persistent_home.rs:162-197` | **复用其控制面结算语义**（§4.1 上半表全部来自这里）和“只恢复 home 子树”的前提。不复用其 archive 解包与 `Inherited` 安装回执：后者绑定 archive 摘要，而本设计可以原样保留 Completed 安装记录，不需要它 |
| **runtime 丢失恢复**（retained 分支） | `runtime_recovery.rs:65-78` | 恢复不走这个分支：它假设 home 完整，以 Mongo 的会话 id 判定可续 |
| **revive 的阶段与原子钉住模式** | `layer2-core/src/sandbox/revive.rs:23-41`、`runtime_update_resume.rs:7-42` | 复用阶段与 authority；恢复钉沿用 runtime update 钉入 revive 转换的写法 |
| **same-image retained recovery**（从 Failed 准入） | `layer2-core/src/sandbox/retained_recovery.rs:72-76`、`docs/CMA_Retained_Recovery.md` | 参考其准入结构（typed 失败类别、Idempotency-Key、证明回执）。不能照抄：home 丢失记录的失败类别是 `provisioning_failed`，不在它的接受范围内（R2） |
| **后台快照导出时的会话清点** | `layer4-runtime/src/snapshot_export.rs:143-153` | 参考其“检查快照中是否含有每个已准入会话 id”的做法实现 §3 的清点 |
| **启动 agent 的磁盘校验** | `layer4-runtime/src/supervisor_agent.rs:300-330` | 依赖它：缺失的 worktree 由它 typed 拒绝，不需要改 worktree 状态机 |
| **现有删除操作** | `RemoveWorktree`/`RemoveAgentHome` | 复用它们做删除重放 |

---

## 7. 实现前必须实测的事项

| # | 事项 | 为什么重要 |
|---|---|---|
| V1 | runtime 在“有内容的 home + 空 state root + 保留的安装记录”下能否完成首次启动：新建 home 绑定、打开新 ledger、启动已有 agent | **本设计的基础**。依据是历史 archive 恢复，但该路径已退役。不过，设计要重做 |
| V2 | Codex 对 t_s 时正在写入的会话文件执行 `session/load` 的行为 | 决定逐 agent 隔离会被触发的频率 |
| V3 | ledger 格式屏障的确切位置，以及移入隔离目录后当前镜像能否建立新屏障 | §2 中“更小做法不可行”的结论与 §5 的 ledger 确认都依赖它 |
| V4 | 已有 workspace 的正常使用是否依赖 workspace journal 中的 Completed 记录 | 若依赖，重置后已有 workspace 可能无法使用 |
| ~~V5~~ | ~~控制面能否拿到“安装完成”的时间~~ | **不再需要**：R2 不在准入时检查，由 §3 第 3 步在安装记录不是 Completed 时直接拒绝（R2-DESIGN §3.2） |
| V6 | 自定义镜像中 runtime 的版本如何确定（D14） | 不支持 `home_restore_v1` 的镜像只能 typed 拒绝 |

---

## 8. 测试

| 编号 | 用例 |
|---|---|
| R1-1 | init：内部状态移入隔离目录；Completed 安装记录保留；非 Completed 时 typed 失败；重复执行幂等；不修改任何用户文件 |
| R1-2 | 清点：agent home、会话 id、workspace id、worktree 均正确；会话 id 无法解析时该 agent 未出现在清点中 |
| R1-3 | 以 fixture 预置 t_s 时处于 Started 的记录，确认启动后**不被重做**（这是 #1473“不重做不确定效果”的直接证据） |
| R1-4 | 结算：t_s 之后新建的 agent 判为未覆盖并 `Stopped(RuntimeLost)`；被删除的子资源被重新删除；磁盘缺失的 worktree 中的 agent 判为未覆盖 |
| R1-5 | 单个 agent reopen 失败不影响其他 agent，revive 完成 |
| R1-6 | 记录：Sandbox 事件与恢复回执完整 |
| R1-7 | 不支持 `home_restore_v1` 的镜像被 typed 拒绝 |
| R1-8 | 普通 revive（无恢复钉）的行为完全不变 |

---

## 9. 相对第一版删减的内容

原则：#1473 没有要求、且有现成先例或可推迟的，一律不做。

| 第一版的内容 | 处理 | 理由 |
|---|---|---|
| 未投递的输入一律 typed 失败 | **删除，沿用现有语义** | 历史 checkpoint 恢复保留并投递从未尝试过的 `Queued` 消息，这是首次投递，不是重发，不违反“不盲重发”。风险（用户看着 t_s 之后的输出写下的输入被投进 t_s 的会话）是已知且被先例接受的 |
| usage 重设基线 | **推迟** | 现有行为是低估用量，上限是 t_s 到故障之间的 token 量，且恢复很少发生。不是 #1473 的要求，可作为后续缺陷单独修复 |
| 通过 `RefreshContext` 给 agent 恢复说明 | **推迟** | 不是 #1473 的要求；owner 可见的记录已满足 CLAUDE.md §5 |
| 重置 managed guidance 版本 | **推迟** | 同上 |
| 截断撕裂的会话尾行 | **删除** | 会修改用户数据；由逐 agent 隔离兜底 |
| 新的 `AgentStopReason::HomeRestored` | **删除，复用 `RuntimeLost`** | 先例对未覆盖的 agent 就用 `Stopped(RuntimeLost)`，少一个严格 reader 必须读懂的枚举值 |
| 新的 `AgentLogKind::WorkingStateRestored` | **删除** | Sandbox 事件加恢复回执已满足“转换不得静默”的要求 |
| workspace/worktree 的“恢复丢失”状态 | **删除** | 需要改状态机；现有的启动校验已能 typed 拒绝在缺失 worktree 中启动 agent |

**删减后 PR-2 需要先部署的形状只剩三项**：Sandbox lifecycle 的 `home_restore`（恢复钉与回执）、`superseded_homes`、`SandboxEventKind::HomeRestored`。第一版是八项。

**不能再删的部分**，每项都对应 #1473 的要求：

- 内部状态隔离：“不重做不确定效果”（§1）；
- 清点与覆盖判定：否则恢复会因 t_s 之后新建的 agent 整体失败，达不到“恢复原工作身份”；
- 身份重绑与 `superseded_homes`：“封旧writer”；
- 删除重放：TC-065“恢复后0资源复活”；
- 逐 agent 隔离：TC-087 要求 10 个 home 的恢复全部成功，一个 agent 的异常不能让整个 home 恢复失败；
- 一条事件加一份回执：CLAUDE.md §5“transitions are never silent”，以及 TC-087 比对 receipt 的依据。

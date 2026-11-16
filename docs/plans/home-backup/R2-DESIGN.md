# R2：恢复的准入设计（哪些 Sandbox 可以恢复、凭什么证据）

- 所属：#1473 独立 home 备份与恢复，方案见 [IMPLEMENTATION.md](IMPLEMENTATION.md) §0.2 R2；与 [R1-DESIGN.md](R1-DESIGN.md) 配套
- 状态：**已批准用于开发（2026-11-16）**；§7 的事项须先实测
- 日期：2026-11-16，基于 `develop@4507dc7ba` 的代码复核

---

## 0. 结论

**准入只看两件事：Sandbox 处于“已停下”的状态，以及原 home 确实不可用的证据。不看失败类别。**

- 可准入的状态有三种：`Paused`、`Failed`、**卡住的 `Reviving`**（revive 停在建盘之前，没有存活的 worker lease）。第三种不能省：home 丢失后 Sandbox 可能根本不会进入 Failed，而是一直重试或被停放。
- 证据以**一次 PVC 读取**为准：按记录的名字读原 PVC，只要原 UID 已不存在（读不到，或同名但 UID 不同），就自动准入。这类事实是单调的——UID 唯一，原对象不可能再回来——所以读取与提交之间的时间差不影响正确性。
- 原 PVC 仍在、但数据损坏的情况，需要运维显式声明并绑定当前 UID，才能准入。
- 入口是一个 admin 命令，写法照搬现有的 same-image retained recovery：在一次 Mongo 事务里 CAS、写回执、离开原状态、启动带恢复钉的 revive。

R2 新增的代码只有四处：一个只读的原 home 观测、一个领域准入函数、一个 admin 路由、回执中的证据字段。其余全部复用（§5）。

---

## 1. 问题

### 1.1 失败类别不能用来判断 home 是否丢失

home 校验发生在 revive 的 `RuntimeProvisioning` 阶段（`cma/src/sandbox/worker/lifecycle/restore.rs:48`），该阶段的终态失败一律记为 `provisioning_failed`（`layer2-core/src/sandbox/revive.rs:61-72`）。这个类别同时涵盖配额不足、拉镜像失败等与 home 无关的情况。

而且原因已经被抹平了：现有读取把“Claim/Sandbox CR/PVC 不存在、正在删除、UID 被换”统统折叠成 `Ok(None)`（`layer3-adapter/src/agent_sandbox/pool/native_home_recovery.rs:80-98,102-112`），最后只剩一条字符串错误 `"native home identity changed or disappeared"`（`pool/native_home_store.rs:154-155`）。

所以**从 Sandbox 的失败记录里得不出“home 丢了”这个结论**，必须在准入时重新读取 provider 的事实。

### 1.2 home 丢失后 Sandbox 不一定进入 Failed

按代码路径推断（需 §7 V-R2-1 实测确认）：

| 丢失方式 | 走到哪里 | 结果 |
|---|---|---|
| PVC 被删除，或被控制器重建为同名、不同 UID 的空 PVC | `verify` 读到 `None`，或 UID 校验失败，判为 Rejected | `Failed(provisioning_failed)` |
| Sandbox CR 不在了 | 读取得到 404，`api_error` 把 404 映射为 `unavailable`（`layer3-adapter/src/agent_sandbox/mod.rs:256-272`），可重试 | **停在 Reviving 反复重试**；重试预算用尽后工作被停放，仍不会进入 Failed（`cma/src/sandbox/work_retry.rs:150`） |
| PV 丢失，PVC 处于 Lost 状态 | 校验不看 phase，可能通过；随后挂载失败、等待就绪超时，映射为可重试 | 同上，**停在 Reviving** |

**如果准入只接受 Failed，后两种情况只能靠人工改库才能恢复。** 这正是 #1473 QA 任务判 FAIL 的情形（“任何恢复依赖人工DB修复……均FAIL”）。所以卡住的 Reviving 必须在准入范围内。

---

## 2. 原 home 的观测（Layer 3，只读）

在现有 home volume 端口上新增一个只读方法，按记录的名字读一次原 PVC，返回中性的分类：

| 分类 | 判定 | 是否单调 | 准入 |
|---|---|---|---|
| `Gone` | 读不到；或读到同名对象但 UID 与记录不同 | **是**：UID 唯一，原对象不会再回来 | 自动准入 |
| `Deleting` | 原 UID 存在，但带 `deletionTimestamp` | 否，但会单调地走向 `Gone` | typed 拒绝，可重试：`restore_wait_for_home_deletion` |
| `Unbound` | 原 UID 存在，但未处于 Bound（例如 PV 丢失后的 Lost） | 否，PV 可能恢复 | 需运维声明（§3.3） |
| `Present` | 原 UID 存在且 Bound | — | 需运维声明（§3.3） |
| `Unreadable` | API 错误 | — | typed 拒绝，可重试：`restore_evidence_unavailable` |

- **只看 PVC。** Claim 和 Sandbox CR 的状态不影响“home 是否还在”的判断。它们只影响封旧 writer，那一步在恢复执行时处理（§4）。
- **不需要新权限。** PVC 的 get/list 已在远端预检表中（`docs/CMA_Deployment_Readiness.md:478-486`）。
- **复用现有读取。** 现有代码已经按记录的名字读 PVC（`native_home_recovery.rs:30-34,52-56`），只是把结果折叠掉了。新方法沿用同一读取，换成返回分类。
- **词汇保持中性。** Layer 2 只看到“原 home 存储已不存在 / 正在删除 / 未绑定 / 存在 / 无法读取”，不出现 PVC、Kubernetes 等名字（CLAUDE.md §3）。

---

## 3. 准入条件

### 3.1 Sandbox 状态

| 状态 | 条件 | 备注 |
|---|---|---|
| `Paused` | 无 work lease、work step 为 `None`（即现有 `revive_eligibility`，`layer2-core/src/sandbox/sandbox.rs:251-259`） | 暂停期间 home 丢失，通常在下一次 revive 前就能发现 |
| `Failed` | **任何失败类别** | 不以类别判断 home 是否丢失（§1.1），由证据决定 |
| 卡住的 `Reviving` | 当前操作是 `Revive`，阶段在 `CheckpointVerification` 或 `RuntimeProvisioning`；没有绑定新 runtime；**没有存活的 work lease**（无 lease，或已过期） | 有存活 lease 时 typed 拒绝、可重试：`restore_wait_for_worker`。准入不抢占正在执行的 worker |
| 其他 | `Active`、`Pausing`、`Rotating`、`Retiring`、`Retired` 等 | typed 拒绝。home 损坏但仍在运行的 Sandbox，运维先暂停，再走 §3.3 |

### 3.2 其他条件

| 条件 | 拒绝码 |
|---|---|
| 开关 `CHRONO_HOME_BACKUP_RESTORE_ENABLED` 已开启 | `restore_disabled` |
| 调用者有 `Capability::ControlAnySandbox` | 403 |
| 指定的备份（或默认最新一份）属于该 Sandbox，状态为 `ready`，未到期 | `restore_backup_unavailable` |
| owner 没有进行中的账户删除，删除账本中没有 tombstone | `restore_owner_erased` |
| 请求中的 `expected_revision` 与当前一致 | 409 |

**不检查“安装是否在快照之前完成”。** R1 的 init 步骤会在安装记录不是 Completed 时拒绝（R1-DESIGN §3 第 3 步），这是更直接的证据。所以 R1-DESIGN §7 的 V5 不再需要。

### 3.3 原 home 仍在时：运维声明

原 PVC 仍在（`Present` 或 `Unbound`）、但数据损坏或 PV 已不可恢复时，请求必须带：

```json
{ "replace_existing_home": { "storage_generation": "<当前观测到的原 UID>" } }
```

- 声明必须与本次观测到的 UID **完全一致**，否则拒绝。这样一份声明只能作用于运维看到的那一块盘。
- 只接受 `Paused` 与 `Failed`，不接受卡住的 `Reviving`。
- 声明写入回执与审计记录。
- 后果是有限的：原 PVC 不会被删除（R1-DESIGN §4.1“home 身份重绑”），只是不再被挂载，在被替换 home 的处置期限前仍可取证。

#1473 明确把“破坏性 home 损坏”列为要恢复的情形（已核实问题第 4 条），所以这条路径不能省；它只多一个可选字段和一处 UID 比较。

---

## 4. 准入事务与恢复执行中的两次复核

### 4.1 准入事务

照搬 same-image retained recovery 的写法（`layer2-core/src/sandbox/retained_reopen.rs:225-290`，`cma/src/store/retained_reopen/transaction.rs`）：

1. **事务外**：读取原 home 的观测（§2）。provider 读取无法放进 Mongo 事务。
2. **事务内**：
   - 读 Sandbox 行；若已有回执且 `Idempotency-Key` 一致，按原结果返回（replay）；key 不同则 409；
   - CAS：`expected_revision` 一致，并满足 §3 的全部条件；
   - `home_backups` 上 `restore_pins += 1`，条件 `state == ready`；
   - `home_backup_schedules` 置为 `restoring`，fence 自增，使在途捕获失效；
   - Sandbox：
     - 从 `Paused`、`Failed`：`start_revive`，状态置为 `Reviving`，清掉 `failure`；
     - 从卡住的 `Reviving`：取消当前操作，再 `start_revive`，得到新的 authority，旧 worker 之后的任何写入都会因 authority 不符而失败；
     - 写入恢复钉 `home_restore{backup_id, operation_id, evidence, attestation?}`，并在同一个 revision 中提交（与 runtime update 把目标钉进 revive 的写法一致，`layer2-core/src/sandbox/runtime_update_resume.rs:7-42`）；
     - `paused_notice` 写一条说明（字符串字段，不新增枚举）；
     - work 调度到 `RunLifecycleOperation`；
   - 写回执，追加 `StatusChanged` 事件。
3. **事务后**：写审计记录 `ActivityRecord`，写法同 `layer1-public/src/rest/admin/runtime_update.rs:38-67`。

revive cause 复用现有的 `RuntimeLost`，靠恢复钉区分，不新增枚举值。retained reopen 也是这样做的。

### 4.2 恢复执行中的两次复核

| 时机 | 复核内容 | 理由 |
|---|---|---|
| `CheckpointVerification` 开始时 | 再读一次原 home 观测。准入时是 `Gone` 的必须仍是 `Gone`（它是单调的，不可能变）；准入时带声明的，原 UID 必须仍是声明的那个 | 与 retained recovery 的“评估之后、提交前再证明一次”一致（`docs/CMA_Retained_Recovery.md`） |
| 建新盘之前 | 原 Sandbox CR 若存在，置为 Suspended，并证明其名下没有 Pod | **防止两个 runtime 同时运行**：原 PVC 被删后，控制器可能用 VCT 重建一个同名的空 PVC 并启动 Pod。复用现有 `AgentSandboxClient::suspend`，它会等到该 Sandbox UID 名下没有 Pod（`layer3-adapter/src/agent_sandbox/suspension.rs:10-44,186-231`） |

即使旧 runtime 还活着，它也拿不到控制面的任何命令：新 generation 已经递增，旧 boot id 会被拒绝（`layer2-core/src/sandbox/runtime_generation.rs:97-121`）。挂起只是为了不让它占资源、不让它在空盘上跑。

---

## 5. 复用与新增

| 现有逻辑 | 位置 | 用途 |
|---|---|---|
| same-image retained recovery 的准入事务、回执、按 key 重放、离开 Failed 的写法 | `layer2-core/src/sandbox/retained_reopen.rs:225-290`、`cma/src/store/retained_reopen/transaction.rs` | **准入事务的模板** |
| admin 运维命令：权限 `ControlAnySandbox`、审计 `ActivityRecord` | `layer1-public/src/rest/admin/runtime_update.rs:32-67`、`admin/sandboxes.rs:177-183` | 路由与审计 |
| Idempotency-Key 提取器 | `layer1-public/src/rest/sandbox_retained_recovery.rs:79-93` 使用的 `idempotency` 模块 | 幂等 |
| 把目标钉进 revive 的写法 | `layer2-core/src/sandbox/runtime_update_resume.rs:7-42` | 恢复钉 |
| 按名字读原 PVC | `layer3-adapter/src/agent_sandbox/pool/native_home_recovery.rs:30-56` | 观测的数据来源 |
| 挂起旧 Sandbox CR 并证明无 Pod | `layer3-adapter/src/agent_sandbox/suspension.rs` | 封旧 writer |
| generation 高水位 | `layer2-core/src/sandbox/runtime_generation.rs:97-121` | 拒绝旧 boot |
| 账户删除的访问拦截 | PR #1827 的共享判定 | owner 已删除时拒绝 |

| 新增 | 层 | 规模 |
|---|---|---|
| 原 home 观测：一次 PVC 读取，返回 5 种分类 | Layer 3 | 一个方法加一个中性枚举 |
| 准入函数：3 种来源状态、证据规则、声明校验 | Layer 2 | 一个函数，结构同 `apply_retained_reopen` |
| 准入事务：Sandbox、备份 catalog、备份调度三处一并提交 | `cma` store | 一个事务，结构同 retained reopen |
| `POST /api/v1/admin/sandboxes/{id}/home-restore` | Layer 1 | 一个路由；admin 路由属于公开契约，两个 SDK 与契约测试要同步更新 |

**不做的**：

| 不做 | 理由 |
|---|---|
| 修改现有失败的分类方式，或新增失败类别 | 准入不依赖类别；改动它会影响所有 revive |
| 改变 404 的重试映射，让卡住的 Reviving 转入 Failed | 影响面是所有 Sandbox 操作；准入直接覆盖卡住的 Reviving 即可 |
| 自动检测 home 丢失并自动恢复 | D10：home 只是暂时不可读时，自动恢复会覆盖健康的 home |
| owner 自助恢复、只读评估接口（retained recovery 有一个 GET） | 首版只有运维使用；拒绝本身是 typed 的，运维和 DR 脚本可以直接用 POST 的结果判断 |
| 新的 revive cause 或事件类型 | 复用 `RuntimeLost` 与 `StatusChanged`；恢复完成的事件已在 R1 中定义 |

---

## 6. 与 #1473 的对照

| #1473 要求 | R2 如何满足 |
|---|---|
| “源不可用时从副本……恢复，不是remount原PVC” | 准入以“原 UID 已不存在”为默认证据；新盘建在新 PVC 上，原 PVC 永不重新挂载 |
| “封旧writer” | 新 authority 让旧 worker 失效；旧 Sandbox CR 被挂起并证明无 Pod；generation 递增拒绝旧 boot |
| QA：“任何恢复依赖人工DB修复、删home或盲重发均FAIL” | 卡住的 Reviving 也能通过 API 准入，不需要改库；原 PVC 不删除；准入本身不重发任何东西 |
| 已核实问题第 4 条：“破坏性 home 损坏” | §3.3 运维声明 |
| TC-087 第 3 步：“以独立副本恢复到隔离环境，原PVC不可读取” | 隔离环境中原 PVC 不存在，观测为 `Gone`，自动准入 |
| QA：“封存……原始结果” | 回执记录证据分类、观测到的 UID、声明、备份 id、操作人、幂等 key；另有审计记录 |
| RTO ≤30 分钟 | 准入本身是一次读取加一次事务。Sandbox 进入可准入状态之前的时间（例如 runtime 丢失后等待 revive 失败）会计入 RTO，须在 TC-087/088 中单独计时（V-R2-3） |

---

## 7. 实现前必须验证的事项

| # | 事项 | 为什么重要 |
|---|---|---|
| V-R2-1 | 在 staging 分别制造四种丢失（删 PVC、删 Sandbox CR、让 PV 丢失、控制器重建空 PVC），记录 Sandbox 最终停在哪个状态、阶段、有无 lease | §1.2 是按代码推断的；准入范围依赖它 |
| V-R2-2 | 原 PVC 被删而 Sandbox CR 仍在时，控制器是否会用 VCT 重建一个空 PVC 并启动 Pod | 决定 §4.2 挂起旧 CR 的必要性与频率。由 infra 在 #1568 中确认 |
| V-R2-3 | DR 环境中，Mongo 恢复后处于 Active 的 Sandbox 要多久才进入可准入状态 | 计入全量 DR 的 RTO；若太慢，再考虑放宽准入（例如接受已确认 runtime 丢失的 Active），本版不做 |

---

## 8. 测试

| 编号 | 用例 |
|---|---|
| R2-1 | 观测分类：读不到、同名不同 UID、带删除时间戳、未绑定、正常、API 错误，各自得到正确分类 |
| R2-2 | 准入：`Paused` 加 `Gone` 成功；任何类别的 `Failed` 加 `Gone` 成功；卡住的 `Reviving` 加 `Gone` 成功 |
| R2-3 | 准入拒绝：`Active`；`Reviving` 但有存活 lease；`Deleting`；`Unreadable`；`Present` 或 `Unbound` 但没有声明；声明的 UID 与观测不符；卡住的 `Reviving` 带声明；备份不可用；owner 已删除；开关关闭；无权限 |
| R2-4 | 幂等：同 key 重放返回原结果；不同 key 返回 409；`expected_revision` 不符返回 409 |
| R2-5 | 事务原子性：备份 catalog 的 pin、备份调度的 fence、Sandbox 的恢复钉三者同时提交或同时不提交 |
| R2-6 | 从卡住的 `Reviving` 准入后，旧 worker 的后续写入因 authority 不符而失败 |
| R2-7 | `CheckpointVerification` 复核：证据变化时恢复失败并给出 typed 原因；旧 Sandbox CR 被挂起且无 Pod 后才建新盘 |
| R2-8 | 审计记录与回执完整 |

# 独立 home 备份与恢复：最小实施方案（#1473）

- Issue：<https://github.com/ChronoAIProject/cma/issues/1473>（P0，截止 2026-11-19，建议负责角色：基础设施负责人）
- 门槛：M-OPS-01 <https://github.com/ChronoAIProject/cma/issues/1553>（截止 11-26）、M-SEC-03 <https://github.com/ChronoAIProject/cma/issues/1550>（截止 11-26）
- 发布规则与负责人决策：<https://github.com/ChronoAIProject/cma/issues/1573>
- 外部依赖：EXT-SANDBOX <https://github.com/ChronoAIProject/cma/issues/1568>、EXT-ATLAS <https://github.com/ChronoAIProject/cma/issues/1565>、EXT-PROVIDERS <https://github.com/ChronoAIProject/cma/issues/1569>（证据截止 11-20）
- 删除级联：<https://github.com/ChronoAIProject/cma/issues/1500>，PR <https://github.com/ChronoAIProject/cma/pull/1827>
- 测试套件：SU-CHAOS <https://github.com/ChronoAIProject/cma/issues/1512>、SU-DATA <https://github.com/ChronoAIProject/cma/issues/1514>

开发者必须先完整阅读 #1473：Background、18 条已核实问题、按日期的交付步骤、实现要点、仓库约束、Affected Files、Verification Checklist（M-OPS-01、M-SEC-03、TC-012/036/064/065/083/087/088/119/136）和 Definition of Done。同时阅读 `CLAUDE.md` 与 `docs/CMA_Data_Migrations.md`。本文与已确定的负责人决策或硬门槛不一致时，以后者为准（#1473「实现要点」首段）。

**“最小”的定义**：#1473 及其指标、用例里写明的每一项要求都必须满足（逐条对照见 §1），在此前提下不增加 Issue 未要求的功能。§10 列出了删掉了什么、为什么可以删，以及为什么剩下的部分不能再删。

---

## 0. 结论

- **备份**：CMA 控制面按 home 编排 CSI VolumeSnapshot。快照类为 `deletionPolicy: Retain`、跨区域存储，由 infra 拥有。catalog 和调度状态放在**两个新 collection**，worker 对 Sandbox 文档**只读**。
- **恢复**：复用 revive 的操作类型、阶段和 authority，把“恢复来源”原子地钉进 revive 转换。校验和 provisioning 两步需要新分支，Failed 状态需要新的准入路径（R2，§0.2 B 组）。**恢复建在新的 PVC 上，原 PVC 永不删除、永不重新挂载**（#1473 QA 任务：“任何恢复依赖人工DB修复、删home或盲重发均FAIL”）。
- **关联**：每个备份记录 Mongo 一致性切点、密钥版本 ID 和删除状态（Background：“关联Mongo、密钥及删除记录”）。备份侧删除时写入 Mongo 之外的 tombstone 副本，DR 恢复时先重放 tombstone。
- **持续演练**：在 `.github/workflows/nightly-staging.yml` 增加受保护的恢复演练，证据绑定审核过的 SHA 和部署身份。
- **Mongo、密钥、文档对象**：配置和证据由 infra 交付。本仓库交付一致性设计、密钥盘点、DR 脚本、覆盖报告和恢复验证 harness。

### 0.1 逾期提醒（截至 2026-11-16，距截止 11-19 剩 3 天）

#1473 中 11-07（风险登记）、11-10（一致性设计）、11-13（隔离恢复环境、权限最小化、DR 脚本、网络隔离校验）三项已经过期。PR-1 先交付其中的文档部分（§2、§6、§7），环境和证据部分由对应角色立即补上。

### 0.2 决策项与建议

**下表的“建议”已落为本方案其余章节的基线写法**，这样方案可以直接开工，不必等齐全部回复。但建议只是方案假设，**不等于负责人已决定**；每一项都要由“决定人”列的角色确认。被否决的项，按它“阻塞”列指向的章节回改方案。

“阻塞”列写明不决定会卡住什么；“最晚”按 11-19 截止和 EXT 依赖 11-20 证据截止倒推。工程侧不得自行降级任何硬门槛口径。

**A. 阻塞捕获（PR-1，这是唯一还有可能按期交付的部分）**

| # | 事项 | 建议 | 决定人 | 阻塞 | 最晚 |
|---|---|---|---|---|---|
| D5 | Sandbox 删除后备份保留多久 | **随 Sandbox purge 一起删除**（purge 本身已有 grace 期）。这样没有备份比 Sandbox 记录活得更久，catalog 不需要 owner hash 字段，也不存在“已删 Sandbox 被旧备份复活”。若要保留到 ≤30d，则需加 owner hash、单 Sandbox tombstone，并让 #1500 的 erasure catalog 能按 owner 找到这些备份 | 法务/产品（#1551/#1501） | PR-1 的 schema（是否要 owner 字段），改晚了要做数据迁移 | 11-17 |
| D2 | 远端集群 RBAC | 按 §7.1 只申请 namespaced 的 `volumesnapshots` create/get/list/delete。恢复走 template 路线，**不需要新的 PVC 权限**。代码和测试可先用 fixture 写，不等授权 | infra（#1568） | PR-1 的生产启用与捕获证据 | 11-18 |
| D9 | K8s 快照对象数量 | 先按“保留 K8s 对象”实现（最简单），同时请 infra 给出远端集群的对象数上限。超限再改为只留 handle、按需重新导入 | infra（#1568） | PR-1 的生产启用规模，不阻塞代码 | 11-18 |
| D1 | RPO/RTO 档位 | **按 A 档（30min 捕获/30min 恢复）实现并据此交付证据，不等决策**：#1553 写明“M-OPS-01 原数字保持”，A 档就是现有数字，改档位只动一个配置值。但若产品倾向 B 档（每日加活跃每小时），必须**今天**说明——那时 infra 的 GCE 快照计划可能替代 PR-1 的大部分捕获逻辑（§10.3） | 产品负责人（#1573） | 不阻塞；但晚说会白做 | 11-16 |

**B. 阻塞恢复（PR-4，按期完成的可能性低）**

| # | 事项 | 建议 | 决定人 | 阻塞 | 最晚 |
|---|---|---|---|---|---|
| R1 | 恢复出的 home 落后 Mongo 最多 1 小时，系统没有处理这种状态的机制 | **设计草案（最小版，2026-11-16 第二版）见 [R1-DESIGN.md](R1-DESIGN.md)，已批准用于开发（2026-11-16）。** 结论：保留 home 用户内容，在 runtime 启动前隔离并重置其内部状态（复现历史 checkpoint 恢复的前提）；控制面沿用历史 checkpoint 恢复的结算语义，只新增三条规则（以清点判定覆盖、身份重绑、删除重放）。仍有 6 项需实测（R1-DESIGN §7），其中 V1 是基础；另请 ledger 负责人确认格式屏障随旧 ledger 移入隔离目录是否安全（R1-DESIGN §5） | 后端负责人（评审）+ ledger 负责人（屏障确认） | PR-2、PR-4r、PR-4 | 11-18 |
| ~~D13~~ | ~~批准“从备份恢复”为连续性表述的例外~~ | **已撤回（2026-11-16）**：逐条核对原文与上下文后，7 处表述无一被违反（R1-DESIGN §5）。只需在 `docs/CMA_Home_Backup.md` 中澄清“home 备份恢复不恢复 ledger、不是镜像回滚、只能由管理员显式准入”，并在相关 ledger 文档加交叉引用 | — | — | — |
| D14 | 自定义镜像的 Sandbox 能否恢复 | 恢复用该 Sandbox 记录的镜像；镜像里的 runtime 不支持 `home_restore_v1` 时只能 typed 拒绝。建议：首版只承诺 managed 镜像可恢复，自定义镜像在证据与公开文案中如实列为“需重建镜像后才可恢复”，不计入可恢复覆盖率的分子 | M-OPS-01 门槛负责人 + 产品 | M-OPS-01 的覆盖口径 | 11-20 |
| R2 | 恢复的准入：哪些 Sandbox 可以恢复、凭什么证据 | **设计草案已出（2026-11-16），见 [R2-DESIGN.md](R2-DESIGN.md)，已批准用于开发（2026-11-16）。** 结论：准入不看失败类别（`provisioning_failed` 也涵盖与 home 无关的失败，而且现有读取把丢失原因折叠成了一条字符串）；只看两件事——Sandbox 已停下（`Paused`、任何类别的 `Failed`、**卡住的 `Reviving`**），以及一次 PVC 读取证明原 UID 已不存在。后者是单调事实。原 PVC 仍在但损坏时，需运维声明并绑定当前 UID。卡住的 Reviving 必须覆盖：home 丢失后 Sandbox 可能一直重试或被停放，不会进入 Failed，否则只能人工改库。准入事务照搬 same-image retained recovery。仍有 3 项需实测（R2-DESIGN §7） | 后端负责人（评审） | PR-2（回执形状）、PR-4 | 11-18 |
| D3 | 从快照建新 PVC 的路线 | 按“恢复专用 template + dataSource”实现：仓库里 legacy seeded home 已在用同样的 VCT 形状（`pool/model.rs:203-209`），所以不需要 PVC create 权限。infra 在远端做一次最小验证：从一个快照建出 PVC 并挂载，确认 hyperdisk 加 CMEK 下可用、`initialize-home` 不会清空已恢复的数据 | infra（#1568） | PR-4 的 provisioning | 11-18 |
| D10 | 恢复的触发方式与 owner 可见性 | **首版只做 admin 命令触发，不做自动恢复**：home 只是暂时不可读或变慢时，自动恢复会用新盘覆盖本来健康的 home。owner 在 Sandbox 详情看到 typed 状态说明（存储故障、运维处理中），不提供 owner 恢复按钮。#1473 没有要求自助恢复；CLAUDE.md §5 关于“不需要 owner 点击修复”针对的是部署，不是存储丢失 | 产品 + 后端负责人 | PR-4 与 web 文案 | 11-18 |

**C. 阻塞删除关联（PR-5）**

| # | 事项 | 建议 | 决定人 | 阻塞 | 最晚 |
|---|---|---|---|---|---|
| D8 | 恢复重放 tombstone 由谁交付 | 机制（格式、有期限的 create-only 写入器、枚举、重放工具、port）由本方案交付；账户删除的调用点在 #1500 的级联里。**每个删除事件只能有一个写入方。** 完整论证见 §5.4.1 | #1500 负责人 + 安全负责人 | PR-5；#1500 的 11-19/11-20 演练要用 | 11-17 |
| D11 | Atlas 备份保留期是否 ≤30d | 确认 Atlas 的快照保留与 PITR 窗口都 ≤30d。若因合规需要更长，必须作为“受法规保留例外”在注册前准确披露，不能默认扩大保留（M-SEC-03）。§6 的数据类表按 ≤30d 写，需要 infra 确认现状 | infra（#1565）+ 法务 | M-SEC-03 的达标口径 | 11-20 |

**D. 影响验收口径与归属**

| # | 事项 | 建议 | 决定人 | 阻塞 | 最晚 |
|---|---|---|---|---|---|
| D7 | “可读副本”的判定口径 | 按 Issue 原文结构：100% home 用**云端快照清单**（不是 Mongo catalog）核对副本存在、状态与完成时间，另抽 10 个 home 实际恢复，nightly 对 fixture 演练（§5.7）。不做“每个生产 home 定期恢复校验”——Issue 没要求，还会把客户数据复制到另一个环境 | M-OPS-01 门槛负责人 | PR-4b 的工作量与证据口径 | 11-18 |
| D4 | 快照存储位置与 CMEK key 位置 | 快照类用多区域存储位置；CMEK 用多区域 key 或在恢复区域有可用副本的 key。infra 需给出：hyperdisk-balanced 快照是否支持跨区域存储位置、CMEK 快照能否在另一区域恢复 | infra + 安全负责人 | TC-088 的区域丢失场景，不阻塞 PR-1 代码 | 11-20 |
| D6 | 本工作项的执行归属 | **拆成两个负责人**：infra 负责快照类、远端 RBAC、配额、区域与 CMEK、清单脚本身份；后端负责 PR-1~PR-5 的代码。#1473 现在只写“建议负责角色：基础设施负责人”，但工作量主要是 Rust 后端 | 项目负责人 | 全部 | 立即 |
| D12 | fallback 的判定点与判定人 | **定 11-18 23:59 SGT 为判定点**：若 D3 未验证或 R1 无设计结论，则 11-19 由产品加门槛负责人启动 #1473 写明的替代路径（明确缩小公开的持久性承诺），而不是到 11-19 当天才发现做不完。工程侧不得自行降级 | 产品 + M-OPS-01 门槛负责人 | 决定 11-19 的交付范围 | 立即 |

### 0.3 我的整体判断

- **捕获、保留、到期（PR-1、PR-3）**：只要 D5、D2、D9 在 11-17 到 11-18 给出结论，按期交付可行。
- **恢复（PR-4）**：按期完成的可能性低。它要求 PR-2 先部署一个版本（严格 reader 的发布顺序），再加上 R1 和 R2 两个尚无结论的设计问题。建议现在就按 D12 准备替代路径，不要等到 11-19。
- **优先级**：今天先定 D6（归属）、D1（档位）、D12（判定点），再把 D2、D3、D4、D9 一并提给 infra（#1568 的证据截止是 11-20），同时请 #1500 负责人确认 D8。

### 0.4 需要先查清的事实（不是决策，但缺了就没法实现）

| 事实 | 为什么要 | 怎么查 |
|---|---|---|
| 生产中 retained 的 legacy home（`cma-home-v1`）数量 | 为 0 就不实现 legacy 路径（§5.1），能省掉一条代码路径 | 只读查询，PR-1 第一步 |
| home 丢失后 Sandbox 实际停在哪个状态 | R2 已改为不依赖失败类别，但准入范围依赖“会停在 Failed 还是卡在 Reviving”。代码推断见 R2-DESIGN §1.2 | staging 实测（R2-DESIGN V-R2-1） |
| 远端集群当前的 VolumeSnapshot 对象数与配额余量 | D9 的上限判断 | infra 只读查询 |
| 单盘快照的最小间隔与项目快照配额 | 30min 间隔是否触发限流（§8 E 段） | infra 查云端配额 |

---

## 1. #1473 需求逐条对照（最小方案的验收清单）

### 1.1 Background 与 Purpose

| 要求 | 满足方式 |
|---|---|
| 备份脱离原 PVC 故障域 | §5.1 Retain 类、跨区域存储，catalog 记录 provider handle |
| 关联 Mongo、密钥及删除记录 | §3.2 `cut`、`custody` 字段；§5.4 tombstone 及其 Mongo 外副本 |
| 每 home RTO ≤30min | §5.5 恢复流程；§9 harness 实测 |
| 综合灾备 RPO ≤1h / RTO ≤4h | §6 一致性切点；§7.4 DR 脚本；全量演练由 M-OPS-06/TC-088 执行 |
| 每份证据包含 digest、SHA、环境、时间窗、N、开关、原始结果位置 | §9.3 证据模板，nightly 产物强制包含这些字段 |

### 1.2 18 条已核实问题的归属

#1473 写明“其余问题的修复建议写在上面各自标注的 issue 里”。本方案交付 #1473「实现要点」逐条列出的第 1–8 项建议；第 9–18 项只处理与备份相交的部分。

| # | 问题 | 本方案处理 |
|---|---|---|
| 1 | PVC 是生产 home 的唯一副本 | **交付**：经测试、带保留策略和 ownership fencing 的 CSI snapshot（§5） |
| 2 | 客户 home 无独立备份，未验证集成恢复/DR | **交付**：独立备份、metadata/key 关联、Atlas PITR 配置核验、恢复 identity/fence、region 丢失流程、checksum 和原始会话证据的隔离恢复；备份删除独立于 PVC reclaim（§5–§7、§9） |
| 3 | 持久 Agent home 无已验证的独立备份 | **交付**：按保留、权限归属和恢复契约测试还原，覆盖加密/访问策略和身份验证（§5.5、§7.1） |
| 4 | retained home 连续性无独立副本 DR 验证 | **交付**：带 fence 的恢复，保持身份安全，包含凭据保管处理（§5.5 第 5 步） |
| 5 | 替换加密密钥可能使保留记录不可读 | **交付备份相关部分**：密钥盘点、独立托管、恢复演练（§7.2、§9）。不声称已有 auth-key 轮换工具（与 #1473 原文一致）。轮换迁移归 #1550 |
| 6 | checkpoint 承诺下的第二副本未验证 | **交付**：独立备份和恢复验证；修正把历史 checkpoint 写成现有能力的文档（PR-1） |
| 7 | 消费者条款、保留政策、完整数据删除未确立 | **交付备份侧**：备份 ≤30d 到期、账户删除时备份清理、tombstone 重放（§5.4）。条款和自助删除入口归 #1499/#1500 |
| 8 | staging 未持续开展安全、依赖和恢复演练 | **交付恢复演练**：nightly 受保护演练、脱敏证据、绑定 revision，结果可作为发布门禁输入（§9.2）。安全和依赖演练及门禁接线归 #1545 |
| 9 | GET 配额可归并完整日志、读取大量 artifact | 归 #1545，本方案不改 |
| 10 | 远程 sandbox 网络隔离未验证 | **交付 #1473 11-13 任务中的部分**：校验远端 runtime 和隔离恢复环境中**实际生效**的网络隔离（§7.3）。整体归 #1549 |
| 11、14、17 | Document 数量、字节、revision 上限与删除 | 归 #1544/#1500。本方案只要求 GCS 文档对象纳入备份覆盖和删除期限（§7.2） |
| 12 | 64GiB homes 持续累积成本 | 归 #1544。本方案测量并上报备份成本（§9.4） |
| 13 | Secret IAM 与轮换权限在 IaC 外 | 归 #1550。本方案在 §7.1/§7.2 列出备份依赖的 IAM 和密钥授权，由 infra 纳入管理 |
| 15 | 集中日志保留与 PII | 归 #1550。本方案要求备份 worker 不记录 handle 原文、owner 原文或任何 secret（§8） |
| 16 | 供应链 gates | 归 #1557，不改 |
| 18 | 共享数据库凭据 | 归 #1550。新 collection 登记到 isolation 目录（§3） |

### 1.3 按日期的交付步骤

| 日期/任务 | #1473 要求的交付物 | 本方案对应 |
|---|---|---|
| 11-07 风险登记（SRE） | 只复用 2026-11-06 只读观测数据；Atlas 余量、资金、值班、备份权限登记为未知；按三条主线排责任；未知项禁止签署 | §2 风险登记（PR-1） |
| 11-10 独立备份与密钥恢复路径（数据负责人） | Mongo PITR、home 独立副本、document objects、key custody 的一致性设计；以“独立 home 副本 0”和 CLAUDE.md 已停 S3 捕获为起点（该句现位于 `CLAUDE.md:517`）；不把旧 archive 记录当现有备份 | §6 一致性设计、§7.2 密钥盘点（PR-1） |
| 11-13 home 独立副本与权限最小化（infra） | 隔离恢复环境实现捕获 ≤30min、复制 ≤30min、Mongo PITR、密钥托管；10 个 home、≥3 类恢复样本；完整 DR 脚本；校验远端 runtime 网络隔离（仓库 manifest 不等于实际策略） | §5（PR-1/3/4）、§7.1 权限矩阵、§7.3 网络隔离、§7.4 DR 脚本（PR-1 起草，PR-4 可执行）、§9 harness |
| 11-19 密钥恢复和删除全链路（安全） | 合成加密记录的新旧 key 托管恢复，0 不可读、0 secret 日志泄漏；Mongo/PVC/GCS/logs/backups 的 owner 与期限；立即撤权；live ≤7d、backup ≤30d；restore 重放 tombstone；backup 长周期用可控时钟加真实 retention 配置；live 删除 10/10 真实完成 | §5.3–§5.4、§9.1 harness 用例 K1–K3 和 T1–T3；live 删除的 10/10 归 #1500/TC-136，本方案提供备份侧回执 |
| 11-19 证据完整性审查（QA） | 封存第 10 次 scheduled、10 个 home 恢复和首个 DR/兼容修复的原始结果；检查 dirty file、cursor、撤权、外部副作用收据；依赖人工 DB 修复、删 home 或盲重发即 FAIL | §9.2 nightly 演练；§5.5 不删 home、不盲重发；§9.3 证据封存 |

### 1.4 实现要点、仓库约束、Affected Files、DoD

- **实现要点 1–8**：见 §1.2 第 1–8 行。
- **仓库约束**：
  - 分层落点见 §3；不新增根目录文件和 `infra/` 文件；
  - 部署面变化在同一组提交中更新 `infra/`（§8）；
  - 全部 CI 门禁、成功和失败测试、小步提交、`Closes #1473`、不提交密钥（§11）。
- **Affected Files**：#1473 要求“实现时以实际改动为准，并在首个 PR 中列全”。逐文件处理见 §4，PR-1 正文必须完整列出（PR-1 即首个 PR）。
- **DoD**：
  - 交付物和相关任务产出链接回 #1473；
  - 用 `/measure M-OPS-01 …` 和 `/measure M-SEC-03 …` 把证据贴到 #1553/#1550；
  - CI 全绿、`infra/` 与代码一致、文档和 `CLAUDE.md` 已更新；
  - 成功和失败路径都有自动化测试（§9.1）。

### 1.5 指标与用例

| 指标/用例 | 本方案提供的实现与证据 |
|---|---|
| M-OPS-01 覆盖率 100%（home 加 DB/key/object） | §9.4 覆盖报告：逐 home join catalog，加上 Atlas PITR 窗口、密钥版本和 GCS 复制状态 |
| 每 ≤30min 捕获、≤30min 完成；integrated RPO ≤1h；Mongo 点 ≤5min | §5.2、§6；harness 分别记录 Mongo 恢复耗时、home 恢复耗时和 full DR RTO |
| 10 个 home、≥3 类、RTO ≤30min，hash/owner/native id/receipts 100% 匹配 | §9.1 RS 系列用例，以及 R1-DESIGN §8、R2-DESIGN §8 的用例；样本覆盖 active/paused/长历史，以及 warm/cold/resumed/retained 版本 |
| hourly 7d / daily 30d，与删除政策一致 | §5.3 |
| 备份开销纳入峰值负载与成本 | §9.4 |
| TC-064 | §9.1 K1–K3：合成加密记录用新旧 key 在隔离环境恢复后可读；错误 key 被拒绝；日志和 artifact 中 0 secret |
| TC-065 | §9.1 T1–T3：从删除前的备份恢复时重放 tombstone，0 复活 |
| TC-083 | 备份 worker 不增加 disruption；节点 drain 时已挂载盘的快照继续或取消后重排；home hash 不变（§8 C 段） |
| TC-087 | §9.1、§9.4、§5.7 |
| TC-088 | §7.4 DR 脚本、按 handle 恢复（§5.1）、sweep 并发配置（§8 E 段）、PITR 回退后的孤儿快照对账（§5.3）；全量演练执行归 M-OPS-06 |
| TC-012/036/119/136 | 归文档、删除、导出的负责 issue；本方案提供备份侧的删除回执和保留期限说明 |

---

## 2. 风险登记（11-07 交付物，只用 2026-11-06 只读观测数据）

| 主线 | 事实（11-06 只读观测） | 未知项（标为未知，禁止签署） |
|---|---|---|
| 性能/可用性 | AM 1 副本，CPU 3.258–3.550/4 cores；Mongo 默认 10 连接/server | Atlas 余量（#1565） |
| 正确性/安全 | 55 PVC、3358GiB，Delete reclaim，独立 home 副本 0；S3 capture 已退役 | 远端备份权限（#1568）；快照存储与 CMEK 位置（D4） |
| QA/观测/发布 | Atlas 备份/PITR 与恢复 RTO 未测量 | 资金、值班（#1546/#1555）；nightly 恢复演练连续性（#1545） |

---

## 3. 分层落点与数据模型

```
layer3-adapter/src/
  home_snapshot.rs                               扩展：storage_handle、creation/ready 时间
  agent_sandbox/pool/native_home_snapshots.rs    新增：native home 快照 create/observe/delete
  agent_sandbox/pool/native_home.rs              新增 validate_restored_volume
  agent_sandbox/pool/model.rs                    恢复用的 VCT/claim 形状（按 D3 结论）
layer2-core/src/sandbox/home_backup/             新增，纯领域
  model.rs      HomeBackupSchedule / HomeBackup / 状态机
  retention.rs  保留与到期计算（纯函数，可控时钟）
layer2-core/src/sandbox/home_restore.rs          revive 的恢复钉
cma/src/store/home_backups.rs (+ model.rs)       新 collection、索引、fenced 写入
cma/src/sandbox/home_backup/                     worker：调度、捕获、到期、tombstone 副本
layer1-public/src/rest/admin/home_restore.rs     唯一的新 admin 路由：恢复命令
```

- Layer 2 中不得出现 VolumeSnapshot、GCE、Kubernetes 等名字（CLAUDE.md §3、§9）。
- 两个新 collection 登记到 isolation 目录和 retirement-archive census（参照 PR #1827）。
- PR 中按 `docs/CMA_Data_Migrations.md` 写 data migration record。

### 3.1 `home_backup_schedules`（每个 retained home 一行，`_id = sandbox_id`）

| 字段 | 说明 |
|---|---|
| `home{mode, pvc_uid, home_identity, storage_reference}` | 计划时观测，每次外部调用前复核 |
| `state` | `active` / `retiring` / `erasing` / `restoring` / `closed` |
| `next_due_at`、`last_ready_at`、`last_ready_backup_id` | 调度、覆盖率 |
| `lease{holder, token, fence, expires_at}`、`next_fence` | §8 A 段 |
| `in_flight{backup_id, phase, started_at}` | 每个 home 同一时刻只有一个捕获 |
| `revision` | 文档 CAS |

**没有 owner 字段。** 按 D5 基线，备份随 Sandbox purge 一起删除，所以不存在比 Sandbox 记录活得更久的备份，按 owner 查备份这件事不会发生；owner 归属始终从 `sandboxes` 取。若 D5 改为“保留到 ≤30d”，则两个 collection 都要加 owner hash（沿用 #1827/#1864 的约定，该约定目前不在 develop 上），并补一次数据迁移。

**schedule 行由谁创建**：捕获 worker 之外还需要一个补建扫描。按 `_id` keyset 分页遍历 `sandboxes`（同 `cma/src/store/sandbox_reconciliation.rs:100-138`），对 home 存在且未释放的 Sandbox upsert 一行 schedule，`next_due_at` 设为立即到期。覆盖率的分母始终取自 `sandboxes`，不取自 schedule 表，否则漏建的 home 会从报告里消失。新 home 从创建到首次捕获的最长延迟等于扫描周期加捕获间隔，要在证据中报告。

### 3.2 `home_backups`（catalog，`_id = "hb-{sandbox_id}-{epoch}"`，确定性）

| 字段 | 说明 |
|---|---|
| `source{pvc_uid, home_identity, runtime_generation, runtime_version, boot_id}` | 捕获时的身份 |
| `cut{requested_at, provider_creation_time, ready_at}` | integrated RPO |
| `storage{snapshot_name, snapshot_class, storage_handle, restore_size_bytes}` | 按 handle 跨集群恢复 |
| `custody{cmek_key_version_id, auth_key_ids[]}` | 只存 ID，不存值 |
| `retention{class, expires_at}`、`state`、`restore_pins` | §5.3、§8 D 段 |
| `erasure{erased_at, receipt}?` | 删除回执，交给 #1500 的 ledger |

---

## 4. Affected Files 逐项处理（PR-1 正文须完整列出）

| 文件 | 处理 |
|---|---|
| `docs/Sandbox_Provider.md` | 修改：:92-105 增加独立备份与恢复的边界说明，改正“PV 是唯一持久性依据”的表述 |
| `docs/CMA_Operations.md` | 修改：配置键、保留策略、DR 脚本和 runbook、告警 |
| `infra/storage/home-seed.yaml` | **核对，不改**：其中 `cma-home-v1` 快照类为 `deletionPolicy: Delete`，只用于 legacy seed，不能当备份。Retain 类由 infra 仓库拥有。PR 中写明理由 |
| `docs/Release.md` | **PR-1 只加一处交叉引用**（指向 `docs/CMA_Home_Backup.md`）。:995 那条“证明 Atlas backup/PITR 与恢复路径前生产留在 v0.7.0”是保护性门禁，本身是对的，**不在文档整理里修改**；要改（例如加入恢复演练的证据要求）只能在证据齐备后由 PR-6 单独提出并写明理由 |
| `docs/CMA_Owned_Continuity.md` | 修改：增加“从独立副本恢复”的连续性语义（ID 保持、切点后事件可见、不盲重发） |
| `docs/CMA_Model.md` | 修改：:325-349 标为历史；描述新的备份和恢复模型 |
| `infra/api/secret.yaml` | 核对：不新增 secret；为 `CHRONO_AUTH_ENCRYPTION_KEY`（:30）补充托管引用说明（不含值） |
| `docs/CMAEG_Operations.md` | 修改：`CMAEG_MASTER_KEYS` 的 keyring 托管（:935-942）纳入密钥盘点，引用恢复证据 |
| `docs/CMA_Billing_Pilot.md` | 核对：:67-119 的计费密钥轮换流程纳入密钥盘点，只加引用 |
| `api/crates/cma/src/store/checkpoint_storage.rs` | **不改**：退役的 archive 存储继续拒绝；新备份不复用它，以免把历史记录当成现有备份（11-10 任务要求） |
| `docs/Document_Artifacts.md` | 修改：说明文档 GCS 对象的备份、复制和 ≤30d 删除期限 |
| `api/crates/layer1-public/src/rest/documents/mod.rs` | 不改：文档 DELETE 归 #1500/#1544 |
| `web/src/DeletionDialog.tsx` | 不改：删除 UI 归 #1500；该处如需说明备份保留期限，由 #1500 按 D5 修改 |
| `api/crates/layer2-core/src/platform.rs` | 不改：备份参数用部署配置，不做运营可调设置（最小化） |
| `api/crates/layer2-core/src/sandbox/agent_reads.rs`、`api/crates/layer1-public/src/rest/agent_reads.rs`、`api/crates/cma/src/store/resource_access/artifacts.rs`、`api/crates/layer2-core/src/sandbox/agent_artifacts/service.rs` | 不改：问题 9 归 #1545 |
| `infra/api/configmap.yaml` | 修改：新增配置键（§11） |
| `docs/CMA_Deployment_Readiness.md` | 修改：:445-452 的“独立快照超出范围”和 :478-486 的远端权限表 |
| `api/crates/layer3-adapter/src/agent_sandbox/pool/model.rs` | 修改：恢复用的 VCT/claim 形状（D3） |
| `api/crates/layer2-core/src/document_artifacts/service.rs` | 不改：文档配额和删除归 #1544/#1500 |
| `docs/CMA_Sandbox_Usage.md` | 修改：说明备份存储不计入用户的 Sandbox 用量，由运营成本单列（M-SUST-02） |
| `infra/api/secretproviderclass.yaml` | 核对：DR 环境需要同一组 Secret Manager 引用和 IAM；如 DR 要求固定版本，由 #1550 决定，本方案只记录 |

除上表外的新增或修改文件，见 §3、§8、§9。

---

## 5. 备份与恢复流程

### 5.1 存储机制

- **快照类**（infra 拥有，按名字配置）：`pd.csi.storage.gke.io`，`deletionPolicy: Retain`，跨区域或恢复区域存储，加密 key 在恢复区域可用（D4）。参数以 #1568 实测为准。
- **catalog 记录 `snapshotHandle`**。整集群丢失时，在恢复集群按 handle 预置 VolumeSnapshotContent 后恢复。
- **Retain 类保证备份独立于 PVC 的 Delete reclaim**，以及 namespace 或 Sandbox 的删除。
- **legacy northeast homes**（`cma-home-v1`）：复用 `provider/home_snapshots.rs`，改用 Retain 类。PR-1 先只读统计生产中 retained 的 legacy home 数量，为 0 就不实现，并在证据中记录这个统计。
- **一致性**：PD 快照是 crash-consistent；会话一致性由 §5.5 的对账保证。

### 5.2 捕获（每个外部步骤之前先持久化）

1. **Claim**：`find_one_and_update` 取出到期 schedule，写入新 lease token，`fence = next_fence++`。
2. **Plan**：只读 Sandbox，确认 `pvc_uid`/`home_identity` 未变。在同一个 fenced 写入中设置 `in_flight` 并插入 catalog（`requested`），同时记录 `custody` 版本 ID（来自配置，不需要额外 I/O）。native id 不复制进 catalog：恢复时直接用恢复出的 home 内容与 Mongo 中的 Agent 记录比对，每次捕获可省一次 Agent 查询。
3. **Create**：复核 fence 和 PVC UID 后 create，再 fenced 写入 `creating`。非首次 claim 只按确定性名字认领，不重建（语义同 `checkpoint_maintenance/export.rs:247-280`）。认领时用 GET 按名字查。不要沿用现有的 `snapshot_inventory()`：它分页 list 全部快照（`provider/home_snapshots.rs:32-48`），在每个 home 近 200 个快照时代价过高。
4. **Observe**：每次一个 GET。`readyToUse == true` 且 VolumeSnapshotContent 上有 handle 时，写 `ready`、计算到期、推进 `next_due_at`、释放 lease。未就绪等下一 tick。这一步看到的是 K8s 侧的状态。CMA 只有 K8s 客户端，没有云 API 客户端，所以“副本确实存在于独立存储”的核对不在 CMA 内做，由 §9.4 的清单脚本用 infra 身份直接查云端快照，再与 catalog 比对。
5. **失败**：超过 30min 记为 `failed{typed reason}`，计数并告警。

### 5.3 保留与到期（与删除政策一致）

- 每小时第一个 ready 点保留 7d；每个 SGT 自然日第一个 ready 点保留 30d；其余点在出现更新的 ready 点后再保留 2h。
- `expires_at ≤ ready_at + 30d` 写死并校验，配置不能放宽。
- **到期**：`{state:ready, expires_at ≤ now, restore_pins:0}` → CAS 改为 `expiring` → 删除快照和底层副本 → 通过 provider 独立接口确认已不存在 → `deleted`，并保留删除回执。
- 长周期验证用可控时钟，**加上**真实的 provider retention 配置和真实的删除回执（#1473 11-19 任务）。
- **孤儿快照对账**：每个快照带 `cma/backup-of=<sandbox hash>` 和 `cma/backup-id` 标签。对账分两层：CMA 内只能看到 K8s 对象，按标签分页列出后与 catalog 比对；云端快照的对账（集群丢失后 K8s 对象不在、底层快照仍在）由 §9.4 的清单脚本完成。比对规则相同：
  - 有 tombstone 的直接删除；
  - 其余按 `creationTime` 补登进 catalog，并套用同一套到期规则。
  - 需要这一步的场景：Atlas PITR 回到 T 之后，T 之后创建的快照不在 catalog 里；或某次 catalog 写入随 Mongo 回滚丢失。没有对账，这些快照会超过 30d，违反 M-SEC-03。对账遵循 `docs/CMA_Data_Migrations.md`「跨存储顺序用持久回执加对账」。

### 5.4 Sandbox 退役、账户删除与 tombstone

- **Sandbox 退役**：native home 的 PVC 在 `TeardownRuntime` 步骤就开始被删除（删除 Claim 和 Sandbox CR 后由控制器回收，`cma/src/sandbox/worker/retirement.rs:126-215`）。现有对 `background_checkpoint` 的等待在其后的 `MarkRetired`（`:217-238`），对 native home 来说太晚。所以必须在 `TeardownRuntime` **之前**（`StopAgents` 步骤）把 schedule 改为 `retiring` 并自增 fence，用 `find_one_and_update` 拿到更新后的文档；如果有 `in_flight`，有期限地等它结束或取消，再继续。捕获 worker 的每次 fenced 写入都带 `state: active` 条件。这样两边在同一个 schedule 文档上串行，不会出现“先检查后动作”的竞态。
  退役时**不立即删除备份**：此时 home 刚被销毁，备份是唯一副本，误删 Sandbox 的恢复窗口全靠它。按 D5 基线，备份在 `Purge` 步骤随 Sandbox 记录一起删除（purge 本身已有 `purge_after` grace 期，`retirement.rs:260-263`），schedule 行同时删除。所以备份不会比它的 Sandbox 记录活得更久。
- **账户删除**：#1500 级联在 PVC delete 步骤之后调用本方案提供的 `erase_home_backups(subject)`。owner 的 Sandbox 列表由调用方传入或从 `sandboxes` 现查（catalog 没有 owner 字段，见 §3.1）：
  1. 每个 schedule 进入 `erasing` 并 fence，取消在途捕获；
  2. 删除该 owner 的全部备份，**立即删除，不等 30d 到期**。这比“≤30d”更严格，也让删除回执能在 #1500 的 7d 期限内完成；
  3. 返回逐个备份的删除回执给 #1500 的 ledger。
  - 这一步**不写 tombstone**：home 备份已被立即删除，没有可供复活的 home 副本。恢复重放 tombstone 针对的是 Mongo 恢复，由 §5.4.1 单独处理。
- 两个新 collection 必须登记进 #1500 的 erasure catalog：PR #1975 要求 catalog 与 `isolation::COLLECTIONS` 一一对应。

### 5.4.1 恢复重放 tombstone（D8）

**名字容易误导。** 它要防的不是“home 备份把 home 复活”，而是“**Mongo 恢复到删除之前的时间点，把整个账户复活**”。

如果 D5 采用建议默认（账户删除时立即删除该 owner 的全部 home 备份），删除之后就没有任何 home 副本可用于复活，home 这一数据类自身不需要 tombstone。真正会复活的是 Mongo 里的记录：subject、授权、分享链接、文档元数据，**以及 #1827 的访问拦截本身**（它读 `account_deletions` 这个 collection，恢复到删除前，这些行不存在，拦截也就消失了）；再加上仍在 soft-delete 窗口内的 GCS 文档对象，和尚未销毁的密钥版本。

所以这是 Mongo、GCS、credentials 三类数据的问题。本方案需要它的原因有两个：DR 脚本的一个步骤（§7.4 第 4 步），以及自己的恢复入口要据此拒绝（§5.5 第 1 步）。

**什么时候会恢复到删除之前**：

1. 区域级灾难后恢复到最近的一致点，integrated RPO 最长 1 小时，这一小时内被接纳的删除不在恢复出的 Mongo 里。
2. 更严重的情况：为了修复较晚才发现的损坏而恢复较旧的快照。Mongo 备份保留 30 天，恢复一个 20 天前的快照，会把这 20 天内删除的账户全部复活。`docs/CMA_Data_Migrations.md:217-232` 把“受保护的备份加 dry-run”写成不可逆变更前的必要步骤，所以旧快照恢复是被认可的运维手段，不是异常操作。

**tombstone 必须做到六件事**：

| # | 要求 | 原因 |
|---|---|---|
| 1 | 不依赖被恢复的那个存储就能**枚举** | 重放要列出某个时间窗内的全部删除。只能按 key 精确读是不够的，因为 key 存在 Mongo 里，而 Mongo 刚被回退 |
| 2 | 写明删除了什么：subject hash、sandbox id、backup id、文档对象 key、凭据和密钥 id | 重放要能撤销旧身份的访问、找到并重新删除被恢复出来的资源、并给出 0 残留的对账依据 |
| 3 | 记录申请时间和完成时间 | 判断哪些恢复出的状态早于删除因而必须重新删除；事后核验 ≤7d/≤30d 的 SLA |
| 4 | 重放幂等 | DR 过程中可能重放多次 |
| 5 | **先拒绝，再对外服务**：访问撤销必须在恢复出的控制面接受任何旧身份请求之前生效 | 否则已删除用户的分享链接和 scoped key 会重新可用一段时间 |
| 6 | 单 Sandbox 恢复入口也要据此拒绝，不只是全量 DR | 恢复入口是一个 admin 命令，随时可能被调用 |

**写入顺序**：先写存储外副本，成功后再提交 Mongo 事务。反过来，进程在两步之间崩溃会留下一个没有重放记录的删除。

**保留期**：有期限，取所有备份保留期（Mongo 备份、快照、GCS 版本窗口）中的最大值，到期即删除。理由是一旦不存在早于该删除的备份，就无法复活，tombstone 也就没有用了。内容只有 hash。**不能用永久保留**：对一个假名标识符做无限期保留，按 M-SEC-03 属于需要在注册前披露的保留例外。

**不能复用 `permanent_custody.rs`**（已核实）：它没有 list API，并且明确把 `storage.objects.list` 列入拒绝权限（`layer3-adapter/src/permanent_custody.rs:2`、`permanent_custody/gcs.rs:10`），`read()` 必须凭 `ObjectReceipt` 精确读取，而 receipt 存在 Mongo 里，恢复后就没有了，正好违反要求 1；它还是 `permanent_hold: true`，并拒绝 `setRetention`，无法做有期限保留。它的审计和只读校验思路可以借鉴，实现要另写。

**分工建议**：

| 部分 | 归属 | 理由 |
|---|---|---|
| 格式、有期限的 create-only 写入器、枚举、重放工具、`DeletionReplayPort` | **本方案（#1473）** | 消费方都在这里：DR 脚本和恢复入口。而且 #1500 的 PR #1975 已明确把 protected tombstones、restore replay、backup expiry 列为未交付 |
| 账户删除时的调用 | **#1500 的级联** | 它持有权威的删除事务，顺序要求（先写副本、再提交）只能在那个事务里满足。每个删除事件只能有一个写入方 |
| 备份侧自身删除的调用 | 本方案 | §5.4 |
| 披露文案与保留例外 | #1551/#1499 | 保留期要进入公开政策 |

**不做的后果**：

- **机制上**：TC-065 的第 3 步和 TC-136 的对账无法通过，M-SEC-03 这个硬门槛判 FAIL，按发布规则即 NO-GO。
- **实际危害**：从删除前的备份恢复后，已删除账户的数据和**授权**一起回来，包括分享链接、scoped key、已撤销的 OAuth 授权。用户申请了删除、收到了确认，数据却又可访问。
- **无法察觉**：这是最糟的一点。删除的证据本身就在被回退的那个存储里，没有存储外的记录可供比对，所以不仅无法阻止，连“这次恢复复活了哪些已删除账户”都查不出来。恢复出来的行和正常行没有区别。
- **影响范围**：备份时间点到恢复时间点之间的全部删除。Mongo 备份保留 30 天，最坏情况一次恢复复活 30 天的删除量。
- **绕不开**：TC-065 明确要求演练“从删除前备份恢复”这个场景。

### 5.5 恢复：带恢复钉的 revive，建在新 PVC 上

> **本节是概要。恢复部分以 [R1-DESIGN.md](R1-DESIGN.md) 与 [R2-DESIGN.md](R2-DESIGN.md) 为准，二者已批准用于开发；本节与之不一致时，以设计文档为准。** 开发顺序上：R2 定义准入（第 1、2 步），R1 定义 runtime 侧准备（第 3 步的 init 步骤）与 HomeRestore 结算（第 6 步）。

1. **Admission**（admin 命令，带 Idempotency-Key）：**按 R2-DESIGN §3、§4.1**。可准入的状态为 `Paused`、任何类别的 `Failed`、卡住的 `Reviving`；证据为一次 PVC 读取证明原 UID 已不存在，或运维声明并绑定当前 UID；`Active` 需先暂停。
   - 事务内同时提交：备份 catalog 的 `restore_pins += 1`（条件 `state == ready`）、备份调度置 `restoring` 并自增 fence、Sandbox 的恢复钉与回执。
   - **不能用 `begin_revive`**：它只接受 `Paused`（`layer2-core/src/sandbox/sandbox.rs:251-259`）。准入写法照搬 same-image retained recovery：直接 `start_revive`、置 `Reviving`、清 `failure`；从卡住的 `Reviving` 准入时先取消当前操作再 `start_revive`（R2-DESIGN §4.1）。
2. **CheckpointVerification**：**按 R2-DESIGN §4.2**。
   - 备份为 `ready` 且已被钉住；再读一次原 home 观测，证据不得变化；
   - 旧 Sandbox CR 若存在，置为 Suspended 并证明其名下没有 Pod。**不删除**：原 PVC 由它持有，所以原 home 不会被删（满足 QA 任务中的“删 home 即 FAIL”）；
   - **不能直接复用 `verify_retained_home_and_absence`**：它会校验原 home 仍然存在，并且对没有 `suspended_runtime` 的 native home 直接判为终态失败（`cma/src/sandbox/worker/lifecycle/home_recovery.rs:44-84`）。需要恢复专用的校验分支：只证明旧 Pod 不存在和 quiescence，不要求原 PVC。
3. **RuntimeProvisioning**：从快照建**新的 PVC**，不涉及原 PVC。恢复 template 的 init container 先执行 R1-DESIGN §3 的 `--prepare-restored-home`，再执行 `initialize-home`。现有 revive 在这一步对 retained home 调用 `resume_runtime`，也就是在同一个 CRD 上恢复（`cma/src/sandbox/worker/lifecycle/restore.rs:113-138`）。恢复需要新增一条 adapter 路径：
   - 创建恢复专用的 template，VCT 的 dataSource 指向被钉住的备份快照，再用新 generation 的 claim 分配。legacy seeded home 已在使用同样的 VCT 形状（`pool/model.rs:203-209`）。
   - 这个 template 是恢复后 home 所依赖的定义，按 CLAUDE.md 对 pool 定义的规则保留，不能在恢复完成后直接删除。
   - `initialize-home` init container 不能重置已有内容的 home：需要实测，或在恢复 template 中去掉它。
   - 新 PVC 必须通过 `validate_restored_volume`：dataSource 正好是被钉住的备份，其他条件与 `validate_native_volume` 相同。
4. **身份重绑**：fenced 写入新的 `pvc_uid`、`home_identity`、`source_generation`。旧身份写入 `superseded_homes[]`，校验器永不重新挂载它。它不算孤儿（有登记），但处置有期限：恢复验证通过后 ≤7d，经授权走正常退役流程（M-SUST-02：“孤儿确认<=24h、授权处置<=7d”）。处置不在恢复路径中。
5. **CredentialDelivery**：先撤销旧 runtime 的凭据（复用 retirement 的 `RevokeRetirementCredentials` 语义），再签发新的 scoped key。凭据只在 supervisor 内存中，不在 home 里（R1-DESIGN §4.1），所以快照不会带回旧 key（实现要点：“凭证保管处理”）。
6. **HomeRestore（结算）**：**按 R1-DESIGN §4**。读取 runtime 返回的清点结果，一次 fenced 提交：
   - 沿用历史 checkpoint 恢复的结算语义：进行中的 turn 中断、不确定的操作以 `runtime_lost` 退休且不重发、已尝试过的排队消息不重放、未覆盖的 agent 转 `Stopped(RuntimeLost)`、清空 work receipts 与 cleanup debt；
   - 三条新规则：以清点结果判定 agent 是否已覆盖、home 身份重绑（见第 4 步）、t_s 之后被删除的子资源重新删除。
7. **ConversationReopen / QueuedMessageDelivery**：Mongo 中晚于切点的事件保留可见（CLAUDE.md §5）；从未尝试过的 `Queued` 消息照常投递。**逐 agent 隔离**：某个 agent reopen 失败时转 `Stopped(RuntimeLost)`，其余继续（R1-DESIGN §4.2）。
8. **PublishActive**：只要求成功 reopen 的 agent 处于 Ready；写 `SandboxEventKind::HomeRestored` 与恢复回执（R1-DESIGN §4.3）；`restore_pins -= 1`，清除恢复钉，备份调度回到 `active` 并立即排一次捕获。

### 5.5.1 home 落后于 Mongo：需要单独设计的部分

**设计见 [R1-DESIGN.md](R1-DESIGN.md)**（已批准用于开发）。要点：

- **问题已证实**：runtime 的检查只防 generation 太高，不防状态太旧；恢复出的旧卷会静默通过，并在启动时重做 t_s 时处于 Started 的操作（R1-DESIGN §1）。
- **做法**：恢复专用 template 的 init container 在 runtime 启动前，把 runtime 内部状态移入隔离目录、只保留已完成的安装记录，并清点 home 中实际存在的 agent 会话、workspace、worktree 和附件（§3）。
- **控制面结算**（在 revive 的 HomeRestore 阶段）：沿用历史 checkpoint 恢复的结算语义；只新增三条规则——以清点结果判定 agent 是否已覆盖、home 身份重绑、被删除的子资源重新删除；单个 agent reopen 失败不拖垮整个 revive（§4）。
- **持久记录**：一条 Sandbox 事件加恢复回执（§4.3）。

本节原先的六行方向表已由 R1-DESIGN §4.1 的规则表取代。

### 5.6 访问策略（实现要点 3 的“加密/访问策略”）

- 读取或恢复快照的权限只授予 CMA 恢复身份和有审计的 break-glass 运维身份。
- 恢复只能通过 admin 恢复路由，经过 tombstone 校验，并写审计事件。
- 隔离恢复环境（DR、演练）有单独的 project 和身份，生产凭据不能进入隔离环境。

**触发方式与 owner 可见性（D10 基线）**：首版只有 admin 命令可以触发恢复，**不做自动恢复**。home 只是暂时不可读或变慢时，自动恢复会用新盘覆盖本来健康的 home，而这个判断控制面做不可靠。owner 在 Sandbox 详情看到 typed 状态（存储故障、运维处理中、原数据未删除），不提供 owner 恢复按钮，UI 也不显示预计完成时间（CLAUDE.md §5：不得由耗时推断进度）。CLAUDE.md §5 中“owner 不需要点击 revive 来修复一次部署”针对部署，不针对存储丢失。如果以后要做自助恢复，前提是先有一个可靠的“home 确实不可用”判定。

### 5.7 可读副本校验（D7 默认口径）

- **每次捕获**：K8s 侧的 `readyToUse` 和 handle（§5.2 第 4 步）。
- **100% home，每日及取证时**：§9.4 的清单脚本直接查云端快照，逐 home 核对副本存在、状态、创建和完成时间、存储位置，并与 catalog 比对。这是“目录登记不能替代可读副本”的证据来源。
- **抽样恢复**：10 个 home（TC-087），加 nightly 的 fixture 演练（§9.2）。
- 不做“每个生产 home 定期恢复”。门槛负责人要求时（D7），要先补上隔离环境中客户数据副本的访问控制和删除回执。

---

## 6. 一致性设计（11-10 交付物）

| 数据类 | 独立副本 | 切点或版本 | 恢复依赖 |
|---|---|---|---|
| home | §5 快照，跨区域，Retain | `cut.provider_creation_time` | 快照 handle、CMEK key 版本 |
| Mongo（含 CMAEG 同库 collection） | Atlas PITR（#1565） | PITR 点 T，距故障 ≤5min | Atlas 恢复权限 |
| 文档对象（GCS） | 跨区域复制或版本化（#1569） | object generation | GCS 加密 key |
| 密钥 | Secret Manager 多区域复制，旧版本只 disable 不 destroy | catalog 中的 `custody` 版本 ID | key custody 流程（§7.2） |
| 删除记录 | Mongo tombstone 加 GCS retention-lock 副本 | erased_at | §5.4 |

**数据类 owner 与期限**（#1473 11-19 任务要求覆盖 Mongo/PVC/GCS/logs/backups；期限需与 #1551 批准的政策一致）：

| 数据类 | owner | live 期限 | 备份期限 | 本方案交付 |
|---|---|---|---|---|
| home PVC | Agent Manager | 账户删除后 ≤7d（#1500） | 快照 ≤30d；账户删除时立即删除 | §5.3、§5.4 |
| Mongo | Agent Manager / CMAEG | ≤7d（#1500） | **Atlas 快照和 PITR 保留期 ≤30d**，由 infra 配置并留证（#1565） | 本表与证据模板 |
| GCS 文档对象 | Agent Manager | ≤7d（#1500） | 版本或 soft-delete 窗口 ≤30d（#1569） | 本表 |
| 集中日志 | infra（#1550） | 按 #1550 核实的策略 | 不适用 | 只登记 owner 和期限，不改 |
| 备份 catalog 与重放 tombstone | Agent Manager | catalog 随备份删除 | tombstone 只存 hash，保留期取所有备份保留期的最大值，到期后删除，不能无限期保留 | §5.4.1 |

**integrated cut**：对每个 home 取 ≤ T 的最新 ready 快照 t_s，`integrated_rpo = t_loss − t_s`，捕获 30min 加完成 30min，所以 ≤ 1h。切点之后 Mongo 中的事件和 receipt 按 §5.5 第 6 步处理。起点事实：独立 home 副本 0，S3 捕获已停（`CLAUDE.md:517`）；**任何旧 archive/checkpoint 记录都不计入备份**。

---

## 7. 权限、密钥、网络隔离、DR 脚本

### 7.1 权限最小化矩阵（11-13 交付物，远端由 #1568 授予）

| 主体 | 资源 | 动词 | 用途 |
|---|---|---|---|
| CMA（远端，namespaced） | `volumesnapshots` | create、get、list、delete | 捕获、观测、到期 |
| CMA（远端，cluster） | `volumesnapshotcontents` | get、patch、delete | Retain 类下到期删除。infra 也可改用以快照名前缀为 IAM 条件的 provider 删除权限，二选一 |
| CMA（远端，namespaced） | `sandboxtemplates`、`sandboxclaims` | 现有授权已够（create 已在预检表中） | 恢复走专用 template，PVC 由控制器创建；**不需要新的 PVC 权限** |
| 恢复环境身份 | 快照 handle | 只读使用 | 隔离恢复 |
| 清单脚本身份（infra） | 云端快照 | list、get，只读 | §9.4 的覆盖核对与孤儿发现；删除孤儿需另行审计授权 |
| break-glass 运维 | 同上 | 审计 | 人工 DR |

`docs/CMA_Deployment_Readiness.md:478-486` 的权限表需同步更新，并保留“这不是请求扩大 infra Role”的口径：每项都标明用途。

### 7.2 密钥盘点与托管（11-10/11-19 交付物）

| 类别 | 密钥 | 丢失后果 | 托管要求 |
|---|---|---|---|
| 数据解密 | `CHRONO_AUTH_ENCRYPTION_KEY`（`infra/api/secret.yaml:30`，直接替换会导致旧记录不可读） | Mongo 备份中的 auth 材料不可读 | 独立备份；版本只 disable 不 destroy，保留至少覆盖备份保留期；恢复演练 |
| 数据解密 | `CMAEG_MASTER_KEYS`（`infra/cmaeg/secret.yaml`，keyring，`docs/CMAEG_Operations.md:935-942`） | CMAEG sealed records 不可读 | 同上 |
| 数据解密 | home 盘和快照的 CMEK、GCS 加密 key（infra） | 快照或对象不可用 | key 版本在恢复区域可用；在相关快照全部到期前不得销毁 |
| 访问凭据（可重新签发） | NyxID client secret、MongoDB URI、计费、storage、CMAEG service token 等 | 恢复环境无法启动 | 恢复环境可获得即可；轮换流程归 #1550 |

catalog 和证据中只出现版本 ID，不出现任何值；gitleaks 规则不变。

### 7.3 网络隔离校验（11-13 交付物，与 #1549 协同）

- 在远端 runtime namespace 和隔离恢复环境中，从 runtime Pod 实测访问元数据服务、集群内部服务、其他 Sandbox 和控制面私有端口，结果必须为拒绝。只看仓库 manifest 不算证据。
- 恢复出来的 home 含有用户任意代码，**必须先确认隔离生效，再在恢复环境启动 runtime**。DR 脚本把这一步作为前置检查。

### 7.4 DR 脚本（PR-1 起草，PR-4 可执行，写入 `docs/CMA_Operations.md`）

1. 冻结旧 writer：停止新准入（复用 #1555 的开关），记录故障起点。
2. 恢复网络、IAM 和 Secret，执行 §7.3 前置检查。
3. Atlas PITR 恢复到 T，记录 Mongo 恢复耗时。
4. 重放 tombstone（§5.4.1）：先恢复访问撤销，再对外服务；标记删除、拒绝恢复、重新删除被恢复出的资源。
5. 按 handle 预置快照内容。
6. 启动控制面和 runtime。
7. 逐个 home 执行 §5.5，并行度有上限，记录每个 home 的耗时。
8. 校验 hash、owner、native id、receipt、cursor，记录 full DR RTO。

三种灾难范围（Tokyo 控制面、Singapore 卷或区域、Atlas 不可用）分别写出依赖项和 NO-GO 条件（TC-088 第 1 步）。全量演练由 M-OPS-06 执行。

---

## 8. 并发问题与处理

### A. 滚动重叠下的多进程竞争（`maxSurge 1`）

- **后果**：重复捕获、重复删除，过期 holder 覆盖结果。
- **处理**：
  - schedule lease 加单调 fence，holder 为 `"{deployment_id}:{uuid}"`；
  - 所有写入带 `{_id, lease.token, lease.expires_at > now}` 过滤，`matched_count != 1` 视为冲突退出；
  - 外部调用的安全性靠三条，缺一不可：效果幂等（确定性名字，重复 create 得到 AlreadyExists 后校验并认领）；单步超时短于 lease 剩余时间；调用之后的提交带 fence。调用前重读 fence 只能缩小窗口，不能当作安全依据；
  - 销毁类效果（删除快照）先用 CAS 把状态持久化为 `expiring`，再执行删除，重复删除无害；
  - 非首次 claim 只认领不重建。
- **复用**：`cma/src/store/agent_lifecycle_writes.rs:6-47`、`agent_lifecycle_operations.rs:251-286`、`layer2-core/src/sandbox/provision.rs:81-145`、`cma/src/composition.rs:245`、`checkpoint_maintenance/export.rs:247-317`（先持久化 claim，执行一步，**调用之后**重读并带 fence 提交；只有首次 claim 才创建）。`cma/src/warm_pools.rs:400-432` 用的是“单步超时 30s 短于 lease 60s”，原理同样成立，但没有 fence 计数，只适合幂等的 configure 和 resize。

### B. 备份写入干扰用户操作

- Sandbox 文档每写一次 revision 就 +1，排队中的用户控制会被拒为 `TargetChanged`（`layer2-core/src/sandbox/control/transition.rs:42-49`）。
- **处理**：备份状态全部放在独立 collection，worker 对 Sandbox 只读；只有恢复写 Sandbox 文档。

### C. 与生命周期的竞态

| 竞态 | 处理 | 复用 |
|---|---|---|
| 捕获中遇到 pause/resume/runtime update/节点 drain（TC-083） | 已挂载和未挂载的盘都能拍快照；create 前后复核 UID，变化即 `failed{source_changed}` 并重排 | `provider/home_snapshots.rs` |
| 捕获中遇到 delete | native home 的 PVC 在 `TeardownRuntime` 就开始删除，所以在它之前把 schedule 改为 `retiring` 并自增 fence，再等 `in_flight` 结束或取消（§5.4）。捕获的 fenced 写入带 `state: active` 条件 | 等待的写法参考 `retirement.rs:230-238`，但位置要前移 |
| 捕获中遇到恢复 | 恢复 admission `$inc` fence，在途捕获失效 | A 段 |
| 恢复与用户操作并发 | 恢复就是 revive，继承单操作槽、`pause_eligibility`，以及每个 Sandbox 只能有一个排队控制的唯一索引 | `store/sandbox_controls/records.rs:58-63` |
| 捕获中遇到账户删除 | `erasing` 先 fence 并取消，再写 tombstone，最后删除备份 | PR #1827 的 erasure lease |

### D. 备份记录自身的竞态

- **到期与恢复**：在 `state` 和 `restore_pins` 上做 CAS，两者不会同时成功。
- **同一 home 并发捕获**：`in_flight` 单槽，加 `{sandbox_id, epoch}` 唯一索引。
- **删除与恢复**：tombstone 先于删除写入；恢复 admission 在同一事务内校验 tombstone。
- **保留分级**：是 `ready_at` 的纯函数，与处理顺序无关。

### E. 吞吐瓶颈

| 瓶颈 | 处理 |
|---|---|
| 共享后台预算不适合备份 worker。它只有 2 个槽（`cma/src/background.rs:12`）；只要有前台活动，最多放行 1 条后台链（`background.rs:43-55`）；每个交互式 HTTP 请求、Sandbox 和 Workspace sweep 的整个执行期都算前台（`sandbox/scheduler.rs:46,54`）；拿不到槽是跳过，不是排队。现有使用者是 reconciliation、agent scheduler、provenance backfill | **不接入共享预算。** 仿照 warm pool 的独立受监督循环：固定 tick、每 tick 处理条数有上限、同时只有一个子任务（`cma/src/warm_pools/supervision.rs:19-58`）。每一步仍是单次调用加持久化。否则在 API 峰值负载下，备份会和 reconciliation 抢同一条链，正好在 TC-087 要求带负载测量时错过 30 分钟 |
| 快照路径没有 K8s QPS 限流 | Layer 3 快照客户端加独立信号量，每 tick 有上限（`CHRONO_HOME_BACKUP_MAX_IN_FLIGHT`） |
| GCE 配额（每个 home 约 170–190 个保留快照） | 由 #1568 确认，纳入 §9.4 成本测量 |
| 惊群 | `next_due_at` 按 `hash(sandbox_id) mod 30min` 错开；按 `{state, next_due_at, _id}` 索引取 limit（同 `store/sandbox_worker_store.rs:26-65`） |
| Mongo 连接池 10（`cma/src/store/mongo.rs:83-110`） | 写入很少，复用主 client 并串行处理；在 §9.4 的峰值负载测量中验证，确有争用时仿照 `store/usage_reports/mod.rs:143-149` 另提 PR |
| 全量 DR 时盘的 hydration | 用等价 used bytes 实测，不外推 |
| **全量 DR 的恢复吞吐**：revive 由 Sandbox worker 执行，每进程只有 `SANDBOX_CONCURRENT_SWEEPS = 4`（`cma/src/sandbox/scheduler.rs:13`），而 provisioning 和 suspend 在进程内等待 120–180s（`layer3-adapter/src/agent_sandbox/claim_guard.rs:160`、`suspension.rs:192`、`pool/claims.rs:13`）。几百个 home 按 4 路串行，可能超出 4h | 每个 sweep 对自己取到的一批是串行处理的（`cma/src/sandbox/worker.rs:181-200`），所以每进程实际并发就是 4。把 sweep 并发数做成配置项，默认仍为 4，只在 DR 恢复环境调大；不新增恢复 lane。sweep 执行期间算前台，会压制共享后台预算，这也是备份 worker 不接入共享预算的原因之一。先在 TC-088 演练中实测，再决定是否需要更多改动 |
| 重复 create 与单盘快照频率 | 确定性名字遇到 AlreadyExists 时，按标签和 UID 校验后认领；失败重试的退避不低于 provider 的单盘快照最小间隔，避免触发限流 |
| K8s 快照对象数量随保留期线性增长（D9）：每个 home 近 200 个 VolumeSnapshot 加同样多的 VolumeSnapshotContent | 所有查询按名字 GET 或带标签分页，不做全量 list；对象总量由 infra 确认上限。超出时改为只保留 handle，按需重新导入 |

### F. 恢复时封旧 writer

- 自增 `runtime_generation_high_water`，拒绝旧 `boot_id`（`layer2-core/src/sandbox/runtime_generation.rs:97-121`）。
- revive 的 authority generation 让旧操作失效。
- 旧 Sandbox CR 保持 Suspended；RWOP 保证只有一个挂载者。
- `superseded_homes` 防止旧盘被重新挂载。
- 先撤销旧凭据，再签发新凭据。

### G. 多个 PR 并行开发

- 依赖关系（**2026-11-16 更新：R1、R2 已批准用于开发**）：PR-1、PR-2、PR-3 的 `retention.rs`、PR-4r 现在都能开工；PR-4 基于 PR-1、PR-2 分支叠加开发，合并要等两者合并，开启要等 PR-2 已部署一个版本、PR-4r 镜像发布、D3；PR-3 的 worker 基于 PR-1；PR-4b 除恢复演练外现在都能开工，恢复演练依赖 PR-4；PR-5 中与 D8 结果无关的部分现在能开工（基于 PR-3 分支），tombstone 机制建议等 D8；PR-6 依赖证据。
- **PR-2 的形状已定**，所以可以与 PR-1 并行。它要尽早合并部署，因为 PR-4 的开启必须比它晚一个已部署的版本，这是恢复部分的关键路径。
- **PR-4r 的第一个提交是 V1、V3 的复现测试。** V1 是 R1 的基础，不通过就停下回改 R1，后面的 PR-4r 与 PR-4 都不再继续。
- 冲突热点：`infra/api/configmap.yaml`、`cma/src/config/features.rs`、`cma/src/worker_supervision.rs`、`docs/CMA_Operations.md` 配置表、isolation 目录。
- 约定：每个 PR 只加自己的条目，合并前 rebase，并重跑 `check-config-keys.sh` 和 `check-config-docs.mjs`。

---

## 9. 测试、演练与证据

### 9.1 自动化测试（成功和失败路径）

| 编号 | 用例 | 层级 |
|---|---|---|
| C1–C6 | 捕获：正常 ready；UID 变化即拒绝；非首次 claim 不重建；30min 超时 typed 失败；双 holder 只有一个成功；备份运行时排队 pause 不收到 TargetChanged | Layer 3 fixture、store ignored 套件、worker |
| E1–E4 | 到期：hourly/daily 跨 SGT 日界；30d 上限不可配置放宽；`restore_pins > 0` 不删除；provider 确认不存在后才标记 deleted | Layer 2 纯函数、store |
| RS1–RS6 | 恢复端到端：正常恢复后 hash/owner/native id/receipt 匹配；dataSource 不匹配被拒；旧 PVC 未被删且不被重新挂载；切点后的 turn 不重发；旧凭据已撤销；与用户 pause 互斥。**另见 R1-DESIGN §8（R1-1~R1-8）与 R2-DESIGN §8（R2-1~R2-8）** | Layer 3、worker、harness |
| T1–T5 | tombstone：恢复入口拒绝已删除 subject；Mongo 回到删除之前后，重放能枚举并重新撤销访问、重新删除资源；重放幂等；写副本失败时删除事务不提交；到期后 tombstone 被删除；账户删除后该 owner 的备份 0 残留 | store、harness |
| K1–K3 | 密钥：合成加密记录（auth、CMAEG sealed）在隔离环境用新旧 key 恢复后可读；错误 key 被拒绝；日志和 artifact 的 sentinel 扫描 0 secret | harness |

### 9.2 nightly 受保护恢复演练

- 在 `.github/workflows/nightly-staging.yml` 新增 job，沿用现有的 `plan` 审核 SHA 绑定、受保护 environment 和“独立观测部署身份”检查。
- 内容：在 staging 写入已知 hash 的 dirty home，等待捕获，让原 PVC 不可读，再按 §5.5 恢复并校验。
- 上传脱敏证据：digest、SHA、环境、时间窗、N、开关、各项耗时，供 #1545 的发布门禁使用。
- 只计不重试的首轮结果（#1473 术语：“nightly / scheduled”）。

### 9.3 证据封存（11-19 QA 任务）

- 第 10 次 scheduled 运行结果、10 个 home 恢复（≥3 类：active、paused、长历史，覆盖 warm/cold/resumed/retained 版本）、首次 DR 原始结果。
- 必须检查 dirty file、cursor、撤权和外部副作用收据。
- 任何需要人工 DB 修复、删除 home 或盲目重发的运行都记为 FAIL。

### 9.4 覆盖报告与开销

- **覆盖报告与清单脚本**（只读，TC-087 第 1 步；对应 Issue 列出的工具“Storage backup inventory”）：以 `sandboxes` 中的 retained home 为分母，join `home_backups`（最近 ready、间隔）和**云端快照清单**（用 infra 身份直接查询，不经过 CMA），再 join Atlas PITR 窗口、密钥版本可用性、GCS 复制状态。输出 0 缺失、0 过期、0 不可读的逐项清单，并列出 catalog 中没有的云端快照（孤儿）。CMA 同时导出覆盖率和最老 ready 时长的 metrics 用于告警。
- **开销**：在 API 峰值负载下测量捕获和到期对 Mongo、K8s API、AM CPU 的影响，以及快照存储量和成本，并上报 M-SCALE-04/M-SUST-02。按 TC-087 第 2 步，分别测量初始全量、稳态增量、峰值变化三种情况下的副本完成时间。

---

## 10. 最小性说明

### 10.1 相对完整版删掉的部分（不影响 §1 中的任何要求）

| 删掉的内容 | 为什么可以删 |
|---|---|
| 新的恢复生命周期操作类型 | 复用 revive 的操作类型、阶段和 authority。但校验、provisioning、Failed 准入、落后对账仍需新代码（§5.5、§5.5.1），恢复部分的工作量不小 |
| admin inventory REST | admin 路由会连带两个 SDK 和契约测试；覆盖报告加 metrics 已满足 TC-087 |
| 跳过“暂停且未变化”的 home | 不是 Issue 要求，还需要额外决策 |
| 独立 Mongo 连接池 | 没有测量证据前不做 |
| 运营可调的备份参数（`platform.rs`） | 部署配置已足够 |
| catalog 中复制 native id | 恢复时直接比对 home 内容与 Mongo，可省掉每次捕获的 Agent 查询 |
| 每个生产 home 定期恢复校验 | Issue 要求的是 100% 清单核对加 10 个抽样恢复；定期恢复还会产生客户数据的额外副本 |
| 独立恢复 lane | 改为一个 sweep 并发配置项，只在 DR 环境调大 |
| CMA 内的云端快照对账 | CMA 没有云 API 客户端，改由清单脚本完成 |
| home 数据类自己的 tombstone | D5 采用建议默认时，删除后没有 home 副本可复活。但 Mongo 恢复的重放 tombstone 仍然必须做，见 §5.4.1 |
| owner 端 UI 或对外“已有备份”的宣传 | Issue 不要求，且证据之前禁止宣称 |
| legacy 路径（条件删除） | 生产中 retained 的 legacy home 为 0 时不做 |

### 10.2 之前被误删、现在已恢复的部分（Issue 明确要求）

| 恢复的内容 | #1473 依据 |
|---|---|
| 恢复不删除、不重挂原 home（新 PVC 路线） | QA 任务：“删home…均FAIL” |
| 备份与删除记录的关联、恢复时重放 tombstone（§5.4.1） | Background：“关联…删除记录”；11-19 任务：“恢复重放tombstone” |
| nightly 受保护恢复演练 | 实现要点 8；QA 任务“第10次scheduled” |
| 100% home 的云端清单核对 | TC-087：“目录登记不能替代可读副本” |
| 风险登记、一致性设计、密钥盘点、权限矩阵、网络隔离校验、DR 脚本 | 11-07、11-10、11-13 任务 |
| 恢复时的凭据撤销与重新签发 | 实现要点 4：“凭证保管处理” |
| 开销测量 | M-OPS-01 测量方法：“备份开销纳入峰值负载与成本” |
| 孤儿快照对账、Atlas 备份保留期 ≤30d、tombstone 有期限且只存 hash | M-SEC-03：backup ≤30d、恢复不复活、删除后 0 残留 |
| 数据类 owner 与期限表（含 logs） | 11-19 任务：“覆盖Mongo/PVC/GCS/logs/backups owner与期限” |
| Sandbox sweep 并发可配置 | M-OPS-01/TC-088：全量 DR ≤4h，不能由 10 个 home 外推 |
| schedule 补建扫描 | M-OPS-01：100% retained homes 覆盖，漏建不能不可见 |
| Affected Files 逐项处理 | 仓库约束：“在首个 PR 中列全” |

### 10.3 为什么不能再小

剩下的每一项都对应 §1 中的一条要求。唯一的更小替代是由 infra 用 GCE 快照计划代替 CMA 捕获。据了解其最小粒度为 1 小时（需 infra 确认），达不到 D1 未变更时的 30min 要求；而且 catalog、fence、tombstone、恢复都仍需 CMA 实现。只有负责人在 D1 选 B 档时才值得评估。

---

## 11. PR 拆分

### 11.1 拆分的必要性（结构性约束，不按时间）

1. **严格 reader 决定发布顺序。** `SandboxLifecycleState`/`SandboxLifecycleOperation` 是 `deny_unknown_fields`（`layer2-core/src/sandbox/lifecycle.rs:109,168`）。恢复钉和 `superseded_homes` 写进 Sandbox 文档后，旧二进制会读失败。恢复 reader 必须比 writer **至少早一个已部署的发布**，回滚才安全（`docs/CMA_Data_Migrations.md`、CLAUDE.md §6）。只让旧版本“能解析字段”还不够：它还必须能继续暂停和恢复已经恢复过的 home，并且遇到进行中的恢复时停住，而不是按普通 revive 去挂原 home。所以 PR-2 的内容比“只加字段”多。
2. **外部阻塞点不同。** 捕获依赖 D2 和 Retain 类；恢复依赖 D3；删除对接依赖 #1827。合在一起会被最慢的未知项卡住。
3. **风险与回滚单元不同。** 捕获只新增快照；到期是不可逆销毁，需要单独评审（`CMA_Data_Migrations.md`「Backup, dry-run and irreversible steps」）；恢复会改生命周期和身份。三者各有独立开关。
4. **证据有依赖。** 恢复和 nightly 演练都需要捕获已经在运行。
5. **能力声明在证据之后。** 修改 CLAUDE.md §5 必须单独提 PR，放在最后（CLAUDE.md §9）。注意 DR 脚本和过时文档的修正**不是**能力声明，随 PR-1 交付。
6. **评审人不同。** infra、#1500 负责人、公开接口负责人、QA。
7. **不按层拆。** 没有使用者的中间层不能端到端测试，`cma` 中未使用的代码在 `clippy -D warnings` 下会报 dead_code。所以每个 PR 都是可部署、可测试、可单独关闭的纵切片。

### 11.2 PR 列表

**归属（D6 基线）**：infra 负责快照类、远端 RBAC、配额、区域与 CMEK、清单脚本身份（#1568 的交付物）；后端负责 PR-1~PR-5 的代码。PR-1 中的文档部分和 PR-6 由本工作项负责人写。

**判定点（D12 基线）**：11-18 23:59 SGT 复核 D3、V1、V-R2-1。任一缺失，则 11-19 由产品加 M-OPS-01 门槛负责人决定是否启动 #1473 写明的替代路径（明确缩小公开的持久性承诺），PR-4 不在 11-19 交付。捕获与到期（PR-1、PR-3）不受这个判定影响，照常推进。

**2026-11-16 更新**：R1、R2 已批准用于开发；原 PR-0 的文档并入 PR-1。共 **8 个 PR**（D8 若答“#1500 自己实现 tombstone 机制”，PR-5 并入 PR-3，变为 7 个）。

| PR | 内容 | 开始开发 | 合并 | 生产启用 / 开关 |
|---|---|---|---|---|
| **PR-1 捕获与文档** | **代码**：两个新 collection、严格 reader、索引；Layer 2 模型；Layer 3 快照 create/observe；schedule 补建扫描；独立受监督的 worker 循环；metrics；退役流程在 `TeardownRuntime` 前 fence；configmap；C 系列测试。**文档（原 PR-0）**：新增 `docs/CMA_Home_Backup.md`（契约、切点定义、保留政策、一致性设计、密钥盘点、权限矩阵、DR 脚本草案、**本 PR 的 data migration record**，状态显式标为未交付，写法参照 `docs/CMA_Retirement_Archives.md` 的“Production remains HOLD”）；本方案与 R1/R2 设计（`docs/plans/home-backup/`，含风险登记与 Affected Files 全量清单）；修正 `docs/CMA_Model.md:325-349` 与 `docs/Sandbox_Provider.md:92-105`；`docs/Release.md` 只加一处交叉引用 | ✅ 现在 | CI 通过；文档另请 infra、安全评审 | `CHRONO_HOME_BACKUP_ENABLED`。需 D2 远端授权、Retain 快照类、D9 上限确认，且 **PR-3 已部署**（没有到期时每个 home 每天新增 48 个快照） |
| **PR-2 恢复兼容下限** | 形状由 R1-DESIGN §9 与 R2-DESIGN §3-§4 确定，只有三项：Sandbox lifecycle 的 `home_restore`（恢复钉与回执，回执含 R2 的证据分类、观测到的 UID、可选的运维声明、幂等 key 摘要）、`superseded_homes`、`SandboxEventKind::HomeRestored`；加上 `validate_restored_volume`；遇到带恢复钉的进行中操作时停住并报 typed 等待；兼容测试。**不含准入路径** | ✅ 现在，与 PR-1 并行 | CI 通过 | 无开关，合并部署即可。**它必须比 PR-4 的开启早一个已部署的版本**；部署后不可回滚到它之前的版本 |
| **PR-3 保留与到期** | retention 纯函数、到期 worker、孤儿快照对账、30d 上限、删除回执；E 系列测试，以及“PITR 回退后孤儿快照被补登或删除”的测试 | ✅ `retention.rs` 现在；worker 基于 PR-1 分支叠加 | PR-1 已合并 | `CHRONO_HOME_BACKUP_EXPIRY_ENABLED`；必须先于 PR-1 的生产开启部署 |
| **PR-4r runtime 恢复准备** | R1-DESIGN §3：`--prepare-restored-home` init 模式、内部状态隔离、安装记录校验、清点与 sha256、`home_restore_v1` 能力声明；不修改任何用户文件；R1-1~R1-3 测试 | ✅ 现在。**第一个提交就是 V1、V3 的复现测试**：runtime 在“有内容的 home + 空 state root + 保留的安装记录”下能否正常启动、ledger 屏障在哪里。不通过就停下，回改 R1 | CI 通过；不依赖其他 PR | 无开关：runtime 镜像按自己的节奏发布与提升（`CHRONO_SANDBOX_IMAGE`），能力必须在固定镜像上演示后控制面才会使用 |
| **PR-4 恢复** | R2 的原 home 观测（Layer 3）、准入函数与事务（`Paused`、`Failed`、卡住的 `Reviving`）、revive 恢复钉、恢复专用校验分支与两次复核、恢复 template 的 adapter 路径、身份重绑、凭据撤销与签发、R1-DESIGN §4 的结算（接到现有非 retained 分支，加三条新规则）与逐 agent 隔离、sweep 并发配置、admin 恢复路由及 SDK/契约；R2-1~R2-8、R1-4~R1-8 测试 | ✅ 现在，基于 PR-1、PR-2 分支叠加。**准入范围在 V-R2-1 实测后定稿** | PR-1、PR-2 已合并 | `CHRONO_HOME_BACKUP_RESTORE_ENABLED`。需 PR-2 已部署一个版本、PR-4r 镜像已发布且能力已演示、D3 远端验证通过；已准入的恢复必须能走完 |
| **PR-4b 取证工具** | DR harness、云端清单脚本与覆盖报告、证据模板、K 系列测试、nightly 恢复演练 job | ✅ **大部分现在**：K1–K3（合成加密记录在新旧 key 下的恢复，与 home 恢复无关）、清单脚本与覆盖报告（按 §3 的数据形状，用 fixture 开发）、证据模板、harness 骨架。⏳ **只有恢复演练步骤与 nightly 恢复 job** 要等 PR-4 能端到端跑通 | 清单部分在 PR-1 合并后（数据形状以合并版为准）；恢复演练部分在 PR-4 合并后 | 不改服务行为；workflow 变更单独评审；清单脚本需 infra 的只读身份 |
| **PR-5 删除关联** | `erase_home_backups`（Layer 2 服务加删除回执）；两个新 collection 登记进 #1500 的 erasure catalog；恢复入口通过 `DeletionReplayPort` 拒绝已删除的 owner；tombstone 机制：格式、有期限的 create-only 写入器、枚举、重放工具（§5.4.1）；DR 重放步骤；T 系列测试 | ✅ **与 D8 结果无关的部分现在**：`erase_home_backups`（基于 PR-3 分支叠加）、catalog 登记、恢复入口的 port 与拒绝。⏳ **tombstone 机制本身**建议等 D8（11-17）：若 #1500 自己实现，这部分整块作废，而答复只差一天 | PR-3 已合并；不必等 #1827 | 跟随 #1500 的开关 |
| **PR-6 能力声明** | CLAUDE.md §5、对外文档、DR runbook 定稿 | ❌ 有 TC-087 证据后。写的是“已经被证明的能力”，内容取决于实测结果（RTO 数值、自定义镜像是否在范围内等），提前写既不能合并，也大概率要重写（CLAUDE.md §9） | 证据齐备 | 文档 |

- 新增配置键（同步到 `features.rs`、`infra/api/configmap.yaml`、`docs/CMA_Operations.md`，开关默认关闭）：
  - `CHRONO_HOME_BACKUP_ENABLED`
  - `CHRONO_HOME_BACKUP_EXPIRY_ENABLED`
  - `CHRONO_HOME_BACKUP_RESTORE_ENABLED`
  - `CHRONO_HOME_BACKUP_SNAPSHOT_CLASS`
  - `CHRONO_HOME_BACKUP_INTERVAL_SECONDS`（上限 1800）
  - `CHRONO_HOME_BACKUP_MAX_IN_FLIGHT`
  - `CHRONO_SANDBOX_CONCURRENT_SWEEPS`
  - 按需：`CHRONO_HOME_BACKUP_LEGACY_SNAPSHOT_CLASS`、tombstone 副本的 bucket 名（若为新 secret，只在 `secret.yaml` 中引用）
- 最后一个合入的 PR 写 `Closes #1473`，其余写 `Refs #1473`。全部 CI 门禁必须通过，小步提交，不加 `Co-Authored-By`，不提交任何密钥。

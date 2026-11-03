# 01：用任务名称与真实状态浏览 Agent

集成修复交接：见 [INTEGRATION-REPAIRS](../INTEGRATION-REPAIRS.md)。Mongo 查询探针
独立复现 10,000 个工具更新后扫描 10,001 个文档才能取回一条相关事件。非 Ready
状态现在直接使用既有 domain projection；Ready 的完成、排队及损坏证据处理保留。
既有 Mongo 共享 inventory 测试经真实 red → green 验证公开 hierarchy/inventory
状态和 Ready 损坏拒绝；探针只证明查询规模，不宣称生产延迟。

**What to build:** 用户在默认侧栏中找到任务、看懂是否正在工作或需要处理，并能从设置路径进入原有 Sandbox 管理。

**Blocked by:** None（评审确认后可独立开始）。

**Status:** done (2026-11-03; all six acceptance criteria verified at the approved U/A seams)

**Implementation:** `codex/cma-ui-01`, isolated `ticket-01` worktree. Approved U/A
seams; each behavior follows red → green. First U red observed 2026-11-03:
`TaskNavigation.test.tsx` could not find visible “Ready to start”; both ready
Agents previously exposed only the hidden lifecycle word “Ready”.

**优先级:** P0

## Acceptance criteria

- [x] 默认导航以 Agent 任务为主，移除常规 Agents/Sandboxes 切换；新增设置中的环境管理入口，原有详情、深链接和上下文创建仍可访问。
- [x] 使用现有标题优先级，空标题显示人类可读的“新任务/未命名任务”；内部 ID 留在详情，不作为正常任务名。
- [x] 工作中、等你审批、准备中、暂停、停止、失败分别可辨；只有最后一轮明确成功且无新工作才显示“完成”，从未执行但 ready 的 Agent 显示“可开始”。
- [x] 对非当前 Agent，列表需要的最后一轮结果通过已授权的列表投影提供；不逐条启动对话读取，不用本地猜测补齐状态。
- [x] 保留 Personal/组织/共享范围、状态过滤、选中项、键盘导航与返回位置；父 Sandbox 不可用时，操作资格仍受原生命周期约束。
- [x] 状态同时有文字与现有状态色；窄屏、长标题、空列表和读取失败可操作，失败不把已显示任务清空。

## Testing boundaries（2026-11-03 已确认）

- **U：** 任务列表→选择任务→设置中的 Sandbox→返回；验证名称、状态、过滤、可访问名称和焦点。不能证明服务端状态新鲜度。
- **A（若扩展列表投影）：** 不同 owner 的列表读取及最后一轮结果；验证权限和真实终态。不能证明页面视觉。
- **首个 red → green：** 一个从未运行的 ready Agent 和一个已成功完成的 ready Agent 同时出现时，页面分别显示“可开始”和“完成”。

## 范围与依赖说明

复用当前任务标题，自动命名补齐由 05 负责，两者没有先后依赖。视觉规范和新增导航规则随本票交付；不移除底层 Sandbox API 或管理能力。

## Delivery

- `cdf64ec4d`: authorized task projection, bounded task filters, safe list titles,
  Mongo/in-memory snapshot adapters, both SDKs, API tests and affected contract fields.
- `8dbf1004b`: task-first navigation, Settings → Manage environments → Back to tasks,
  visible states, title fallback, stale-read recovery, U tests and Web Style rules.
- `f3f8a825a`: distinguish setup completion from task completion by recorded identity.
- Integration `09b8174d3` is included through merge `c59ca7489` (also includes ticket 09).
  Both Web Style additions and their append-only changelog entries were retained.
  The final integration update changes only baseline evidence documentation;
  validated task source remains `f3f8a825a`.
- Original source and the integration worktree were not edited. No deployment,
  remote PR, production mutation, homepage/create implementation, composer/header,
  answer-footer implementation or environment-panel implementation was changed by 01.
  Ticket 09 implementation enters this branch only through the integration merge.

The task result is a read-only projection over durable turn boundaries, existing
first-message identity, active/queued work and admitted command records. It adds
no stored schema, migration, runtime dispatch or per-row conversation endpoint.
Owner/share checks and disclosure fences still guard the returned list. Missing
evidence stays unknown; lifecycle readiness alone never proves completion.

### Setup versus task identity

The first-message record's `turn_sequence` must match the completed turn:

| Recorded evidence | Navigation result |
| --- | --- |
| Creation with no task turn | Ready to start |
| Completed first message with `purpose=setup` | Ready to start |
| Completed first message with `purpose=task` | Completed |
| Completed legacy first message with absent purpose | Status unavailable |
| Later successful task turn after setup | Completed |
| New active, queued or admitted work | Working, or the stronger lifecycle state |

Failed/interrupted turns and parent lifecycle barriers retain their precedence.
The setup test uses identical message text across purpose cases; no text/title
heuristic is involved. Running setup retains the existing lifecycle indication.
The setup-only API test first failed with `completed` instead of `not_started`,
then passed with the other five task API tests.

## Acceptance evidence

| Criterion | Verified behavior and evidence |
| --- | --- |
| Default task navigation and environment path | `shell/TaskNavigation.test.tsx`, `ConsoleCommands.test.tsx` and Chrome: no ordinary view switch; actual Sandbox link opens its detail route; Back returns to the Agent route and preserves the draft; scoped New agent still carries the Sandbox ID. |
| Human titles | Owner title beats provider/configured titles; empty/ID-derived titles become Untitled task; long titles retain full hover text and truncate without hiding status. Shared API projection keeps the same title priority without exposing runtime metadata. |
| Real task states | Six named `task_navigation_*` API tests cover unstarted/successful ready tasks, failed shared turns, newly admitted work, owner isolation, setup identity and parent pause. U tests check all visible state words and existing tones. |
| Authorized list projection | Workspace summaries and scoped provenance inventory expose `task_status`; shared/organization filters use that field. Browser request audit and shared U tests open no noncurrent Agent conversation/transcript/events endpoints. |
| Scope, selection, keyboard and return | Personal/organization/shared tests, per-view filters, keyboard activation/menu actions, environment disclosure, selected route, focus and saved scroll pass. A shared task remains mounted while browsing another environment scope. |
| Small screens, empty/error states | Chrome at 1440×1000 and 390×844: no horizontal overflow; Settings target 32px/44px; all eight status words visible; empty navigation remains actionable; refresh failure retains rows, removes stale dots and recovers on retry. Shared stale rows cannot navigate; confirmed access revocation removes them. |

## Validation (actual runs, 2026-11-03)

Evidence directory: `/tmp/cma-ticket-01-evidence`. Source for the final backend
checks is `f3f8a825a`, containing integration `6a606ebf0`. Later ticket updates are
documentation only. Rust 1.96.1 uses the real, non-symlink private targets
`/Users/liuyitian/code/cma-ui-worktrees/ticket-01/api/target` and
`/Users/liuyitian/code/cma-ui-worktrees/ticket-01/sdk/rust/target`, with
`RUSTFLAGS='--cfg cma_ui_ticket01'`. After merging, only this worktree's affected
workspace crate artifacts were cleaned before rebuilding. Earlier unflagged
artifact results are superseded. Nonzero named counts and source paths are in
the final logs; the zero-test doc-test phase is not counted as API evidence.

| Command / scope | Actual result | Log |
| --- | --- | --- |
| Web `npm test -- --run src/shell src/resources src/Console src/api/rest` | 52 files, 380 tests passed after integration merge | `web-merged-final.log` |
| Web `npm run typecheck` | Passed | `web-merged-typecheck.log` |
| ESLint on all 18 changed Web TS/TSX files; Prettier/rustfmt comparison on all 45 changed source/test files | Passed; baseline-only error-response formatting excluded | `final-source-checks.log` |
| TypeScript SDK `npm test`, `npm run typecheck`, changed-file ESLint | 67 files, 3558 tests passed; typecheck/lint passed | `typescript-sdk-final.log`, `typescript-sdk-typecheck-final.log`, `typescript-sdk-lint.log` |
| Rust SDK `cargo test --locked` with the ticket flag | 231 tests including doctests passed; 2 explicit external-fixture tests ignored | `rust-sdk-private-final.log` |
| Public API `cargo test --locked -p layer1-public task_navigation -- --nocapture` with the ticket flag and `RUST_MIN_STACK=16777216` | Six named task tests passed, including setup-purpose identity | `setup-purpose-green.log` |
| Public API `cargo test --locked -p layer1-public -- --skip shared_public_contract_matches_real_backend` with the same flag/stack | 407 tests passed; exactly one aggregate fixture test excluded for the independently reproduced baseline mismatch below | `public-private-final.log` |
| Selected real-handler public-contract replay with `CMA_PUBLIC_CONTRACT_FILE` and the same flag/stack | Seven nonzero test runs; 211 cases passed: channel usage 35, creation inventory 25, shared creation inventory 18, instances 27, shared hierarchy 70, instances storage 18, integrated message feedback 18 | `contracts-private-final.log` |
| `cargo clippy --locked -p layer1-public -p cma -- -D warnings` with the ticket flag | Passed, including the production Mongo adapter and service compilation | `rust-clippy-private-final.log` |
| Chrome local fixture browser proof | Desktop/mobile routes, draft return, status text, long titles, empty/error/retry and no noncurrent conversation reads all passed | `browser.mjs`, `browser-results.json`, `browser.log` |

Red → green evidence also includes `scope-return-red.log`/`scope-return-green.log`
(lost shared focus after changing environment scope), `parent-state-red.log`/
`parent-state-green.log` (failed task under a paused parent), `stale-dot-red.log`,
`sdk-filter-red.log`/`sdk-filter-green.log`, and `setup-purpose-red.log`/
`setup-purpose-green.log`. Each implementation followed its failing behavior test.

Visual evidence is outside the repository: `desktop-before.png`,
`mobile-before.png`, `desktop-after.png`, `mobile-after.png`, plus desktop/mobile
`settings`, `empty`, and `refresh-failure` PNGs in the evidence directory. Before
images use archived `c9e2d7b91`; after images use the merged UI. They prove fixture
presentation, not a live backend. `changed-paths.txt` records the full ticket diff
against integration, excluding its independently authored ticket 09 changes.

## Baseline prerequisites and limitations

- Parent independently reproduced `E0308` at `instance_problem.rs:207` with
  `cargo check --locked -p layer1-public`, exit 101: the detail tuple arm returned
  `String` while the other arms returned `&str`. Exact parent evidence:
  `/Users/liuyitian/code/cma-ui-worktrees/baseline-public-build.log`.
  The two-line typing-only correction is isolated in `4d1b27e7d`; parent landed
  the identical canonical correction as `60fc98ee5`. Both histories are now
  merged, with no duplicate source hunk and no typing fix in the task commits.
- The full aggregate public-contract test still has an unrelated preserved
  mismatch: `agent-skills-install-version-conflict` expects
  `agent_operation_in_flight`, while the handler returns
  `agent_skill_version_upgrade_unsupported`. It was reproduced from archived
  integration `60fc98ee5` in `/tmp/cma-ticket-01-api-baseline`, with a separate
  APFS-cloned private target, affected workspace crates cleaned, the ticket
  RUSTFLAGS and an actual one-test failure. See `baseline-skill-private.log`.
  Its handler/fixture were not modified. No all-public-contract green claim is made.
- API acceptance uses real authorized routers/services and in-memory durable
  log/command stores at the approved A seam. The Mongo implementation compiles
  and passes Clippy; no live Mongo deployment or performance benchmark was run.
  Rust SDK's two ignored tests require an isolated CMAEG HTTP fixture and the
  disposable official Responses SDK server. No new runtime behavior is claimed.
- No ticket-01 acceptance gaps remain. The aggregate baseline fixture mismatch
  and unrun live-environment checks above remain explicit validation limitations.

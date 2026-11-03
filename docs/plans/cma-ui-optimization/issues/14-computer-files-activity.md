# 14：常驻 Agent 电脑面板展示文件与操作输出

**What to build:** 用户在对话旁持续查看 Agent 的文件和真实命令活动，切换面板时不丢失会话或未保存内容。

**Blocked by:** None（复用现有文件侧栏和工具事件）。

**Status:** done

Implementation uses the approved U/A/R boundaries. The remaining scope question
is resolved by the repository's actual layout: authorized Sandbox workspace and
worktree files are intentionally shared, while supervisor control/Agent homes,
runtime state and foreign Agent uploads are private. Public files now enforce
that distinction without withdrawing shared workspace editing or downloads.
See [file-scope evidence](../14-FILE-SCOPE.md) for source contracts, enforcement
and red/green proofs. An isolated pinned native command, candidate durable fold,
public projection and Chrome reload also have staged evidence below; this does
not claim a deployed Linux control-plane journey or OS process isolation.

**优先级:** P3

## Acceptance criteria

- [x] 支持的宽屏布局默认显示右侧“Agent 的电脑”，用户可收起；窄屏用明确入口与可返回面板，保留对话阅读宽度、composer 和键盘焦点。
- [x] 文件 tab 继续使用当前授权文件树、读取、下载及 owner 编辑能力；切走/关闭时尊重未保存文件草稿，不能以重挂载丢弃修改。
- [x] 终端活动 tab 只显示该 Agent/turn 已记录的命令及输出，按顺序提供运行、失败、取消、截断和未报告状态；不支持命令输入，不请求已退役的 PTY。
- [x] 环境能力及拒绝原因由公开的可信状态提供，定义 browser/files/operation-output/replay/control 的支持状态与作用范围；加载中、unsupported、未启用、权限不足和暂时故障可区分。
- [x] 没有浏览器能力时显示说明，不放假浏览器图和 Live 标记；15 接入后使用同一 panel scope。shared view 只提供获授权的数据和动作。
- [x] 面板、历史和消息流共享当前事件观察结果，避免为每个 tab 再建立重复整段订阅；隐藏页面停止无必要轮询，重新打开按游标恢复。
- [x] Agent/账号/权限改变立即隔离旧数据与晚到回调；不会展示同 Sandbox 其他 Agent 的私有文件、状态或输出。按 2026-11-03 用户澄清，已授权共享的 Workspace/worktree 文件保留原语义，不误判为其他 Agent 的私有输出。
- [x] 详情、文件预览与电脑面板保持单一可理解的选中状态；其更新不重挂载会话、不清空文本/附件/滚动位置。

## Testing boundaries（2026-11-03 已确认）

- **U/A：** 面板切换→真实文件读取/活动流→权限变化，验证文件修改保护和 Agent 隔离；输出必须来自可回读的实际事件。fixture 无法证明 runtime 报告完整性。
- **R：** 固定 runtime 执行已知无害命令，页面读到对应输出；明确缺失时展示未报告。该证据只证明日志观察，不证明 PTY 能力。
- **真实浏览器：** 默认宽屏、收起恢复、手机返回、焦点和 composer DOM 连续性。
- **首个 red → green：** 有未发送草稿时在文件与终端活动间切换，看到对应 turn 的已知输出，原草稿与阅读位置不变。

## 范围与依赖说明

不以本票替代真实浏览器、控制权或回放；15/17/18 继续交付。08 已存在时复用其选中文件状态，14 不硬依赖成果卡片即可独立验收。

## Implementation and handoff (2026-11-03)

- `AgentPanel` and `AgentSidePanel.onSelect` are the existing single selection
  interface: `details | files | activity | browser | undefined`. 07/08 can open
  the retained files panel through this selection without rebuilding conversation
  ownership or introducing a new Chat/Session. File-path selection remains the
  existing explorer's responsibility; this ticket does not implement 08.
- The file explorer is keyed by Agent and shared/owner context, not mutable write
  capability. Revoking write while retaining read preserves the same dirty editor
  as read-only. Changing tabs or closing the panel keeps its draft mounted.
- `OperationOutput` takes stable `toolCalls`, `activeRunId`, and `active` props.
  It filters only when tool data or visibility changes, groups one visible page
  in one pass, renders at most 50 operations, and renders no hidden history.
  Shared readers publish the already authorized conversation observation into
  `AgentComputerObservation`, scoped by client, subject and Agent; no extra event
  stream is opened. Ticket09 `finalAnswer` and run-scoped recovery, and existing
  plan turn scope, are preserved.
- Shared output also requires the panel's current successful `view_history`
  observation. Expiry withdraws existing output while renewal is pending;
  timeout/failed renewal keeps it hidden and offers a scoped retry. Explicit
  denial and a refreshed environment `operation_output: forbidden` hide already
  recorded output as well as late updates. Valid background leases retain output
  until their deadline. This gate does not reset a still-readable file draft.
- `GET /api/v1/agents/{id}/environment` is the public capability seam for 15/18:
  Agent identity, policy revision, opaque runtime incarnation, revalidation
  deadline, and feature `{state, reason, scope, writable}` for files, operation
  output, browser, replay and control. It is an observation, never a retained
  permission grant. Browser/replay/control currently remain explicitly
  unsupported. Future implementations must provide their own authorized data
  contract before changing that state; 17 owns control.
- Files explicitly retain `mounted_home` (`/home`), while output uses
  `agent_turn_history`. No provider address or boot identifier is exposed. Do
  not treat the mounted home as proof of an Agent-private filesystem. Public
  file routes exclude `.cma`, `cma/.cma`, foreign `attachments/<agent>` namespaces
  and destructive ancestors. The runtime also protects configured state and
  checkpoint roots inside the mounted home. Internal managed instruction
  operations remain separate and tested; shared workspace files remain usable.
- ACP text content and the bridge's reported `rawOutput.formatted_output` pass
  through the actual core recorder. Each durable output snapshot is bounded to
  16 KiB of valid UTF-8, with explicit truncation. `ToolOutputReported` preserves
  content-only updates without inventing status. Metadata-only deltas retain
  pending-message/first-message/operation evidence without flooding the log.
- Agent v2 snapshots/tails negotiate `Prefer: cma-tool-output`; legacy event
  reads use `tool_output=true`. Older requests retain their cursors while new
  output is hidden. AGUI adds `cma:output` and a statusless
  `cma.tool_output_reported` CUSTOM event. Responses v1 retains reported output;
  Responses v2 adds `cma:output` to informational activity while keeping the
  standard `output` null and existing lifecycle/event numbering. In-progress
  output-only v2 reads use retrieval or Agent/AGUI events. These are replacement
  snapshots, not stdout deltas, terminal input, exit codes or replay frames.
- Old stored logs remain readable. New durable output records require the new
  reader: disabling the UI does not make an older strict core reader compatible.
  Any rollout/rollback must retain a compatible reader or restore pre-upgrade
  data. No destructive log migration or automatic downgrade was added.

## Validation and limits

The implementation followed the approved U/A/R seams. Recorded red/green cases
include output lost at the SDK boundary, content-only ACP output, native formatted
output, write-capability draft loss, missing mobile entry, 10,000 rendered output
records, unrecorded output creating a phantom operation, and technical capability
errors leaking into the panel. The private Rust run also caught a regression in
metadata-only update evidence; restoring the existing flush/evidence path made
all nine named tool-update tests pass. The existing toolbar geometry assertion
caught the added scope row overflowing the file panel; the flexible explorer
layout passes at both desktop and mobile sizes.

| Boundary/check | Actual result |
| --- | --- |
| Private core `tool_update_tests` | 9 passed, including bounded content-only output, actual provider formatted output, 4,096 quiet metadata deltas, first-message/operation evidence and generation fences |
| Private core `sandbox::tests::filesystem` | 4 passed, including capability scope, owner/cross-owner, shared write/read and revocation |
| Public `reported_output_crosses_durable_snapshot_tail_and_response_boundaries_without_status_invention` | 1 passed; real recorder, negotiated/legacy snapshots, tail, Responses and outsider denial |
| Public `recorded_tool_output_is_readable_only_in_the_authorized_agent_stream` | 1 passed; reported output plus statusless output-only AGUI, exact Agent/turn and outsider denial |
| Public `computer_environment_public_route_preserves_file_scope_and_denies_foreign_readers` | 1 passed; owner/viewer scope and write capability through the real router, foreign 404 |
| Native capture → core → public AGUI/Responses | 1 explicitly enabled ignored test passed with the actual Linux capture |
| Rust SDK event/files/Responses suites | 25 passed, including typed recorded output and all activity replay cursors |
| TypeScript SDK | 67 files / 3,560 tests passed at merge `b4e445225`; SDK typecheck also passed after `9a900786a` |
| Focused web before 03/06 merge | 12 files / 116 tests passed; lifecycle, files/presence, attachments, shared permissions/drafts, final-answer identity, plans and conversation/message performance |
| Intermediate web after 03/06 merge | 126 passed, 3 disclosure assertions failed; ticket02 later independently identified four more ActiveThinking failures. All seven are repaired in this branch by expanding real disclosures, with ordering/pending/replay checks retained |
| Final merged web | 10 files / 84 tests passed, including all three disclosure suites, scoped permissions/drafts, files/presence, lifecycle, plan scope and long-conversation performance. After the final type-safe selector correction, all three disclosure files again passed 48/48 with no exclusions |
| Private-file public routes | 2 named tests passed: protected Agent homes/control roots and Agent-scoped uploads, owner/viewer/writer matrix, shared workspace edits/downloads, same-prefix namespace denial and recursive ancestor protection |
| Existing file REST corpus | `agent-filesystem.json` replay passed through `shared_public_contract_matches_real_backend`; no fixture recording/update mode used |
| Runtime file boundary | 11 named tests passed: real runtime HTTP private state/alias checks plus existing versioned writes, hidden/binary files, size bounds, symlink races, shared credential protection and lifecycle recovery |
| Managed settings compatibility | 4 actual `hosted::managed_context::writer_tests` passed: exact home writes/receipts, concurrent edits, uncertain acknowledgments and stale-boot fencing |
| Final scoped U checks | 4 passed: expired/pending/failed renewal and recovery with existing output; revoked history plus late callback; write-revocation draft preservation; unknown-call output does not invent a status. New expiry test first failed with `scoped-proof` still in the DOM, then passed after the authority gate |
| Chrome computer | 6 passed; native recorded output and reload, capability reasons, statusless/truncated/missing output, 10,000 turns, mobile focus, desktop drafts/scroll |
| Chrome existing toolbar | 5 passed; 1600px/390px × light/dark, retained files, resize bounds, rename and system prompt continuity |
| Scoped Chrome refresh | 1 passed: output visible before environment renewal, absent after `operation_output: forbidden`, browser denial remains distinct; no writes |
| Types/build | Final merged Web/e2e TypeScript checks and production build passed; private `cargo check --locked -p layer1-public -p layer4-runtime` passed. SDK TypeScript had already passed; the scope fix adds no SDK field or wire revision |
| Formatting/lint | Changed UI/e2e Prettier and ESLint passed; changed Rust files passed rustfmt with `skip_children=true`, avoiding unrelated baseline modules; `git diff --check` and Gitleaks passed |

Final functional merge `b31a7c146` includes `7739c431f` (02, 03, 06, 09 and 11).
The style append conflict retains both tickets' rules. Final merged evidence is
in `final-merged-web.log`, `completed-disclosures.log`, `final-merged-chrome.log`,
`final-merged-rust.log`, `final-merged-build.log`, `final-scope-runtime.log` and
`completed-gitleaks.log` under `ticket14-proof`. Chrome passed all six computer
and five toolbar cases after this merge, including the current scope label and
retained mobile editor actions; desktop/mobile screenshots were inspected.
The earlier named durable-output public test actually ran one test, using the
private target; its evidence remains in `public-output-merged.log`.

Rust uses pinned 1.96.1, `CARGO_BUILD_JOBS=2`, and the private APFS-cloned target
`/Users/liuyitian/code/cma-ui-worktrees/ticket14-proof/cargo-target`. Workspace
packages were privately cleaned before proof builds; `cma-sdk` was separately
cleaned there. Initial filename-based filters matched zero tests and are **not**
accepted evidence; the corrected module filters above ran the named tests.
The shared target and other workers' builds were not cleaned or stopped. The
initial file-corpus replay exceeded the default test-thread stack; rerunning
with `RUST_MIN_STACK=16777216` passed. An initial `managed_context_tests` filename
filter matched zero tests and is excluded; the actual module filter above ran
four named tests. Filtered zero-test integration executables are not counted.

The focused web commands use `--configLoader=runner --no-cache --maxWorkers=2`.
Final scope evidence is in `ticket14-proof/scope-red.log`, `scope-green.log`, and
`scope-final.log`; the last command ran four named tests and intentionally
excluded 23 unrelated SharedAgent tests. The full post-merge result is separately
reported above, including its intermediate failures and their completed repair.
Early Vite/Vitest runs used generated dependency caches through the prepared
`web/node_modules` link into the original checkout. Final checks use a private
APFS dependency clone at `ticket14-proof/web-deps-TnKt07/node_modules`; original
source files and the integration worktree were not edited. No cleanup of another
worker's shared cache was attempted.

### Isolated native proof

- Explicit Docker context: `colima-cma-ui-proof`; `--network none`, disposable
  container, immutable committed source archive `44bcb1be4` mounted read-only.
- Pinned image:
  `sha256:22071f29fff5db494c8e5633a74692ebae442d7c2c7a0856e2d15ff7f9802057`.
  It supplies Codex 0.147.0 and the exact patched ACP bridge described in
  `/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/README.md`.
- `recorded_output.test.cjs`: 1 native Linux test passed, executing the harmless
  `printf 'native-computer-proof\n'` command through the real CLI/ACP process.
  The model HTTP boundary is controlled; no production credentials or network
  services are used.
- Capture: `ticket14-proof/linux-native-output.json`, SHA-256
  `d0272e31472350303d2c8a691d511e8ea2ac26597a96448fad32033767e4ec04`.
- Candidate recorder/public projection artifact:
  `ticket14-proof/public-native-output.sse`, SHA-256
  `e5725c97f52e0a02be1ef5ca3cb8d6bbe53a48be71be6bf6a729ee2400d4153a`.
  The public test uses the real durable recorder and router with the existing
  in-memory store fixture, then Chrome consumes those exact response bytes and
  verifies the output again after reload. Supporting account/inventory HTTP
  responses remain controlled fixtures.
- Screenshots are retained under
  `/Users/liuyitian/code/cma-ui-worktrees/ticket14-proof/screenshots/`:
  desktop, mobile, native output, 10,000 turns, and four toolbar theme/size views.

This proves actual native output survives the candidate recording/projection and
page boundaries. It does not claim a deployed candidate Linux HTTP service,
database-process restart, real-model quality, browser observation, replay or
interactive PTY. The public file scope now separates intentionally shared
Sandbox workspace facts from actual private Agent/runtime namespaces, based on
the source contracts and the user's explicit clarification. It does not claim
that arbitrary shared work-product files or processes are OS-isolated per Agent.

All source changes stay on `codex/cma-ui-14`; integration is only merged into this
worktree. No production deployment, original/integration source write, push or
remote PR is part of this work. Baseline aggregate Rust format/skill-corpus gaps
and large web chunk notices remain documented in the corrected plan baseline;
focused checks do not claim full CI, five-image builds or coverage of those gaps.

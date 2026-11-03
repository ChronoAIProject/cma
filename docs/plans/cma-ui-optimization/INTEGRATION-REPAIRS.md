# Bounded integration repairs — tickets 01 / 03 / 14

This is the authorized repair batch within the existing 19-ticket workflow, not
an additional ticket or the final standards/spec review. Work is isolated to
`codex/cma-ui-integration-repairs`, initially based on `d19c225bd`; the original
workspace and the integration worktree remain untouched by this worker.

## Scope and agreed seams

- Preserve the computer drawer and embedded file layout while giving each CSS
  class one stylesheet owner; no collision allowlist or policy changes.
- Extract cohesive filesystem access guards and live reasoning folding to keep
  production files below 500 lines without changing their behavior.
- Complete the environment operation's shared JSON inventory, real public
  handler corpus and typed SDK dispatch. The REST Markdown inventory is already
  fixed separately by `54a7414b6`.
- Repair stale Home/navigation and event URL assertions through the existing
  rendered App and public client seams, retaining all no-creation/no-launch
  authority assertions.
- Avoid task-evidence queries for a known non-Ready Agent status. Validate the
  public inventory/status behavior, Ready completion/queued/corruption handling,
  and the separately measured Mongo query scaling; do not claim production
  latency from a synthetic query probe.

## Initial reproduced failures and repairs

Evidence logs are outside the worktree at `/tmp/cma-integration-repairs/`.

- `web-red.log`: 3 files, 7 failed / 55 passed. Six AppPublic assertions assumed
  the old Home title and no session storage; task identity now records only the
  viewer marker. The copy check also exposed the visible label `Retained task`
  and mistakenly scanned a multi-class CSS attribute as visible text.
- `rest-web-red.log`: 1 failed / 22 passed. The event read correctly requests
  `tool_output=true`; the old expected URL omitted it. These are all eight
  integration failures identified by ticket 07, with its separate upstream
  reproduction recorded in that ticket's handoff.
- `home-green.log`: 3 files / 62 tests passed. The App renders the actual task
  composer for an owner, preserves the visitor landing and checks the exact
  viewer marker, empty task, enabled explicit action, zero resource mutations,
  zero launches and no unexpected requests across remounts. Visible copy uses
  `Submitted task`; the copy scanner still checks visible attributes and its
  example now verifies that multiple CSS class names are not user-facing copy.
- `css-red.log` reproduces the `.agent-files-panel` collision. The embedded
  modifier now belongs to `AgentFiles.css`, retaining the same flex, height and
  min-height declarations; `css-green.log` passes with the existing three
  deliberate shared classes. No design rule or Web Style entry is added.
- `structural-web.log`: 4 files / 41 tests passed, including the public event
  URL, conversation folding/performance and retained computer panel behavior.
  Live reasoning and keyed upsert are extracted without changing their bodies.
  Filesystem path and mutation-authority guards move together into a private
  scope module; protected homes, foreign uploads and runtime filters remain.
- `inventory-red.log`: the exact named shared JSON inventory test ran **1 test**
  and failed with 228 actual / 227 registered operations before the later
  ticket merges. This is distinct from the already-fixed REST Markdown row.

## Isolation and completion status

Web uses the existing linked dependencies with `--configLoader=runner
--no-cache --maxWorkers=2`. Rust uses jobs=2 and the private, non-symlink
`/Users/liuyitian/code/cma-ui-worktrees/cargo-target-repairs`, APFS-cloned from
the integration cache, then cleaned for cma/layer1-public/layer2-core/layer3-adapter
before recompilation. The retained cache flag is `--cfg cma_ui_ticket04`; target
isolation, clean output and compilation paths establish this worktree's binary.
Zero-test runs never count as evidence.

## Environment shared contract repair

The exact operation is registered with its real authorization problems
`not_authenticated` and `resource_not_found`. `agent-environment.json` adds ten
actual handler cases: owner, read-only accepted link, writable accepted link,
runtime not ready, unsupported mounted home, paused runtime, foreign subject,
revoked link, anonymous request and missing Agent. Cases retain private/no-store
headers, exact Agent identity, policy revision, opaque runtime incarnation,
renewal deadline, file mutation authority and read-only output/browser/replay/
control facts. Only the live link renewal deadline is captured symbolically.

- `environment-record.log` records the real handlers; an independent non-recording
  replay in `environment-handler-green.log` passes **1 named test / 10 cases**.
- TypeScript dispatch initially fails all ten cases and the completeness test
  (`environment-sdk-ts-red.log`, 11 actual failures). The repaired typed method
  passes the entire corpus: **2,747 tests**, `sdk-ts-green.log`.
- Rust initially runs **1 failing test**, reporting the unmapped environment
  operation (`environment-sdk-rust-red.log`). The typed method and structural
  equality of the exported environment models give **1 passed / 10 typed HTTP
  cases**, then the complete corpus gives **1 passed / 2,302 typed HTTP, 173 typed
  preflight, 240 unrepresentable wire-shape and 27 browser-owned cases**
  (`environment-sdk-rust-green.log`, `sdk-rust-all.log`). SDK TypeScript typecheck
  passes. Neither SDK bypasses the typed method with a raw HTTP call.
- The merged **global** inventory remains separately blocked by ticket 07's four
  unregistered delivery operations. Before this repair it actually ran **1 failed
  test**, with **234 actual / 229 listed** (`inventory-merged-red.log`). This repair
  adds the environment entry and raises the exact listed count to **230**; it does
  not count absent delivery entries as covered. Those routes and ticket 10's two
  feedback routes remain intact. This is a new integration gap, not an original
  skill-fixture failure and not a passing global gate.

## Merged Web result

`web-all.log` passes the complete merged suite: **329 files / 2,963 tests**, no
committed skips or test exclusions. Web typecheck, eslint and the unchanged CSS
collision gate pass (`web-typecheck.log`, `web-lint.log`, `css-final.log`). jsdom
prints its existing cross-Document navigation notices without failing tests.

The source-size gate no longer reports either repaired production source:
`agent_filesystem.rs` is 473 lines and `conversation.ts` is 469. It reports the
unchanged original 508-line `agent_operation_service.rs` and the newly merged
ticket-10 `agents_tests/message_feedback.rs` at 556 lines. The latter file is
outside this batch's owned source and is a separate new integration handoff.

Environment corpus, Mongo fast path, latest integration merge and final affected
checks remain in progress. Original skill fixtures, LocalAgentV2 head-404,
formatter drift and unchanged 508-line `agent_operation_service.rs` are outside
this batch. No pushes, PRs, deployments or shared fixture shutdowns are authorized.

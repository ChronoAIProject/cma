# Ticket 02 — composer settings and confirmed completion

Verified 2026-11-03 in `/Users/liuyitian/code/cma-ui-worktrees/ticket-02`, branch
`codex/cma-ui-02`. Initial integration base: `d656e542e`. Functional merge:
`a19e88697`, including integration `111530241e88806140685382b53de2134b8a7b79`
(01, 03, 06, 09 and 11). Logs, native/public JSON and screenshots are outside the
repository at `/tmp/cma-ticket-02-evidence/`; generated artifacts are not committed.

## Acceptance evidence

| Criterion | Implementation and observed result |
| --- | --- |
| Compact settings | One collapsed disclosure summarizes catalog model, reasoning and permission. Existing controls, capability checks, idle guard, explicit Apply and exact uncertain-request retry remain in SessionSettings. Unapplied values and recovery notices survive collapse. |
| Truthful queue/interrupt | Responses Queue says “Process after the current turn finishes.” Interrupt says “Stop the current turn, then start a new turn with this message.” AG-UI retains idle Send/active Stop. No protocol values, permission levels or transport selection changed. |
| Sending, stop, approval, recovery | Full AgentSurface/controller tests retain Responses after applying settings. Real public command/control/queue/interruption handlers and native cancellation prove applicable behavior beyond UI mocks. Existing settings/approval/retry/Stop browser checks pass. |
| Successful elapsed | Recorded 10:00 → 10:04 displays “Completed in 4 minutes” in the header only after success. Existing turnOutcomes is authoritative; latestTurnId is shared with 11. AG-UI/Responses live updates, history reload, native completion versus interruption and public replay are covered. |
| Missing times/unsuccessful outcomes | Missing/invalid/reversed timestamps, unknown outcome, failure, interruption, approval pause, active next turn and stopped/failed/deleted Agent omit success. Subsecond completion says “Completed in less than 1 second”. No wall-clock or mount-time fallback. |
| Persistent accessible composer | First test types a multiline draft, uploads a file, opens/closes settings, retains the textarea node and sends exact text/attachment manifest. Chrome checks both transports at 1440px/light and 390px/dark, actual theme, keyboard Enter/Space, expanded/controlled region, status markup, 44px targets, long labels/drafts, visible Send and no horizontal overflow. Header updates retain draft, attachment and node. |

`AgentHeaderProgress` combines completion with 06's existing `progress` slot and
keeps its memoized current-turn PlanProgress link. It adds no activity summary.
AgentSurface is 480 lines; the cohesive helper is 24 lines. 01 domain status and
09 canonical final-answer marker are unchanged. The final merge reuses 11's
latest-turn identity. Web_Style was updated before UI code; integration merges
retain the integration document as an exact prefix and append 02's rules/changelog.

## Test-first record

The first behavior is `preserves the same draft and uploaded attachment through settings disclosure and sends them unchanged`.
`first-red.log` has one actual failure because the settings disclosure does not
exist; `first-green.log` has one actual pass after implementation. Subsequent
vertical slices have separate red/green logs: `queue-copy-*`, `completion-*`,
`live-completion-*`, `responses-completion-*`, and `lifecycle-completion-*`.
Tests use the approved complete-surface U and public/native A/R seams.

## Real U/A/R chain and limits

1. `composer_completion.test.cjs` starts the real pinned native app-server via
   the patched ACP bridge. It asserts `codex-cli 0.147.0` and bridge SHA-256
   `84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c`.
   A loopback model endpoint completes one turn and holds the next. Native
   `thread/read` proves `inProgress`; actual `session/cancel` yields `cancelled`
   and native `interrupted`. A replacement prompt yields a distinct third turn.
   Native statuses are exactly completed/interrupted/completed with three IDs.
   `native-completion.json` records the actual bridge replies.
2. `completion_outcomes_keep_recorded_time_and_success_distinct_on_both_public_transports`
   loads that artifact via `CMA_COMPLETION_NATIVE_PROOF`, validates pins/statuses,
   and feeds the actual replies to `DurableAgentUpdateRecorder`. Real public
   AG-UI replay/tail and AgentV2 snapshot routes preserve identity, successful
   versus cancelled terminal kind and recorded times. `public-completion.json`
   contains their literal output. The executed proof uses the native artifact;
   the test's routine deterministic fallback is not counted as native evidence.
3. `composer-completion.check.ts` consumes those public frames/snapshots through
   the actual browser SDK/controller, using loopback SSE and isolated Chrome
   profiles. It verifies active → confirmed complete, same composer, reload and
   interrupted history. Screenshots are in `browser-before/` and `browser-after/`.
   Final after checks explicitly set and verify light desktop/dark mobile. Before
   images used the default dark theme: layout evidence, not matched-theme comparison.

This is layered boundary evidence, not browser-to-deployed-runtime or production
authentication coverage. Model replies are controlled; public handler persistence,
authorization and clock use existing fixtures. Four minutes is a fixed recorded
time example, not measured native latency. Native completion/cancellation comes
from real processes. Public FIFO/interruption tests execute real admission and
dispatch logic; U mock call counts do not establish queue timing.

Linux runs the same new native test against the unchanged pinned provider in the
local `cma-ui-runtime-proof:baseline` image (ID
`sha256:22071f29fff5db494c8e5633a74692ebae442d7c2c7a0856e2d15ff7f9802057`).
02 changes no production Rust, bridge or layer4 binary. This is native-provider
evidence only, not a candidate binary, full production image or deployed Sandbox.

## Executed checks

Every Rust invocation sets `PATH=/Users/liuyitian/.cargo/bin:$PATH`,
`CARGO_BUILD_JOBS=2`, `CARGO_TARGET_DIR=/Users/liuyitian/code/cma-ui-worktrees/cargo-target`
and **`RUSTFLAGS='--cfg cma_ui_ticket02'`**. The final executable is
`layer1_public-f8a61e1cf038988e`; logs show actual nonzero counts. No unisolated
shared-cache result is used.

| Check | Actual result | Log |
| --- | --- | --- |
| Final focused Web after merging 11 | 11 files, 114 passed, 0 skipped | `final-focused.log` |
| Public AG-UI including native-derived completion, 06 plan, 11 send | 25 passed | `final-public-agui.log` |
| Public Responses interruption | 3 passed, including `interrupt_stops_original_then_starts_distinct_message_before_preserved_fifo` | `final-public-interruption.log` |
| Public Responses controls/approval/recovery | 8 passed | `final-public-controls.log` |
| Public queued edit/cancel/pagination | 6 passed | `final-public-queue.log` |
| Public conversation commands/admission/replay | 16 passed | `final-public-send.log` |
| New native completed/cancelled/replacement on macOS | 1 passed | `native-completion.log` |
| Existing native steering/cancellation/runtime observations | 4 passed | `native-controls.log` |
| New native test in isolated Linux image | 1 passed | `linux-native-completion.log` |
| Final Chrome AG-UI/Responses × desktop/mobile | 4 passed | `final-browser.log` |
| Final Chrome 06 public plan frames/reconnect/link | 2 passed | `final-browser-plan.log` |
| Existing composer/approval/uncertain-settings/Stop browser checks | 4 passed, after 06 merge | `browser-settings-focused.log` |
| Broad Web before 11, with identified integration failures excluded | 114 files, 926 passed, 7 skipped | `agent-regression-scoped.log` |
| Web/E2E typecheck, ESLint, CSS check | Passed; no collisions across 875 classes | `final-typecheck.log`, `final-e2e-typecheck.log`, `final-lint.log`, `final-css.log` |
| Production Web build | Passed; existing large-chunk warning | `final-build.log` |
| Changed Web Prettier; new Rust rustfmt; whitespace | Passed | `final-prettier.log`, `final-rustfmt.log`; `git diff --check` |
| Branch secret scan | No leaks; no allowlist added | `final-gitleaks.log` |

Commands below run from the worktree unless preceded by `cd web`:

```sh
npm --prefix web test -- --maxWorkers=2 --configLoader=runner --no-cache src/agent/AgentCompletion.test.tsx src/agent/ComposerSettings.surface.test.tsx src/agent/SessionSettings.test.tsx src/agent/AgentSurface.test.tsx src/agent/AgentSurface.suggestions.test.tsx src/agent/PlanProgress.test.tsx src/agent/AnswerFeedback.test.tsx src/agent/styles.test.ts src/Console.test.tsx src/shell/TaskNavigation.test.tsx src/resources/TaskNavigation.test.tsx

export PATH=/Users/liuyitian/.cargo/bin:$PATH
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR=/Users/liuyitian/code/cma-ui-worktrees/cargo-target
export RUSTFLAGS='--cfg cma_ui_ticket02'
CMA_COMPLETION_NATIVE_PROOF=/tmp/cma-ticket-02-evidence/native-completion.json CMA_COMPLETION_PUBLIC_PROOF=/tmp/cma-ticket-02-evidence/public-completion.json cargo test --manifest-path api/Cargo.toml --locked -p layer1-public --lib agui:: -- --nocapture
cargo test --manifest-path api/Cargo.toml --locked -p layer1-public --lib responses::v2::interruption:: -- --nocapture
cargo test --manifest-path api/Cargo.toml --locked -p layer1-public --lib responses::v2::controls:: -- --nocapture
cargo test --manifest-path api/Cargo.toml --locked -p layer1-public --lib responses::v2::queued_ -- --nocapture
cargo test --manifest-path api/Cargo.toml --locked -p layer1-public --lib conversation::commands_tests:: -- --nocapture

CMA_CODEX_TEST_BINARY=/Users/liuyitian/.npm/_npx/abfeab83ffa1581c/node_modules/@openai/codex/bin/codex.js CMA_ACP_TEST_BRIDGE=/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/bridge.mjs CMA_COMPLETION_NATIVE_OUTPUT=/tmp/cma-ticket-02-evidence/native-completion.json node --test api/crates/layer3-adapter/src/codex/composer_completion.test.cjs
docker --context colima-cma-ui-proof run --rm --name cma-ticket02-native-completion --entrypoint node -v /Users/liuyitian/code/cma-ui-worktrees/ticket-02/api/crates/layer3-adapter/src/codex/composer_completion.test.cjs:/proof/codex/composer_completion.test.cjs:ro cma-ui-runtime-proof:baseline --test /proof/codex/composer_completion.test.cjs

cd web
CMA_COMPLETION_PUBLIC_PROOF=/tmp/cma-ticket-02-evidence/public-completion.json CMA_COMPLETION_PREVIEW_URL=http://127.0.0.1:4602 CMA_E2E_BROWSER_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' ./node_modules/.bin/playwright test --config e2e/composer-completion.config.ts
CMA_PLAN_PUBLIC_PROOF=/Users/liuyitian/code/cma-ui-worktrees/ticket06-evidence/public-plan.json CMA_PLAN_PREVIEW_URL=http://127.0.0.1:4602 CMA_PLAN_EVIDENCE_DIR=/tmp/cma-ticket-02-evidence/plan-browser CMA_E2E_BROWSER_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' ./node_modules/.bin/playwright test --config e2e/plan-progress.config.ts
CMA_STARTUP_PREVIEW_URL=http://127.0.0.1:4602 CMA_E2E_BROWSER_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' ./node_modules/.bin/playwright test --config e2e/agent-settings.config.ts --grep 'composer|access approval'
```

Browser commands require a loopback candidate Vite preview. This run used
`/tmp/cma-ticket-02-evidence/serve.mjs` with `configLoader: runner` and private
per-port Vite caches to avoid writing through linked dependency caches.

## Independently reproduced integration failures

The broad run found an old composer CSS assertion, which 02 updated, and seven
unrelated assertions. The latter reproduce without 02 in an immutable
`git archive 9a900786a` checkout at `integration-baseline/`:

- `ActiveThinking.test.tsx`: four tests covering completion-ordered tool cards,
  pending tools after RUN_FINISHED/RUN_ERROR and terminal-tool replay.
- `MessageIdentity.test.tsx`: identity once per run across tool calls.
- `SharedAgent.test.tsx`: two tests retaining hydrated tools/controls during late
  history/access refresh.

Exact names and stacks: `integration-baseline-failures.log` (5 failed, 17 passed)
and `integration-baseline-shared-failures.log` (2 failed, 21 passed). The broader
926-pass run excludes exactly those seven names with an explicit negative `-t`
expression. No committed skips or weakened CI gates. Final focused 114/114 has
no exclusions. These are existing integration failures, not assertions that all
were present before the UI project.

Legacy broad browser suites also fail independently on integration: old creation
and signed-in landing expectations disagree with 03, and LocalAgentV2's missing
head route returns fatal 404. `browser-baseline-legacy.log` reproduces four
representative failures (new Sandbox creation, cold 390px idle, signed-in landing,
system text scaling). `browser-queue-controls.log` exposes the same head setup
problem. 02's new browser proof explicitly supplies the unavailable-head response
and real public events; it does not alter that legacy fixture. The four relevant
existing settings browser tests pass. No all-E2E-green claim is made.

Previously documented `agent-skills-install-version-conflict` corpus mismatch
and original formatting failures remain unchanged. No unrelated repair or
full-repository-green claim is included.

## Changed paths relative to integration

```text
api/crates/layer1-public/src/agui/completion_tests.rs
api/crates/layer1-public/src/agui/mod.rs
api/crates/layer3-adapter/src/codex/composer_completion.test.cjs
docs/Web_Style.md
docs/plans/cma-ui-optimization/02-EVIDENCE.md
docs/plans/cma-ui-optimization/IMPLEMENTATION.md
docs/plans/cma-ui-optimization/issues/02-composer-language.md
web/e2e/agent-settings.check.ts
web/e2e/composer-completion.check.ts
web/e2e/composer-completion.config.ts
web/e2e/shell-layout.check.ts
web/src/Console.test.tsx
web/src/agent/AgentCompletion.test.tsx
web/src/agent/AgentHeader.css
web/src/agent/AgentHeaderProgress.tsx
web/src/agent/AgentSurface.test.tsx
web/src/agent/AgentSurface.tsx
web/src/agent/Composer.css
web/src/agent/ComposerSettings.css
web/src/agent/ComposerSettings.surface.test.tsx
web/src/agent/Details.provenance.test.tsx
web/src/agent/SessionSettings.test.tsx
web/src/agent/SessionSettings.tsx
web/src/agent/SteeringMode.tsx
web/src/agent/TurnCompletion.tsx
web/src/agent/styles.test.ts
web/src/agent/turnTiming.ts
web/src/resources/SharedAgent.test.tsx
```

Main commits: `d46c3b150` settings/draft, `492cfc96f` completion and transport proof,
`f6bc17944` merge 06/03 and share progress, `375fd1ebd` browser proof, `a19e88697`
merge 11 and share latest-turn identity. Final evidence commit verifies actual
light/dark browser appearance and formats changed tests. Worktree retained for
the independent merger; no production/remote writes, PR, push or edits in other
worker/source trees.

# Ticket 12 delivery evidence

Date: 2026-11-03 (Asia/Singapore). Branch: `codex/cma-ui-12`. Worktree:
`/Users/liuyitian/code/cma-ui-worktrees/ticket-12`.

Started clean at integration `ef5e20c1f`. Style and in-progress tracker were
committed first as `63e0b6141`. Implementation is `fe057a6cb`; public/browser
proof is `a3e625f89`. Integration through `d19c225bd` is merged, including 04 and
14. The browser attachment selector is corrected for the retained Computer
panel in `c8d3c5d08`. Integration source remains unchanged by this worker.

## Delivered behavior and approved seams

| Acceptance | Evidence |
| --- | --- |
| Full text, focus, no implicit send | Real AgentSurface/controller/composer tests insert a supplied generated-set boundary fixture with newlines and surrounding spaces; no network send occurs. Production templates use the same editor. |
| Existing draft conflict | The specified first test starts with `保留预算上限` plus uploaded `budget.txt`, chooses Edit then Cancel, and verifies the unchanged body, attachment and composer focus. Append preserves the original multiline body and adds a newline; Replace preserves the attachment and offers Undo replacement. |
| Undo and subsequent input | Undo restores the previous text and focus. Manual input, another applied edit or submission retires that replacement undo. It cannot reappear merely because the user types the same text again. |
| Normal admission and immutable recovery | The real controller and SDK send the edited body with ticket 11's existing answer condition. Tests hold or lose receipts, edit the draft, recover the identical retained body/scope/key and then observe a fresh request identity for the newer intent. AG-UI and Responses are both exercised. |
| Failure and late acknowledgement | Definite refusal and uncertain receipt keep the edited body and attachments. A late acknowledgement cannot remove text or attachments retained by a newer replacement. Suggestions remain until confirmed acceptance or source invalidation. |
| Owner and Agent boundaries | Account change closes the old edit dialog; the new account receives no old body or attachment. Navigating between two actual Agent surfaces retains each Agent's draft separately. An expired edited source becomes ordinary input; an account mismatch is rechecked before posting and the owner's draft survives reopening. |
| Source expiry and scoped refusal | A newer failed turn withdraws the open decision without editing the draft. An explicit stale-answer refusal preserves the body; a later deliberate Send uses ordinary admission. No automatic unconditional retry is added. |
| Desktop, mobile, keyboard and touch | Six real Chrome cases cover 1440, 390 and 320px in light/dark themes, attachment upload receipts, Cancel/Escape, append, replace/undo, empty insertion, focus, failure and uncertain recovery. |
| Public history and ownership (A) | `edited_reply_keeps_exact_body_owner_and_identity_in_public_history` exercises AG-UI and Responses, with and without the existing condition. The public finite history contains exactly one user message `保留预算上限\nCompare latency and cost.` in the correct Agent. Another owner cannot submit or read it; same-key recovery returns the same command, and changing its body under that key is refused. |

The U tests use real AgentSurface, Composer, useAgentDraft, controller and SDK;
only external service/HTTP boundaries are controlled. They do not substitute an
`onEdit` or `onSend` mock for composer behavior. The A test uses real public
routers, authority/admission, command worker and finite history with an in-memory
store and controlled external runtime sender. Browser tests run the built app
in fresh Chrome contexts with a local HTTP fixture. These are complementary
approved seams, not a deployed end-to-end claim or a new native runtime proof.
No Layer 2–4 implementation, SDK wire shape or transport protocol changed.

## TDD and verification

Evidence directory (outside the repository):
`/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/ticket12-evidence`.

The initial required red is `01-cancel-red.log` (one failing test: edit action
absent); `01-cancel-green.log` records its first passing run. Subsequent recorded
red/green slices cover insertion (02), append (03), replacement/undo (04), normal
edited admission (05), immutable recovery/fresh intent (06), late-ack attachment
ownership (07), source expiry (09), explicit scoped refusal (10) and undo retirement
(11). The corrected stale-refusal red uses the actual public error contract.
Account and navigation checks also exercise existing isolation behavior rather
than manufacturing a production defect. The final edit suite has 14 tests.

| Check | Result | Retained log |
| --- | --- | --- |
| Merged Agent + connection suites | 116 files / 946 passed, no skipped tests | `merged-web.log` |
| Public normal admission/recovery | 4 actual named tests passed; the new test covers four protocol/condition combinations | `merged-public-admission.log` |
| Web and E2E TypeScript | Passed | `merged-typecheck.log`, `merged-e2e-typecheck.log` |
| Full Web ESLint | Passed | `merged-lint.log` |
| Production build | Passed; existing large-chunk warning remains | `merged-build.log` |
| Baseline Chrome on `ef5e20c1f` archive | 6 passed | `browser-before.log` |
| Final merged Chrome | 6 passed | `browser-final.log` |
| Changed TS/TSX/CSS formatting | Passed | `format.log` |
| Changed Rust formatting | Passed with pinned rustfmt; no unrelated formatting applied | Command below |
| Git diff whitespace and secret scan | Passed; no allowlist changes | `secret-scan.log` and final Git check |
| Ticket-owned source/test bounds | All within 500/2000 lines | Manifest below; AgentSurface 488, Composer 479, edit test 486 |

Rust uses the private target
`/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/ticket12-cargo-target`.
It was APFS-cloned from dependency cache, then `layer1-public`, `layer2-core`
and `layer3-adapter` were cleaned before compiling this worktree. Logs name this
worktree's crate paths and all four actual tests. The merged run rebuilds all
three layers and reports 4 passed / 419 filtered out, not a zero-test cache hit.
Builds use two jobs and the pinned toolchain via `/Users/liuyitian/.cargo/bin`.

### Known integration gates outside ticket 12

- The merged CSS check fails because `.agent-files-panel` is declared in both
  `web/src/agent/environment/AgentComputer.css` and
  `web/src/agent/files/AgentFiles.css`. An isolated archive of integration
  `d19c225bd`, without ticket 12, reproduces the same failure in
  `integration-css-baseline.log`. This is a ticket-14 integration issue, not an
  original-baseline failure; neither stylesheet nor the gate was changed here.
  Before merging 14, the ticket-12 CSS check passed.
- The merged size gate flags `api/crates/layer2-core/src/sandbox/agent_filesystem.rs`
  (515), `web/src/agent/conversation.ts` (503), and the already-known
  `api/crates/layer2-core/src/sandbox/agent_operation_service.rs` (508). All three
  files are byte-identical to integration `d19c225bd`. See `merged-size.log`.
  The first two belong to the merged computer work; the original Agent service
  remains untouched. Ticket 04 has resolved the former Console size failure.
- No full-repository green claim is made. Existing skill fixtures and unrelated
  baseline formatting were not modified, skipped or weakened. The merged
  Agent/connection run includes the seven disclosure assertions assigned to 14
  and passes them without exclusions.

## Browser review and reproduction

Before images are in `browser-before/`; final images are in `browser-final/`,
under the evidence directory. Each final case retains `conflict.png`, `edited.png`,
`failed.png`, `uncertain.png` and `recovered.png`. Screenshots disable finite CSS
animation while capturing the settled state. Final desktop light edited input,
320px dark conflict and 320px light failure were visually inspected. Controls
wrap, the dialog remains usable, attachments stay visible and compact settings
and the shared completion header remain intact with the Computer panel present.

From the ticket worktree:

```sh
npm --prefix web test -- --configLoader=runner --no-cache --maxWorkers=2 src/agent src/connections
npm --prefix web run typecheck
npm --prefix web run e2e:typecheck
npm --prefix web run lint
npm --prefix web run build -- --configLoader runner
PATH=/Users/liuyitian/.cargo/bin:$PATH CARGO_BUILD_JOBS=2 \
  CARGO_TARGET_DIR=/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/ticket12-cargo-target \
  RUST_MIN_STACK=16777216 cargo test --locked --manifest-path api/Cargo.toml \
  -p layer1-public --lib --target aarch64-apple-darwin reply_ -- --nocapture
PATH=/Users/liuyitian/.cargo/bin:$PATH rustfmt --edition 2021 --config skip_children=true \
  --check api/crates/layer1-public/src/agui/tests.rs \
  api/crates/layer1-public/src/agui/tests/suggestion_edit.rs
```

For browser reproduction, start an owned loopback preview with
`npm --prefix web run preview -- --host 127.0.0.1 --port 49312 --strictPort --configLoader runner`,
then in another terminal run:

```sh
CMA_SUGGESTION_EDIT_URL=http://127.0.0.1:49312 \
  CMA_SUGGESTION_EDIT_EVIDENCE=/tmp/cma-ticket12-browser \
  CMA_E2E_BROWSER_PATH='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
  web/node_modules/.bin/playwright test --config web/e2e/suggestion-editing.config.ts
```

Stop the preview afterward. This task retains its worktree for parent cleanup;
it does not push, deploy, open a PR or merge into the integration worktree.

## Scoped manifest

Exactly these 16 paths differ from integration `d19c225bd`; the list includes
this evidence file and the tracker/handoff documentation:

```text
api/crates/layer1-public/src/agui/tests.rs
api/crates/layer1-public/src/agui/tests/suggestion_edit.rs
docs/Web_Style.md
docs/plans/cma-ui-optimization/12-EVIDENCE.md
docs/plans/cma-ui-optimization/SUGGESTION_CONTRACT.md
docs/plans/cma-ui-optimization/issues/12-suggested-reply-edit.md
web/e2e/suggestion-editing.check.ts
web/e2e/suggestion-editing.config.ts
web/src/agent/AgentSurface.suggestionEditing.test.tsx
web/src/agent/AgentSurface.suggestions.test.tsx
web/src/agent/AgentSurface.tsx
web/src/agent/Composer.tsx
web/src/agent/ComposerSuggestionEditor.tsx
web/src/agent/SuggestedReplies.css
web/src/agent/SuggestedReplies.tsx
web/src/agent/useSuggestionEditing.ts
```

# Ticket 08 — safe answer deliverable preview

Date: 2026-11-03. Status: done. All seven original acceptance criteria are unchanged.

Worktree: `/Users/liuyitian/code/cma-ui-worktrees/ticket-08`.
Branch: `codex/cma-ui-08`. Confirmed starting commit:
`01656f54f0c42a9e891b906a29172d9e1cbe1d24` (07 and 12 integrated).
Evidence root: `/Users/liuyitian/code/cma-ui-worktrees/evidence/ticket-08`.

## Implementation and boundaries

`2baa0a81c` appends the design rule without changing any earlier Web Style byte.
`43171c01c` implements the preview; `441524648` merges integration
`a2965df09` (including 10 and 13); `e5b38408b` fixes the callback dependency.
The style merge was asserted equal to the entire incoming integration document
plus the exact original ticket 08 append. No generated image is committed.

Cards select Agent + answer + delivery identity. Preview reads/downloads use the
unchanged 07 SDK, including exact reference, size, SHA-256 and post-buffer access
checks. Live files are verified live resources, not claimed immutable snapshots.
Documents keep the referenced revision when a newer revision exists. No new
API, SDK contract, message protocol, resource publication or model generation is added.

`DeliverablePreviewFrame` wraps the existing AppFrame; the mounted details/file
subtree stays in place while the preview replaces its visible column. The
conversation and composer are sibling content and keep their node identities.
Closing restores the originating card after inert overlay removal; Escape also
works after a native download moves focus out of the preview. Reopening the same
card creates fresh preview state. Each abandoned read/download is aborted.

The read-only renderer reuses FilePreview and MessageContent. Markdown uses inert
links and media. HTML/SVG are decoded as text; no raw document is framed or opened
as an interactive same-origin page. Raster previews use the existing allowlist.
The existing 1 MiB preview limit is checked before requesting download bytes.
PDF/unsupported formats retain an authorized opaque download and explicit fallback.
Resource authorization renews on focus, every 30 seconds, and at the existing
55-second maximum deadline. Refresh hides content; denied/error/expired context
removes it. Sharing the conversation does not authorize private files.

## Acceptance evidence

| Original criterion | Evidence |
| --- | --- |
| Answer-bound selection and highlight | Whole Console test opens A, then B, and closes; exact SDK identity/bytes checks; browser card selection. |
| Safe text/Markdown/raster; PDF fallback | UTF-8 read-only content, GFM heading, decoded PNG with natural width 1, explicit PDF fallback in real Chrome. |
| Distinct states and stale A → B cancellation | 17 preview tests include loading, failure/retry, 1 MiB guard, deleted/changed/denied/not-ready/resource-too-large/unavailable, newer document revision, aborted late A and same-card recovery. |
| No script or untrusted navigation | HTML/SVG samples remain textarea values; Markdown links/images are inert; browser asserts no active script/frame/object/embed/link/SVG and no external requests or execution flag. |
| Preserve conversation and pending work | Console node identity/draft/scroll test; Chrome keeps exact composer/history/file-editor nodes, uploaded attachment, draft, and delayed save across file → preview → file → details. |
| Mobile return and keyboard | Chrome 390px touch open/close restores original card focus; 1440px Enter opens/downloads known bytes and Escape closes; column width measured after existing animation. |
| Independent shared permissions | Real SharedAgent U test uses only view_details/view_history, independently reads a permitted document, refuses the private file, has no Save/composer, and clears revoked bytes. Public Router tests separately enforce document access and revocation. |

## Red → green

Initial test setup exposed an authorization fixture omission and synthetic scroll
geometry; those were corrected before accepting evidence. `first-red-authorized.log`
then proves the approved Console scenario fails with the original AppFrame wiring.
`first-and-safe-green.log` proves the complete two-resource draft/position behavior.
Additional independent failures and passing slices are retained as
`safe-markdown-red.log`, `revocation-{red,green}.log`, `limit-{red,green}.log`,
`pdf-{red,green}.log`, `version-{red,green}.log`, and `reopen-red.log` plus `targeted.log`.

Real Chrome found two issues that DOM tests did not: mobile focus attempted while
the center was still inert, and desktop Escape after download had left the panel.
`chrome-second.log`, `chrome-focus-red/`, and `chrome-focus-green.log` preserve the
failures. `chrome-final.log` and `merged-chrome-final.log` pass all four scenarios.
Screenshots were visually inspected, including `preview-1440.png`,
`preview-390.png`, `unsafe.svg-390.png`, `file-save-390.png`, the Markdown/PDF/PNG
samples, and revoked-content states.

## Executed validation

| Check | Actual result | Evidence under the evidence root |
| --- | --- | --- |
| Focused U and neighboring regressions | 10 files / 65 passed, repeated after integration | `targeted.log`, `merged-targeted.log` |
| Shared history-only reader | 1 file / 1 passed (also in focused run) | `shared-preview.log` |
| Full Web after integration | 333 files: 330 passed, 3 failed; 2,985 tests passed, 8 failed | `merged-web-full.log` |
| Answer delivery public Router | 5 passed, 441 filtered | `merged-public-api.log` |
| Exact OpenAPI inventory test | 1 passed, 445 filtered | `merged-openapi.log` |
| TypeScript SDK answer deliveries | 1 file / 9 passed | `merged-sdk.log` |
| Actual Chrome | 4 passed, 1440px and 390px | `merged-chrome-final.log` |
| Web and E2E TypeScript | Passed | `merged-typecheck.log`, `merged-e2e-typecheck.log` |
| ESLint | Passed | `merged-lint.log` |
| Production Web build | Passed; existing large-chunk notice | `merged-build.log` |
| Owned-file Prettier / diff whitespace | Passed | `format.log`, `diff-check.log` |
| CSS collision gate | One incoming collision, none owned by 08 | `css.log` |
| File-size gate | Four incoming violations; all 08 paths within 500 source / 2,000 test lines | `file-size.log`, `owned-lines.txt` |

Rust uses toolchain 1.96.1, jobs 2, `RUSTFLAGS='--cfg cma_ui_ticket08'`,
`RUST_MIN_STACK=33554432`, and the private `${evidenceRoot}/cargo-target` directory.
Actual named tests ran in
`cargo-target/debug/deps/layer1_public-a488534b15eaea02`, SHA-256
`a8a5eb417a59d676779c62bc574bd0fa4d23ae65153c218f8dd400769c840da1`.
No shared target cleanup, Mongo database, container or VM operation was needed.

Commands (run from this worktree, evidence redirects omitted):

```sh
npm --prefix web test -- --configLoader=runner --no-cache --maxWorkers=2
npm --prefix web run typecheck
npm --prefix web run e2e:typecheck
npm --prefix web run lint
npm --prefix web run build
npm --prefix web run check:css
node web/node_modules/vitest/vitest.mjs run --root sdk/typescript --configLoader=runner --no-cache --maxWorkers=2 tests/answer-deliverables.test.ts
cargo +1.96.1 test --offline --locked --manifest-path api/Cargo.toml -p layer1-public --lib rest::answer_deliverables_tests:: -- --nocapture
cargo +1.96.1 test --offline --locked --manifest-path api/Cargo.toml -p layer1-public --lib rest::openapi::tests::every_exported_operation_appears_in_the_rest_documentation_inventory -- --exact --nocapture
node web/node_modules/@playwright/test/cli.js test --config web/e2e/deliverable-preview.config.ts
```

Vite uses `--configLoader=runner --host 127.0.0.1 --strictPort` and an OS-selected
ephemeral port recorded in `vite-port.txt`. Chrome uses the installed executable
with fresh Playwright profiles. `CMA_PREVIEW_URL` supplies the local origin;
`CMA_E2E_BROWSER_PATH` supplies the Chrome binary. Browser HTTP fixtures prove U,
not a deployed provider. The 07 native file/document provenance proof remains the
prerequisite; 08 adds no runtime capability and executes no dynamic model generation.

## Manifest and shared hunks

`manifest.txt` lists the exact 21 owned paths relative to the integrated tip.
`shared-hunks.patch` isolates the existing component changes. No API/SDK source,
AgentSidePanel, AgentComputer, AgentSurface, MessageStream or replay/browser worker
container is edited by 08. Their incoming integration changes are not claimed here.

| Shared path | Owned change |
| --- | --- |
| `web/src/Console.tsx` | Replace AppFrame import with preview frame wrapper; pass viewer/Agent/access scope and panel-close callback (3-line seam). |
| `web/src/agent/AnswerDeliverables.tsx` | Accessible name button, exact selected state, access invalidation, shared opaque save helper. |
| `web/src/agent/AnswerDeliverables.css` | Selected/focus/44px name-button rules in the selector's existing owner. |
| `web/src/agent/MessageContent.tsx` | Optional inert link/image mode; conversation default unchanged. |
| `web/src/agent/files/FilePreview.tsx` | Export existing raster allowlist; optional hidden read-only preview header. |
| `docs/Web_Style.md` | Strict append only. |
| Tracker | Issue 08 checkboxes/status, this evidence file, and implementation entry. |

New files are `DeliverablePreview{,Frame}.tsx`, `DeliverablePreview.css`,
`previewSelection.ts`, `previewRead.ts`, `saveDelivery.ts`, the three preview U
tests, and the three `web/e2e/deliverable-preview*` files.

## Limits and unrelated checks

The full Web failures are the six existing AppPublic assertions, the event URL
assertion missing `tool_output=true`, and TaskHome public-copy vocabulary. The
incoming CSS collision is `.agent-files-panel` in AgentComputer/AgentFiles CSS.
The source-size gate reports incoming `message_feedback.rs` 556,
`agent_filesystem.rs` 517, `conversation.ts` 503, and the original
`agent_operation_service.rs` 508. Parent integration-repair owns the integration
issues; 08 neither duplicates their fixes nor weakens a gate. The original skill
fixtures/full Rust formatting/LocalAgentV2 head-404 limitations remain separate;
no full Rust or production-green claim is made. The browser only samples the
listed dangerous content, not all possible file formats.

Only this worktree is changed. No push, PR, deployment, production operation,
dynamic model call, or outgoing message is made. Own Vite/Chrome processes are
stopped after validation; shared Mongo/VM and other workers remain untouched.

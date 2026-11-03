# Task 15 E2E type repair

Date: 2026-11-03. Branch: `codex/cma-ui-e2e-types`.
Worktree: `/Users/liuyitian/code/cma-ui-worktrees/e2e-types-repair`.
Integration base: `40b627b899f742ac7178a18622dd002fcb531254`.
Scope: `web/e2e/browser-live-chain.check.ts` and this evidence document only.
The approved task 15 U/A/R seams and assertions remain in place.

## Red → green

The first actual `npm run e2e:typecheck` in this worktree's `web/` exited 2:
all 11 reported task 15 diagnostics reproduced, alongside 21 SDK diagnostics
caused by the absent local `eventsource-parser` dependency. The existing Web
dependencies were linked; the SDK dependency was not. An ignored link inside
this worktree now reuses the installed `eventsource-parser` 4.1.0, matching the
SDK manifest and lockfile. No installation or dependency/configuration edits
were needed. Repeating the unchanged command before editing isolated exactly
the 11 task 15 diagnostics at original lines 113, 115, 135–137, 200 and 203.

The repair adds runtime assertions for the displayed image, nonempty encoded
frame, matching authorized receipt/frame, and both navigation results. A reversed
copy followed by `find` retains the newest matching receipt without mutating
receipt order or requiring ES2023. Narrowed page A/B records replace unchecked
array indexing. No casts, non-null assertions, suppressions or skips were added;
compiler settings and gates are unchanged.

All original navigation, dimensions, byte budget, identity, frame hash/sequence,
authorization, stale-incarnation, reconnect, polling, draft and mobile assertions
remain. Successful proof behavior and outputs are unchanged; missing evidence
fails explicitly before dereference/decode. No source feature changes or new
mirrored tests were needed.

## Validation and artifacts

Commands run from this worktree's `web/` unless indicated:

| Command                                                                                                                      | Result                         | Raw log                        |
| ---------------------------------------------------------------------------------------------------------------------------- | ------------------------------ | ------------------------------ |
| `npm run e2e:typecheck` before editing, after dependency link                                                                | Exit 2; 11 task 15 diagnostics | `e2e-typecheck-red-linked.log` |
| `npm run e2e:typecheck` after repair                                                                                         | Exit 0                         | `e2e-typecheck-green.log`      |
| `npm run typecheck`                                                                                                          | Exit 0                         | `web-typecheck.log`            |
| `./node_modules/.bin/eslint e2e/browser-live-chain.check.ts --max-warnings=0`                                                | Exit 0                         | `focused-eslint.log`           |
| `./node_modules/.bin/prettier --check e2e/browser-live-chain.check.ts ../docs/plans/cma-ui-optimization/E2E-TYPES-REPAIR.md` | Exit 0                         | `focused-prettier.log`         |
| `git diff --cached --check` from worktree root                                                                               | Exit 0                         | `diff-check.log`               |

Raw logs, exit codes, initial dependency failure/setup, integration merge receipt,
final diff, commit identity and exact file SHA-256/Git blob manifest are outside
the repository under
`/Users/liuyitian/code/cma-ui-worktrees/evidence/e2e-types-repair/`.
See `manifest.json` for the final commit, integrated ref and file identities.

No native rerun was needed for these equivalent lookup/narrowing changes. No
native pass is claimed by this repair, and no server, native VM, MongoDB or
production resource was started or modified. The original workspace (including
its 46 existing changes and concurrent document) and the active 16/17 worktrees
received no writes. The parent independently merges this bounded repair.

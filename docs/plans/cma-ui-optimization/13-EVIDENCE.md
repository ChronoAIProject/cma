# Ticket 13 delivery evidence

Date: 2026-11-03. Branch `codex/cma-ui-13`, isolated worktree
`/Users/liuyitian/code/cma-ui-worktrees/ticket-13`; initial integration base
`424b1534e`. Implementation and runtime evidence are delivered. Tracker remains
in-progress solely for literal independent human quality signoff at full-delivery
review. Ticket 12 editing is integrated and machine proof work is complete.
Actual billed cost remains unknown because the provider supplied no monetary data.

## Delivered behavior

Confirmed successful turn recording atomically inserts a bounded Mongo outbox
entry. The canonical `AgentTranscriptTurn::final_answer()` supplies the existing
`event:<sequence>` identity. Background workers claim each answer/strategy once,
obtain current sandbox credential custody and the existing title-worker model
proxy endpoint, and invoke the bounded HTTP Responses adapter with no tools.
No suggestion text is admitted as user input, and no suggestion Agent turn exists.
Main-answer recording never waits for model generation.

The only new public operation is authenticated owner GET `/api/v1/agents/{id}/suggestions`
with the existing answer scope. SDKs expose the same read. GET/transcript/history
do not enqueue, generate, repair, retry or admit work. The browser reads at most
50 times / 75 seconds while pending, hides terminal failure or absence, and uses
ticket 11's conditional normal-send path for exact text. It preserves composer
drafts and uncertain-send recovery. The merged ticket-12 editor consumes the saved
set through the same existing identity and conditional normal-send contract.

The `agent_suggestions` collection uses a unique owner/Agent/answer/version key,
majority transactions, claim-before-call and a terminal interrupted state after
worker loss. There is no automatic regeneration after uncertain billing. Pending
work older than two minutes expires. Two workers per process, 60-second model
timeout, 80-second complete worker bound, 128 exact-turn events, 4 KiB goal,
8 KiB answer, 20 KiB request, 24 KiB response and 1,024 output tokens bound work.
Indexes cover current-answer admission checks and job discovery. Agent purge
also purges its saved suggestions; shared Mongo/VM remain running.

Current owner, Agent lineage, runtime, credentials/grants, profile/preferences,
new admissions and canonical turn state fence public reads and completion. A
transactional Agent write conflicts concurrent admission, and a late result
keeps its measured usage while exposing no old set. Strategy upgrades do not
silently regenerate previously billed answers. Version 3 is immutable for this
delivery; changed generation behavior requires a new strategy version.

Candidate parsing enforces exact JSON, 0–3 items, bounded text, grounded excerpts,
dedupe, single-intent lexical guards, no generic continuation, unsupported
resource/external-action claims, invented numeric constraints or first-person
personal assertions. Prompts also require semantic consistency. These guards
and five examples do not prove universal semantic correctness. There are no
confirmed artifacts in this version; ticket 07 is not a dependency.

## Quality and measured model work

[Fixed rubric](13-QUALITY-RUBRIC.md) was committed before sampling.
[Actual samples and semantic judgments](13-MODEL-SAMPLES.md) retain all five
final exact outputs, public saved sets and per-case observations. English
comparison, text edit, Chinese clarification, closed arithmetic and false
artifact declaration are covered. All five ran through the production adapter,
real authorized model route, confirmed-turn recorder, Mongo persistence,
authenticated public reads and separately reopened store. Three useful final
sets and two deliberate empty sets were returned; no template passes dynamic
acceptance. Earlier v1/v2 rejected clarification outputs are retained openly.
The [verbatim independent parent AI review](13-PARENT-AI-REVIEW.md) passes all five
v3 samples after reading the fixed rubric, actual raw outputs and saved public
JSON. This independent AI review is explicitly not literal human approval.

Final run: five calls, 2,200 input tokens, 0 cached input tokens, 270 output
tokens; measured generation total 35,629 ms (mean 7,125.8 ms, range
2,792–15,930 ms). Actual billed cost is unknown because this route returned no
cost field/header. All real observations persist null, not zero or an estimate.
A separate controlled HTTP fixture verifies explicit reported cost persistence;
its synthetic price is not actual billing evidence. The parent's availability
probe is excluded from adapter, persistence, quality and cost evidence.

The local proof forwards synthetic input through the current authorized local
configuration with credentials only in memory. It does not prove a deployed
sandbox endpoint or a production SLA. The production adapter is the same Rust
adapter; production credential resolution reuses existing runtime/title ports.
The private local proxy stops after five calls and never forwards tools or
starts a CLI Agent. Raw reasoning payloads and credentials are not retained.

## Tests and artifacts

### Explicit real-model sample invocation

The five-case external-model proof is compiled only with the test-only
`real-model-suggestion-samples` Cargo feature and remains ignored. Default Mongo
selection retains the deterministic suggestion HTTP/persistence/lifecycle tests.
After explicit authorization for the real model and its cost, configure
`CMA_SUGGESTION_SAMPLE_PROXY` with the authorized loopback proxy,
`CMA_SUGGESTION_SAMPLE_MODEL` with the approved model,
`CMA_SUGGESTION_SAMPLE_OUTPUT` with an evidence path outside the repository, and
`CHRONO_TEST_MONGODB_URI` with an owned isolated replica set. From `api/`, run:

```sh
cargo +1.96.1 test -p cma --bin cma --locked \
  --features real-model-suggestion-samples \
  store::sandbox_mongo_integration_tests::command_ledger_tests::contextual_suggestions::samples::contextual_suggestions_real_samples_are_saved_and_read_through_the_public_route \
  -- --ignored --exact --test-threads=1
```

Missing sample configuration still fails before database or model access. Enabling
the feature alone does not run the proof; `--all-features` compilation/lint covers
it. A successful run records real samples for review and does not supply human
quality signoff.

### Recorded delivery results

All logs below live under `/Users/liuyitian/code/cma-ui-worktrees/` unless
otherwise noted. Rust builds use private `cargo-target-ticket13` and
`cargo-target-ticket13-sdk`, two jobs, and verified named nonzero test counts.
Mongo tests create and drop their own `ticket13_*` databases on the shared
`cma-ticket11-mongo` replica set.

| Boundary | Result | Artifact |
| --- | --- | --- |
| First UI red: A arrives after accepted B | Fails before fetch integration; passes after scope fence | `ticket13-red-ui.log`, `ticket13-green-ui.log` |
| First public red: old A after normal POST | pending/expired mismatch becomes passing public read | `ticket13-red-late-api.log`, `ticket13-green-late-api.log` |
| Public generation lifecycle, filter, bounds, timeout, authority, admission | 8 tests passed | `ticket13-public-final.log` |
| Actual Mongo recorder/outbox, HTTP adapter, stable reopened GET, replica exclusion, worker loss, late authority and admitted input | 4 tests passed | `ticket13-mongo-final.log` |
| Real five-case model generation + persisted public reads | 1 named test / 5 calls passed | `ticket13-proof/samples-v3-run.log` |
| Whole-conversation UI + existing suggestions/connections | 41 tests in 3 files passed | `ticket13-web-final.log` |
| Metrics red/green: visibility, confirmation, failed bound task | red then included in 24/24 and final 41/41 | `ticket13-red-metrics.log`, `ticket13-green-metrics.log` |
| TypeScript SDK GET and six shared contract cases | 7 passed; 2,711 unrelated cases filtered | `ticket13-sdk-ts-contract.log` |
| Rust SDK exact GET and six-case shared corpus | 2 named tests passed | `ticket13-sdk-rust-contract.log` |
| Public shared HTTP corpus for this operation | 1 test / 6 cases passed | `ticket13-contract-final.log` |
| OpenAPI operation/problem inventory and REST doc inventory | 1 test each passed | `ticket13-inventory.log`, `ticket13-rest-docs.log` |
| Web and E2E typecheck, focused ESLint, Web build | Passed; existing large-chunk advisory only | `ticket13-web-types-final.log`, `ticket13-e2e-types.log`, `ticket13-web-lint.log`, `ticket13-web-build.log` |
| Real Chrome, generated text, saved reload, exact send, late A | 5 tests passed | `ticket13-browser.log` |

Chrome screenshots are in `ticket13-proof/browser`: desktop 1440 px and narrow
320 px, light/dark, pending answer and saved-result states. Desktop light and
320 px dark screenshots were visually inspected: no clipping, reachable 44 px
buttons and preserved draft. Browser proof uses the actual sample text with
fixture identities and a controlled HTTP boundary; it does not masquerade as a
browser attached to a deployed production backend.

## Metrics and ticket 12 handoff

`suggestionMetrics.ts` emits bounded local browser events for visible enabled
set exposure, actual send confirmation (`original` / `edited`), and the exact
receipt's durable task outcome. It never treats a click or acceptance as task
success. No analytics platform, remote collection or raw message text is added.
See [shared contract](SUGGESTION_CONTRACT.md) for dedupe, denominators and the
`confirmSuggestionSend(set, itemId, 'edited', receipt)` seam. The merged
`useSuggestionEditing` observes its existing normal-send callback and retained
request; ticket 13 forwards the existing optional receipt without changing any
message protocol. It remembers only the existing set/item and request identity,
records nothing before acceptance, and correlates recovered receipts after the
group disappears. `useAgentSuggestions` keeps this composition out of the
495-line AgentSurface. Ticket 12's composer behavior remains the only editor.

## Acceptance still pending

- An actual person must review the fixed rubric and exact samples at the final
  full-delivery review. Independent parent AI review passed all five v3 samples,
  but it is not human approval. This precise item remains unchecked and does not
  stall remaining authorized ticket work.
- Actual billed-cost confirmation is unavailable from this route. Null reporting
  and usage/latency evidence are implemented, but no actual monetary estimate is
  claimed.

Original formatting/skill fixtures/508-line Agent service limits remain separate
from the task-introduced AppCreation navigation test and Console503 issues fixed
by incoming ticket 04. No unrelated broad failure is reclassified here.

## Earlier ticket-04 integration checkpoint

Commits `d0cff82a0` (production adapter/store/SDK/evidence) and `3a1aa9a46`
(visibility and recovered-receipt metrics) follow the initial style commit
`1455d4d06` and first red/green implementation `f958b0bae`. Integration
`a426b87ee` was merged at `1770220fc`; the subsequent ticket-04 integration
`d19c225bd2a373f0e4861e95ebbbb5a5bca8adc8` was merged at `543ea035e`.
Both conflicts were only the append point in Web Style: the complete incoming
file was retained byte-for-byte as a prefix, followed by the unchanged ticket-13
block. No integration/original/other-worker files were edited in place.

After the final merge: 53 whole-conversation U tests passed (four files), as did
Web/E2E typechecks, focused lint, eight public generation tests, four Mongo tests,
the six-case ticket-13 HTTP corpus and seven selected TypeScript SDK tests. Both
Rust SDK tests also passed: exact scoped GET plus the shared corpus (five typed
HTTP cases and one invalid query wire shape). Detailed final
logs use the `ticket13-after04-` prefix. The real model prompt, adapter, fixed
cases and filtering were unchanged by integration; no paid sample rerun was
performed. The preceding merged Chrome proof passed five tests and retained
screenshots under `ticket13-proof/browser-merged`.

Recovery metrics now retain only the existing pending journal key and scope,
then confirm the matching accepted receipt after the group unmounts; they do not
change send/retry behavior. Hidden-document exposure is excluded and becomes
eligible on a real visibility change. Exact regression red/green artifacts:
`ticket13-red-recovery-metric.log`, `ticket13-red-visibility.log`,
`ticket13-green-metric-recovery.log` (24 passed, no unhandled errors).

One incoming integration defect remains outside ticket 13: the shared public
contract inventory lacks ticket 14's `GET /api/v1/agents/{id}/environment` entry
and cases. The incoming integration declares 227 operations despite adding that
route; ticket 13 adds its own entry and six cases, yielding a declared 228 versus
actual 229 after merge. `ticket13-after04-inventory.log` records that precise
failure. The checked REST documentation inventory passes after incoming
`54a7414b6`; it is distinct from the still-missing shared contract coverage.
Ticket 13's public operation and named error cases pass. This gap is reported to
parent without expanding into ticket 14 or mislabelling it an original baseline.

Task source files remain at or below 500 lines after incoming 04's extraction
(`AgentSurface.tsx` 491, OpenAPI module 497). Owned preview/model servers are
stopped, failed-run ticket-13 databases are removed, and the shared Mongo/VM are
retained. Worktree and private build caches remain for parent cleanup; no push,
PR or deployment was performed.

Final Gitleaks scans of the ticket diff and external proof directory both pass
without an allowlist. The first proof scan flagged five provider cache identifiers
in the older v1 response metadata. Unneeded provider metadata was removed while
preserving the exact assistant outputs, requests, timing and usage. Final logs:
`ticket13-final-secrets.log`, `ticket13-proof-secrets.log`. No credential value was
printed or committed.

## Final ticket-07/12/10 composition and handoff

Latest integration `f793a41518fa969af068d7913ff6310dee7e040b` is an ancestor
through merges `88963638f`, `1c70eca3f` and `6de02dae9`. The final merge includes
07 deliverables, 12 editing and 10 feedback. Conflicts preserve all incoming
behavior and the complete incoming Web Style prefix, then append the original
13 style block. The shared client retains both scoped deliverables and saved
suggestions; OpenAPI retains both feedback and suggestions. No parent or other
worker worktree was edited.

Ticket 12's existing editor now receives persisted generated sets through
`useAgentSuggestions`. Its normal-send callback and immutable uncertain-request
journal feed ticket 13's metrics. A new two-case red first showed missing edited
confirmation on immediate acceptance and lost-receipt recovery. The green verifies
exact body/condition/key retention, no event before acceptance, one edited
confirmation, preserved newer draft and a separate receipt-bound completed task.
The existing 14 editing tests still pass, including cancellation, replacement
undo, attachments, ownership changes and expired-source behavior.

| Final focused verification | Actual result | Log under `/Users/liuyitian/code/cma-ui-worktrees/` |
| --- | --- | --- |
| Edited-metric regression red | 2 failed at missing metric assertions | `ticket13-red-edited-metrics.log` |
| Combined suggestion/edit/connection/surface/delivery/feedback UI | 88 passed / 7 files, no unhandled errors | `ticket13-green-edited-metrics.log` |
| Final hook lifecycle recheck | 40 passed / 3 suggestion files | `ticket13-final-edited-metrics.log` |
| Public generation lifecycle | 8 passed, nonzero named tests | `ticket13-final-public.log` |
| Real Mongo adapter/persistence/restart/late result | 4 passed, 0 ignored | `ticket13-final-mongo.log` |
| Ticket-13 shared public HTTP corpus | 1 test / 6 cases passed | `ticket13-final-contract.log` |
| Focused OpenAPI composition | 10 passed | `ticket13-final-openapi-recheck.log` |
| Checked REST documentation inventory | 1 passed | `ticket13-final-rest-docs.log` |
| TypeScript SDK scoped GET and shared cases | 7 passed, 2,737 unrelated tests filtered | `ticket13-final-sdk-ts.log` |
| Rust SDK scoped GET | 1 passed | `ticket13-final-sdk-rust.log` |
| Rust shared SDK corpus | 1 passed; 2,297 typed HTTP, 173 typed preflight, 241 wire-shape and 27 browser contract cases | `ticket13-final-sdk-rust-contract.log` |
| Chrome desktop/320px, light/dark, editing and late A | 6 passed | `ticket13-final-browser.log` |
| Web, E2E and TypeScript SDK types | Passed | `ticket13-final-types.log`, `ticket13-final-e2e-types.log`, `ticket13-final-sdk-ts-types.log` |
| Focused ESLint, changed formatting, whitespace and secret scan | Passed, no exclusions added | `ticket13-final-lint.log`, `ticket13-final-format.log`, `ticket13-final-rustfmt.log`, `ticket13-final-secrets.log`, `ticket13-proof-secrets.log` |

The merge's focused OpenAPI expectation initially retained the incoming 64 paths /
79 operations. Ticket 13 adds exactly one scoped read, so the expected counts are
now 65 / 80. The exact route/identity/schema checks remain intact. The initial
9-pass/1-fail evidence remains in `ticket13-final-openapi.log`; all ten pass after
that precise correction. The inherited diagnostic still prints a hardcoded
`operations=45`; the actual enumerated and asserted total is 80.

The independent integration inventory gap is still present: 235 actual operations
versus 230 declared. The missing entries are ticket 14's environment read and the
four ticket-07 deliverable operations; neither has shared JSON coverage in this
incoming checkpoint. Ticket 13 adds its own operation and all six cases, and 10's
two operations are preserved. `ticket13-final-inventory.log` retains the failure.
This is reported to the parent integration-repair work, without weakening the
inventory gate, claiming whole-repository green, or duplicating that worker's
CSS/size/shared-inventory/task-status/AppPublic work.

Browser artifacts are in `ticket13-proof/browser-final`. Final desktop light
saved-result and 320px dark edited-result screenshots were visually inspected:
text wraps, editing remains reachable, the composer retains the new body, and
07's scoped empty-delivery fixture introduces no spurious resource claim. These
are controlled browser-boundary checks using the exact accepted model sample.
Real model configuration, prompts, adapter, cases and filters did not change in
these merges; no additional paid samples or automatic generation retries ran.

The parent's report is copied verbatim to [13-PARENT-AI-REVIEW.md](13-PARENT-AI-REVIEW.md).
It passes all five v3 samples and is explicitly independent AI review. Literal
human/user quality acceptance remains the single unchecked criterion. Actual
cost remains null/unknown, with measured tokens and latency retained. Machine
delivery is ready for integration and does not stall tickets 16–19.

All ticket-owned production source files are within 500 lines and test files
within 2,000; AgentSurface and the top-level OpenAPI file are both 495 lines.
Owned Vite on port 4373 and all bounded model proxies are stopped. Shared Mongo
responds to ping and has no ticket-13 test databases left; it and its VM remain
running (`ticket13-final-cleanup.log`). Worktree and private targets are retained
for parent cleanup. No push, PR or deployment occurred.

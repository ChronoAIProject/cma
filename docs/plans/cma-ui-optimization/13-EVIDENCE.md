# Ticket 13 delivery evidence

Date: 2026-11-03. Branch `codex/cma-ui-13`, isolated worktree
`/Users/liuyitian/code/cma-ui-worktrees/ticket-13`; initial integration base
`424b1534e`. Implementation and runtime evidence are delivered. Tracker remains
in-progress for literal independent human quality signoff, unavailable actual
billed-cost confirmation, and the separately owned ticket-12 edit integration.

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
drafts and uncertain-send recovery. Ticket 12 owns editing, not this patch.

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
`confirmSuggestionSend(set, itemId, 'edited', receipt)` seam. Ticket 12 wires its
own edited normal-send/recovery callback; ticket 13 forwards the existing
optional receipt without changing any message protocol.

## Acceptance still pending

- An actual person must review the fixed rubric and exact samples. Current
  judgments are implementation-agent judgments only; parent AI review, if given,
  must remain labelled independent AI review. Artifact was sent to parent for
  independent review; no approval is fabricated.
- Actual billed-cost confirmation is unavailable from this route. Null reporting
  and usage/latency evidence are implemented, but no actual monetary estimate is
  claimed.
- Ticket 12's separately owned edit UI and edited-send recovery must integrate
  the shared set and metric callback. This worker does not duplicate editing.

Original formatting/skill fixtures/508-line Agent service limits remain separate
from the task-introduced AppCreation navigation test and Console503 issues owned
by ticket 04. No unrelated broad failure is reclassified or repaired here.

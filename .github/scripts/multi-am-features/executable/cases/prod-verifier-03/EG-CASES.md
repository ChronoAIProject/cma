# EG executable cases — author handoff

Part of #2149. Integration owner: developer-15. Author branch:
`tl/cma-13b/coverage-prod-verifier-03`. The frozen assignment remains all 79
rows, EG-001 through EG-079. All 79 rows remain registered. EG-023 and EG-026
are explicitly BLOCKED pending missing product workflows; the other 77 retain
authored actions, subject to behavioral auditing and runtime verification.
No row has a two-EG/two-AM runtime PASS; runtime execution remains parent-owned.
Registration and matching step IDs do not prove behavioral coverage of every
checklist clause. The authored cases still need verification with authentic
parent fixtures and the compiled product.

`index.mjs` exports individual definitions with the exact frozen step IDs.
Browser modules perform real Playwright UI actions, including personal report
completion and contents, permission/session boundaries, revisioned channel edits
and deletion. Platform cases require verified synthetic ingress through real EG
processes, independently observed non-owner AM attribution, provider acceptance
and actual synthetic platform delivery. Local fixtures do not establish live
third-party or production bot acceptance. Tool cases assert exact identities,
reply effects, pagination, current permissions and no-effect read behavior.

The shared browser adapter now exists at `executable/browser.mjs`. Cases use its
`open/evidence` interface and local session origin; browser attribution is retained
on success and failure. The parent must install it with its leased Chromium and
compiled web artifact. Specialized `eg_browser`, `eg` and `eg_accounts` fixtures
remain prerequisites. Their concrete methods and setup are in `fixtures.json`.
Authentic setup exports include `setups.channel_edit` plus parent-bound setup
functions for every remaining authored scenario. `channelEditSetup` creates and
cleans up a reserved disposable channel through real EG HTTP; `createEgTransport`
checks actual replica witnesses and exact source/image binding. The remaining
setups bind the parent's real disposable EG lease and seed configuration only;
they do not execute a case or return a result oracle. They need the explicit
`primitives.eg` contract in `fixtures.json`. Missing fixtures or methods return BLOCKED. No generic adapter-run scenario or
HTTP-success-only fallback substitutes for the real behavior.

Run focused offline verification from repository root:

```sh
node --test .github/scripts/multi-am-features/executable/cases/prod-verifier-03/helpers.test.mjs .github/scripts/multi-am-features/executable/runner.test.mjs
```

Focused verification (86 tests in the current source):

```sh
node --test .github/scripts/multi-am-features/executable/cases/prod-verifier-03/*.test.mjs .github/scripts/multi-am-features/executable/runner.test.mjs .github/scripts/multi-am-features/executable/serving.test.mjs .github/scripts/multi-am-features/executable/eg-evidence.test.mjs
```

Latest full executable verification: 314 tests passed, zero failed with
`node --test --test-concurrency=1` over all executable and author test files.
The earlier 308-test parallel run passed 307/308: the unchanged shared action-timing test
exceeded its 10 ms synthetic threshold under scheduling contention. That test
passed in isolation and in the complete serial run; no timing gate was changed. The focused suite covers frozen IDs,
exact steps, missing prerequisites, wrong event/delivery joins, duplicated effects,
unsafe-fixture cleanup, cursor cycles, wrong EG replica attribution and shared
browser evidence custody. These tests do not execute the product. The parent
owns N=3 before restart, N=1 after, exact image binding, the sole controlled AM
restart and HPA run. Browser-only and EG-only witnesses retain their true scope. AM-backed EG turns now require actual `amRequests` with joined owner proofs; `retainTurn` publishes `multi-am-eg-evidence-v1` for shared-runner ingestion. Requested AM labels never satisfy that evidence.

The 20 latest cases declare `user-fixtures` and use the real registry dispatcher;
an unavailable parent factory blocks before feature execution. EG-023 requests
`producer_edit`, separate from EG-022's one-source `channel_edit` setup. Cleanup
aliases reuse the same promise, retain the lease receiver and preserve rejection.
The HTTP helper forwards expected statuses and supports empty 204/404 bodies.
Regression tests exercise the dispatcher and local HTTP transport, not a product
campaign.

EG-019 now distinguishes managed connection capabilities from relay authoring
verification modes. The actual browser loads capability discovery from both EG
replicas, with response IDs joined to real EG serving receipts. It compares
native setup gates, platform/verification choices, reply destinations and thread
write availability, then opens an authentic retained raw-verification source
whose new writes are disabled. The saved selection and edited local draft stay
visible; a save attempt must be refused locally without downgrading or sending
a mutation. A browser guard records any attempted write as failure, and both-EG
record readback plus scoped platform/provider counters prove discovery had no
saved or delivery effect. This row declares `evidence_scope: eg_only`; EG
receipts never become AM ownership. Five offline regressions cover capability
schema separation, activation/provider gates, actual EG receipts and write-guard
cleanup. The sole parent still must supply the retained source and real EG
browser proxy/cookie/replica-switch bindings described in
`fixtures.json.channel_capabilities_browser_contract`.

EG-020 and EG-021 now configure the real channel form and exercise accepted
create-response loss. The browser generates separate record/idempotency UUIDs;
the case validates the submitted producer, consumer, grouping, reply policy and
credential hashes, then records cleanup custody before forwarding exactly one
POST. Recovery uses the actual Check original channel control on the opposite
observed EG process, compares durable ID/version/binding/definition and keeps
the original draft locked with explicit uncertainty. Only after that read-only
reconciliation does the peer UI reload and open the channel from actual list
responses. Addressed list rows do not require an extra detail GET. Parent
allocation observations must prove one record/channel/binding, no additional
create or credential replacement, and no unresolved provider work. These global
cases declare `eg_only`; the separate parent AM witness remains required.
Fourteen offline regressions cover intent/credential validation, custody before
dispatch, cancellation, duplicate mutation refusal, cleanup joins, actual EG
receipts and list/address recovery. Parent setup needs the low-level methods
in `fixtures.json.channel_creation_browser_contract`; no high-level creation
result delegate is accepted and no runtime PASS is claimed.

EG-023 has a source-confirmed missing UI clause. At the inspected author and
canonical source, `web/src/services/channelForm/NodeFields.tsx` and
`web/src/services/ChannelMap.tsx` expose producer add/edit, while
`web/src/services/EventGateway.tsx` supplies only channel deletion to the
inspector. There is no producer-removal control or routing-reference check in
that web workflow. The old case bypassed the UI and filtered by `source.id`, a
field absent from the source schema, so it could not establish the requested
removal. That invalid path has been removed. The frozen row now returns
`producer_removal_web_workflow_missing` after fixture custody and before any
feature mutation; it cannot count as executable coverage or PASS. A product
owner must supply the missing workflow or resolve the frozen entry-point
specification. Authoring its complete add/edit/remove journey remains required.
The fixture registration and exact three step IDs remain intact.

EG-024 now uses the actual Producer and Channel inspectors to save allowlist,
scoped sender/chat/group admission, both conversation grouping modes and reply
policy. Its previous API-only shortcut accepted any disposition and queried a
fictional aggregate partition shape; those checks are removed. The new case
requires verified signed local Telegram group events, actual cma_codex dispatch
through the two AMs and the parent fake model. Two senders and two conversations
must share/isolate actual Agent history as selected. The supported partition
inspection must join the same transport conversation, binding and Agent.

The bounded matrix has 10 accepted events and five exact authenticated policy
refusals: sender allowlist, scoped sender, chat, unmentioned group and bot sender.
A distinct bot identity avoids changing a human sender's platform identity.
Completion holds expose four real acknowledgements before model release; nine
final answers reach the original conversations. A final auto-final=false event
must produce no send. Independent capture/history and no-late-work barriers
must establish exactly 10 turns/effects, five context groups and 13 sends, with
no pending/unknown work. Each UI update is validated before its one real PUT,
then reloaded through the opposite observed EG. Duplicate or unexpected browser
mutations remain failures even after a successful save. AM and EG receipts stay
separate. Thirteen offline regressions cover disposition, wire/process identity,
context leakage/loss, real partition mapping, ack/final delivery, finality and
mutation/cancellation/cleanup guards. The named setup now requires the low-level
methods in `fixtures.json.channel_admission_browser_contract`; authentic parent
implementation and N3/restart/N1 execution remain outstanding.

EG-025 now creates both profile-target and existing-Agent-target consumers
through the actual channel form. It reuses creation intent custody and forwards
the original accepted response once, then fills and verifies model/effort,
permissions, authorized services, prompt, pinned Ornn skills, supported local MCP
settings, typed non-secret environment values and queue lifetime. Each channel
reloads through the opposite observed EG and its real Consumer inspector.
Independent provider bind capture and retained applied configuration must agree
with the exact owner, target, credential hash, profile and allowed service scope;
storing a refused/ignored field is insufficient. Both original intents remain
under public cleanup custody. The case uses the existing leased Chromium's
contexts, never another application/browser process. It declares `eg_only`,
with parent restart evidence still required. Six new offline tests cover
ordinary response delivery, unknown delivery preservation, fixture bounds,
actual/applied provider agreement and typed UI controls. See
`fixtures.json.consumer_profile_browser_contract`; runtime remains unverified.

EG-026 has a source-confirmed missing execution clause. The audit at author
head `d62c55af66c26c67cbd4b8e3c5b14c6a2c9957d8` and canonical product head
`01a0f5d84c74cd3ce7ec98df120e196e994342f2` found the following distinction:

- `RoutingFields.tsx` configures consumer publish targets, maximum handoff hops
  and per-consumer/per-channel event rates. `channels/routing_update.rs` retains
  these settings and `channel/routing_validation.rs` validates their ranges.
- The pure `cmaeg-core/src/routing/precedence.rs` resolver accepts a supplied
  published target/causation tuple, but does not enforce hop or event-rate limits.
  Every application `RouteInput` construction supplies `published: None`.
- In `cmaeg/src`, AgentEventPolicy's hop/rate fields are read only by console
  presentation/size accounting; `agent_events` is otherwise stored/projected.
  The REST routers and MCP tool selection have no agent-event publisher path.
  Ordinary ingress max-target enforcement does exist and is a different clause.

The old case sent routing data to `PUT /v1/channels/{id}` even though the real
policy endpoint is `PUT /v1/channels/{id}/routing`, accepted any event disposition,
and compared routing schema version 1 instead of the channel record revision.
Those checks could not establish the frozen row. EG-026 now acquires cleanup
custody and returns `routing_handoff_runtime_and_limits_missing` before feature
mutation. It cannot count as complete executable coverage or runtime PASS.
A product owner must supply the actual authenticated handoff publisher and
hop/rate enforcement, or resolve the frozen specification. The complete UI
policy/mentions/rules/defaults/labels/handoff/limits journey remains required;
ordinary-routing-only coverage is not a replacement. No fake parent publisher,
source-only check or HTTP-success fallback supplies the missing behavior.

EG-047 now exercises the real Consumer list and detail UI through AM routes.
It refreshes and traverses `next_after` pages, checks the complete owned identity
set and every rendered row, returns through Previous page and opens a row.
It refreshes active/provisioning/failed/legacy details through the opposite
observed AM and checks state, progress, explicit failure, last test and callback
metadata. It opens the actual connection picker and checks both available
connection API representations, owner isolation and the `manage_own_bots` gate
on list/detail/new pages plus the API. All reads join actual request IDs and
complete AM receipts; cookie sessions are required. Five offline regressions
cover insufficient fixture refusal, duplicate/foreign/cyclic pages, actual
receipt joining and direct Consumer entry without an unrelated EG navigation.
The parent must supply 51–150 real owned records, stable lifecycle states,
actual stored last-test/callback metadata, a foreign record and a separate
non-visitor cookie identity without the capability. Missing authentic setup
remains BLOCKED; no runtime PASS is claimed. See
`fixtures.json.consumer_inventory_browser_contract` and the named setup contract.

EG-048 now dispatches all four required steps. It fills the actual Consumer
wizard, verifies its saved intent before submitting, and forwards exactly one
real AM create request. The test then loses that accepted response at the browser
boundary, verifies the UI retains the same body/key, and polls the observed Bot
ID through the opposite **actual** serving AM. It checks exact profile revisions,
configured services/image/workspace, source admission and reply sinks, then opens
the linked EG channel in the UI. It joins actual list/pagination responses to
EG receipts and opens an addressed row directly; only missing addresses require
an additional recovery GET. It checks the selected channel identity/version and
policy without waiting for a nonexistent detail request. EG receipts cannot
inherit AM attribution. Parent setup now requires the
`wizard` inputs, `retainCreationIntent`, `creationRecords`, and an `eg_browser`
proxy binding with `egEvidence` and `networkIdle`. Missing setup stays BLOCKED. Setup supplies a
separate existing channel so the newly created channel is selected explicitly.
Eight new offline fault/receipt regressions plus an independent all79 step-dispatch
check cover these changes; they do not establish compiled-UI or runtime PASS.

EG-049 now uses the real Bot Father review/create/publish UI and Consumer
wizard. Before forwarding each actual profile POST it verifies the retained
owner, exact reviewed body and separate create/publish keys, then registers
cleanup custody with the parent. It reads the returned private document and
immutable revision through the opposite observed AM and resumes the publication
receipt after reload. The Consumer uses the product's inline configuration path;
its generated profile resolves the omitted model to the qualified deployment
default. The test retains the actual accepted Consumer request, changes the real
cookie login in the same browser context, verifies the other owner cannot resume
or read the first owner's resources, and restores the original exact receipt.
Independent stored allocation reads must show one profile/publication/Consumer.
Eight offline regressions cover custody, changed identity/configuration, duplicate
and premature POSTs, unknown transport, cancellation, cleanup joining and default
model provenance. The parent still must bind the actual login switch, local
connection, cleanup custody and stored allocation reads. These source changes do
not establish a runtime PASS; see `fixtures.json.bot_father_browser_contract`.

E1 delayed reply, definition edit and unknown-head release remains in the original
scope and now has a supplemental executable export, `e1Journey` in `e1.mjs`.
It stays outside the frozen 79/362 denominators. The case pins reviewed E1 source
`6f03d970cb82adc7d8a1b2346746fc8203422544` (PR #2191; architect03 source GO),
requires a parent-owned local release-enabled fixture, and never operates a
release controller or changes NyxID configuration. It exercises delayed reply
progress behind an unknown parent, same-intent pending polling, changed-body
refusal, definition edit with frozen original request retention, explicit release
authority, stale-proof refusal, a race through both EG replicas, exactly one head
advance, duplicate-operation readback, audit/tombstone retention, stale reply
refusal and successor progress. Provider-terminal evidence is required separately
from event expiry. Same-key pending polling follows E1's explicit HTTP contract;
unknown transport failures are never resubmitted.

The parent invokes this export through the shared `executeCase` using a separate
supplemental row. `fixtures.json.e1_supplement` names the low-level controls and
actual observations. Missing short-lived authentic event admission, real provider
fault/capture, scoped journal/partition/audit reads, explicit release authority or
source-bound topology blocks execution. Seven new offline regressions cover these
contracts (six E1 tests plus real local transport Retry-After retention). No E1
runtime run, release action, additional application or native lane was launched.
The exact-image two-EG/two-AM run, all remaining fixture bindings and the separate
2->4->2 qualification remain parent-owned and unverified.

All browser contexts and EG leases register cleanup on acquisition. Preferences
restore before context close. The EG lease must reconcile uncertain work, establish
no-late-effect finality and use exact public cleanup; cleanup failure prevents PASS.
No application process, customer bot, environment or persistent test resource was
created during authoring. The temporary npm cache was removed. The pinned ignored `multi-am-features/node_modules` dependency remains available for shared WebSocket tests. The lead owns
worktree cleanup after integration. Developer15 owns the consolidated PR and
develop merge; no competing PR or default-branch merge was created.

| Case | Implementation | Runtime prerequisite |
| --- | --- | --- |
| EG-001 | Authored; unverified | browser |
| EG-002 | Authored; unverified | browser, eg_browser |
| EG-003 | Authored; unverified | browser, eg_browser |
| EG-004 | Authored; unverified | browser |
| EG-005 | Authored; unverified | browser |
| EG-006 | Authored; unverified | browser |
| EG-007 | Authored; unverified | browser, eg_browser |
| EG-008 | Authored; unverified | browser, eg_browser |
| EG-009 | Authored; unverified | browser, eg_browser |
| EG-010 | Authored; unverified | browser, eg_browser |
| EG-011 | Authored; unverified | browser, eg_browser |
| EG-012 | Authored; unverified | browser, eg_browser |
| EG-013 | Authored; unverified | browser, eg_browser |
| EG-014 | Authored; unverified | browser, eg_browser |
| EG-015 | Authored; unverified | browser, eg_browser |
| EG-016 | Authored; unverified | browser, eg_browser |
| EG-017 | Authored; unverified | browser, eg_browser |
| EG-018 | Authored; unverified | browser, eg |
| EG-019 | Authored; unverified | authentic parent fixture setup required |
| EG-020 | Authored; unverified | authentic parent fixture setup required |
| EG-021 | Authored; unverified | authentic parent fixture setup required |
| EG-022 | Authored; unverified | browser, user-fixtures; implemented channel_edit setup |
| EG-023 | BLOCKED; incomplete | producer removal web workflow missing; full UI journey still required |
| EG-024 | Authored; unverified | authentic parent fixture setup required |
| EG-025 | Authored; unverified | authentic parent fixture setup required |
| EG-026 | BLOCKED; incomplete | product handoff execution and hop/rate enforcement missing |
| EG-027 | Authored; unverified | browser, eg |
| EG-028 | Authored; unverified | authentic parent fixture setup required |
| EG-029 | Authored; unverified | authentic parent fixture setup required |
| EG-030 | Authored; unverified | authentic parent fixture setup required |
| EG-031 | Authored; unverified | browser, eg |
| EG-032 | Authored; unverified | eg |
| EG-033 | Authored; unverified | eg |
| EG-034 | Authored; unverified | eg |
| EG-035 | Authored; unverified | eg |
| EG-036 | Authored; unverified | eg |
| EG-037 | Authored; unverified | eg |
| EG-038 | Authored; unverified | eg |
| EG-039 | Authored; unverified | eg |
| EG-040 | Authored; unverified | eg |
| EG-041 | Authored; unverified | eg |
| EG-042 | Authored; unverified | eg |
| EG-043 | Authored; unverified | eg |
| EG-044 | Authored; unverified | authentic parent fixture setup required |
| EG-045 | Authored; unverified | authentic parent fixture setup required |
| EG-046 | Authored; unverified | authentic parent fixture setup required |
| EG-047 | Authored; unverified | authentic parent fixture setup required |
| EG-048 | Authored; unverified | authentic parent fixture setup required |
| EG-049 | Authored; unverified | authentic parent fixture setup required |
| EG-050 | Authored; unverified | browser, eg |
| EG-051 | Authored; unverified | browser, eg |
| EG-052 | Authored; unverified | browser, eg |
| EG-053 | Authored; unverified | browser, eg |
| EG-054 | Authored; unverified | browser, eg |
| EG-055 | Authored; unverified | authentic parent fixture setup required |
| EG-056 | Authored; unverified | eg |
| EG-057 | Authored; unverified | eg |
| EG-058 | Authored; unverified | eg |
| EG-059 | Authored; unverified | eg |
| EG-060 | Authored; unverified | eg |
| EG-061 | Authored; unverified | eg |
| EG-062 | Authored; unverified | eg |
| EG-063 | Authored; unverified | eg |
| EG-064 | Authored; unverified | eg |
| EG-065 | Authored; unverified | eg |
| EG-066 | Authored; unverified | eg |
| EG-067 | Authored; unverified | eg |
| EG-068 | Authored; unverified | eg |
| EG-069 | Authored; unverified | eg |
| EG-070 | Authored; unverified | authentic parent fixture setup required |
| EG-071 | Authored; unverified | authentic parent fixture setup required |
| EG-072 | Authored; unverified | authentic parent fixture setup required |
| EG-073 | Authored; unverified | eg |
| EG-074 | Authored; unverified | eg |
| EG-075 | Authored; unverified | eg |
| EG-076 | Authored; unverified | eg |
| EG-077 | Authored; unverified | eg, eg_accounts |
| EG-078 | Authored; unverified | eg, eg_accounts |
| EG-079 | Authored; unverified | eg |

# EG executable cases — author handoff

Part of #2149. Integration owner: developer-15. Author branch:
`tl/cma-13b/coverage-prod-verifier-03`. The frozen assignment remains all 79
rows, EG-001 through EG-079. All 79 rows remain registered. EG-023, EG-026 and EG-027
are explicitly BLOCKED pending missing product workflows. The other 76, including
EG-043, retain authored actions, subject to behavioral auditing and runtime
verification. EG-043 now includes separate native intake/documents and selected
relay resource/message-effect journeys; its authentic parent fixture is unverified.
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

Focused verification (also included in the full serial suite):

```sh
node --test .github/scripts/multi-am-features/executable/cases/prod-verifier-03/*.test.mjs .github/scripts/multi-am-features/executable/runner.test.mjs .github/scripts/multi-am-features/executable/serving.test.mjs .github/scripts/multi-am-features/executable/eg-evidence.test.mjs
```

Latest full executable verification: 503 tests passed, zero failed/skipped with
`node --test --test-concurrency=1` over all 75 executable and author test files.
The earlier 308-test parallel run passed 307/308: the unchanged shared action-timing test
exceeded its 10 ms synthetic threshold under scheduling contention. That test
passed in isolation and in the complete serial run; no timing gate was changed. The focused suite covers frozen IDs,
exact steps, missing prerequisites, wrong event/delivery joins, duplicated effects,
unsafe-fixture cleanup, cursor cycles, wrong EG replica attribution and shared
browser evidence custody. These tests do not execute the product. The parent
owns N=3 before restart, N=1 after, exact image binding, the sole controlled AM
restart and HPA run. Browser-only and EG-only witnesses retain their true scope. AM-backed EG turns now require actual `amRequests` with joined owner proofs; `retainTurn` publishes `multi-am-eg-evidence-v1` for shared-runner ingestion. Requested AM labels never satisfy that evidence.

The common poller now checks cancellation, its own deadline and the remaining
case budget before each fixture read and before accepting its result. Cancellation
interrupts the interval between reads, while an in-flight read still joins before
cleanup. The parent remains responsible for that read's transport timeout; the
case never races away from unresolved work. Eight offline regressions cover
expired budgets, cancellation before/during reads and intervals, late results,
cleanup ordering, and preservation of timely results and original read failures.
The old helper reproduced reads and successful returns after cancellation/expiry.

The supplemental E1 delayed-reply poller also checks cancellation and the case
budget after retaining each response. A confirmed reply received after the budget
ends cannot advance to replay checks or later mutations. It still joins the
original request and retains its acknowledgement for reconciliation. Two offline
regressions reproduce late acceptance after cancellation and budget exhaustion;
a positive control preserves timely confirmed replies. These are case-control
tests and do not establish product or two-AM runtime acceptance.

The common EG request helper also checks cancellation and the remaining case
budget before dispatching through a lease, which bypasses the AM context request
guard. An already-started request still joins and retains its original response;
the next request, including a subsequent POST or pagination read in the same
step, cannot start after expiry. The parent retains transport-timeout custody.
Five negative regressions reproduce dispatch after cancellation/expiry, and a
positive control preserves timely results and original transport failures for
contexts without budget hooks. No product runtime acceptance follows from these
offline tests.

An independent source-pinned audit at this head reads the authoritative
`CHECKLIST.json` (275 rows) and the executable catalog (362 rows): all 79
assigned EG IDs are present exactly once, every definition's `required_steps`
matches its frozen catalog row, and the supplemental `E1:EG-2171` export remains
outside the 79-row denominator. This audit checks registration and step identity;
it does not claim product or two-AM runtime acceptance.

The 34 parent-bound cases declare `user-fixtures` and use the real registry dispatcher;
an unavailable parent factory blocks before feature execution. EG-023 requests
`producer_edit`, separate from EG-022's one-source `channel_edit` setup. Cleanup
aliases reuse the same promise, retain the lease receiver and preserve rejection.
The HTTP helper forwards expected statuses and supports empty 204/404 bodies.
Regression tests exercise the dispatcher and local HTTP transport, not a product
campaign.

EG-004 now binds the real public documentation routes as well as the compiled
Account -> Docs -> Event Gateway Protocol journey. The existing parent browser
must route `/event-gateway/docs/*` and `/event-gateway/openapi.json` to the
selected EG, preserving original content, headers and request IDs. Its actual
local advertised API base and advertisement mode are prerequisites; production
origins and mutating requests are refused before dispatch. Page fetches retain
EG process receipts and join their source to the compiled Web artifact.

Both EGs must serve identical canonical Markdown and the actual gateway OpenAPI
schema. The compiled view and each EG's native HTML view must render every
source heading, every source-declared diagram, unique heading targets and
working section/internal links. The current source contains 12 reviewed diagrams;
a zero-image loop cannot pass. Each offered download must preserve all three
complete documents and every rendered heading anchor, allowing only known
public-URL, annotation and Markdown-link rewriting; native server downloads may
include their final separator. Download custody is registered immediately and
all owned files are deleted even on failure. The case returns through the real
service selector. Eight offline regressions cover content loss, missing diagrams,
invalid links/schema, local routing, response/source attribution and cleanup.
The public browser binding in `fixtures.json.documentation_contract` remains
parent-owned; these checks do not establish authentic UI or two-EG runtime PASS.

EG-006 now measures rendered text at all three offered sizes across eight
Account-menu, EG-navigation and Usage-dialog samples. Each sample must scale
against the measured Medium baseline by the actual 0.9/1/1.15 preference, remain
inside the viewport and clipping ancestors, and pass hit testing. Trial clicks
check menu, navigation, Usage filter and close controls without submitting a
report or signing out. Saved values and CSS variables alone cannot pass when
the compiled font-scaling plugin is absent or a surface stays unscaled.

The original preference is restored through the UI, including after a failure
with the retained Usage dialog still open. Reload must retain both the selected
preference and its computed font size. Evidence contains numeric font/geometry
measurements and screenshot hashes, with no text content. Five offline
regressions reject unchanged/compounded/partial scaling, clipping, obscured or
missing text and incomplete modal cleanup. This uses the existing parent browser
lease; authentic rendered measurements remain unverified until its execution.

EG-035 now checks exact signed callback behavior through both actual EG replicas.
Invalid signatures must return `401 callback_invalid/token_signature`; a correctly
signed wrong route must return `401 callback_invalid/route_denied`. Denied senders
and unmentioned groups must return their exact `202 refused` reason. A generic
rejected/refused/ignored flag cannot satisfy these distinct protocol outcomes.

The named relay setup requires a clean isolated channel with verified raw
Telegram facts, one permitted sender, bounded chats/routes, mention-only groups,
and an actual CMA provider binding. Independent ingress captures join real request
IDs to transmitted body/signed-header/token digests and parsed wire facts. An
explicit duplicate reaches the other real EG while provider completion is held;
two further duplicates replay the same retained bytes after delivery. No callback
is re-signed or retried following unknown transport. Counters prove one original
turn/effect, no work for all eight negative callbacks, two final answers, and no
pending/unknown/late work. Both EGs read back the two exact completed Activity
records with their original source, sender, chat, consumer, Agent and response.
The parent must bind actual capture, signer/replay custody, completion holds and
independent work/delivery counters under `fixtures.json.relay_verification_contract`.
The fixture setup is exported and cleanup is registered before validation. Eight
offline regressions cover prerequisite cleanup, policy bounds, exact protocol
outcomes, wire/capture attribution, changed replay bytes, duplicate work, retained
identity and no replay after unknown transport. No runtime acceptance is claimed
from the offline regression suite.

EG-032/033/034/036/037/038 now inspect the actual saved relay source, callback URL
and current verification capabilities through both EGs. Fixture-reported
configuration is insufficient. The chosen mode must match the saved flags and
remain enabled; the final source/capability readback must match the original,
including unavailable modes. Signed callback captures and actual EG receipts
join exact input facts. Normalized trust explicitly retains unknown sender-kind
and mention evidence instead of being labeled raw verification.

Each turn verifies the actual fake-model input assembled by its AM, a
case-chosen numbered answer and independent platform delivery. The three
contextual rows run three sequential first/follow-up pairs in distinct private
conversations. A follow-up contains no original token, so its provider history
must supply the prior marker; markers from another conversation fail. Every pair
retains its Agent/conversation, each event/response is distinct, and observed
AM/EG receipts establish both ingresses. The other three rows execute one full
turn. Work/effect/delivery counters and finality are checked after each answer.

All six named parent setups now export concrete validation and cleanup custody.
The exact low-level requirements are in `fixtures.json.relay_journey_contract`.
Six offline regressions cover frozen steps, setup cleanup, source/trust modes,
normalized evidence, real-context requirements and complete pair attribution.
EG-032/033/034 still require the shared independent actual-platform proof; a
synthetic platform result cannot satisfy that separate acceptance gate. These
source changes do not establish a runtime PASS or authorize an external run.

EG-039 now verifies direct Telegram webhook activation from captured getMe,
setWebhook and getWebhookInfo calls, including the exact bot, native URL,
secret digest, update selection and retained pending-update policy. Both EGs
must return the same actual scoped native source. A stored credential or generic
activation flag cannot establish registration.

The two admitted private/group updates retain exact raw native captures and
real EG/AM receipts. Each EG also receives an invalid secret, denied sender and
unmentioned group; exact authentication/refusal responses must leave the work
and delivery counts unchanged. The actual provider input and selected answer
join one native platform send per admitted event. Both EGs read back the exact
completed native Activity records and unchanged source.

The original-message association is checked in the actual captured, verified
sealed event reference, joined to the AM request, bot, chat and message ID.
Current ordinary native Telegram delivery does not set a quoted reply-to field;
the case rejects a fixture that invents one. This follows the native ingress and
send_message implementation, not a relaxation of the original-message clause.
The parent must supply nativeEventCapture from its existing synthetic-key
custody and real provider traffic. No production Secret/key access is authorized.
Eight offline regressions exercise source/admission, registration, wire facts,
original-reference attribution, platform delivery, retained Activity and cleanup
without replay after unknown transport. The exact setup/capture contract is in
fixtures.json.native_telegram_contract; authentic execution remains pending.

EG-041 now distinguishes native WhatsApp authentication, admission and ignored
foreign envelopes using the actual HTTP contract. Invalid verification tokens
and signatures return typed 401 not_authenticated responses. A correctly signed
foreign phone or WABA returns 200 with zero accepted and refused messages;
a denied sender returns 200 with one refused message. Those outcomes must leave
the independently observed AM work and platform effect counts unchanged.

Both EGs receive fresh valid/invalid challenges and all four negative callback
variants. Actual source readback and captured Graph custody reads establish the
app/account/phone relationship. Native required default-consumer routing remains
present. Independent signed-wire and provider captures join one private message,
its real AM request, verified original reply address and model input. The Graph
send must use the exact phone, individual recipient, original message context
and case-selected answer. WhatsApp's unknown actor kind must remain unknown.

The saved acknowledgement delay is zero; exact HTTP webhook acknowledgement is
checked separately, and any unsolicited ack/read/typing or additional send fails.
Both EGs read back the completed native Activity and unchanged configuration.
Ten offline regressions cover missing prerequisites, custody, source/routing,
challenge echo/status, signed facts, all four protocol outcomes, provider identity,
actual delivery, retained Activity and no replay after unknown transport. The
parent requirements are in fixtures.json.native_whatsapp_contract. No actual
WhatsApp operation or product-runtime PASS is claimed from these checks.

EG-040 now requires actual native Discord discovery, IDENTIFY/READY/heartbeat,
source selection and capacity, exact raw Gateway dispatches and independent AM,
model and quoted-platform-answer captures. Both EGs read the configuration and
retained Activity; one EG owns the Gateway socket and the peer may be standby.
Shared inbox dispatch may finish on either EG, independently of socket ownership.
A received-sequence heartbeat proves socket receipt only; an independent sealed
journal read through the parent-owned local record API must prove durable commit.
No HTTP receipt or AM ownership is invented for a WebSocket frame.

The compiled producer inspector is read through both EGs and must show the actual
saved activation status, Message Content Intent requirement, checked DM/server
and reaction controls, and explicit unsupported bot-message notice. It must not
offer a bot-message control. The existing parent browser/EG proxy binds the same
source/image; unexpected editor writes fail. No new browser process is started.

The case sends a DM, a mentioned server message, and an allowed human reaction
on the retained server message. The reacting human differs from the original
sender, but the original Agent/conversation must remain. Reaction evidence must
be a delta with no fabricated producer timestamp or full old/new reaction sets.
Native bot input must produce a durable refusal and no fourth turn. A saved but
unselected control channel remains empty with no socket throughout. Unknown
message reactions are not assumed immediately refused: source retains a pending
inbox for up to 60 seconds. That old author-added probe has been removed; the
frozen retained-message reaction remains fully required.

Eleven offline regressions cover transport/setup custody, native source and intents,
selection-versus-readiness, process/session attribution, discovery, exact frames
and independent durable checkpoint, reaction partition/provenance, quoted delivery,
Activity, truthful producer UI, unknown dispatch and complete case orchestration. Current runtime still needs an
admitted local binding for the native fixed Discord Gateway/REST origins; the
internal Rust test injection is not a supported parent binary binding. Missing
binding is BLOCKED before dispatch, with no real Discord contact or DNS/TLS
changes. Exact parent primitives and captures are in
fixtures.json.native_discord_contract. These tests do not prove product runtime.

EG-043's native custom-app
source uses its own regional app credentials and supports neither lark_resources
nor lark_message_effects bindings. Those settings belong to NyxID relay sources;
selection metadata alone also does not grant the channel key access. The old
resourceOperation/result flags and resourceEffects count could claim success
without the public attachment/message APIs, actual provider calls or compiled
resource-selector UI. They have been removed and cannot yield whole-row PASS.
The replacement covers native encrypted intake separately from selected
relay resource reads/effects, with actual connection/scope refusals and disabled
native controls/forms checked in the compiled UI. All original EG-043 steps and
expected-result clauses remain required and have authored actions. They still
require execution against the admitted product and real parent fixtures.

The exported createNativeLarkWebhook(parent, configuration, record) now supplies
an actual callback sender for the EG-043 replacement. It uses the existing local
EG transport, fresh AES-CBC encrypted envelopes, exact-byte signatures and real
EG serving receipts. Encrypted URL verification deliberately has no signature
headers. No owner bearer is sent on this platform callback. The parent supplies
only already-owned synthetic Encrypt Key/Verification Token and the existing
local topology; the helper starts no application or platform listener.

It supports actual invalid-signature/token/app/tenant/time probes, cancellation
and joined sender shutdown, with no replay after unknown responses. Retained
proof is explicitly sender provenance; independent ingress/provider/platform
capture remains required. Eight offline regressions independently decrypt and
verify actual bytes received at two loopback test servers. They do not execute
the Rust native handler. The exact exported interface and remaining parent
responsibilities are in fixtures.json.native_lark_webhook_primitive. This sender
alone does not satisfy EG-043; the separate resource journeys and compiled UI
actions remain mandatory parts of its execution.

EG-043 step_1 now binds the named native_lark_resources setup and executes actual
public capability, channel and resource-connection reads on both EGs. The seed
contains separate native-app and relay channels for each of Lark and Feishu,
with distinct app/bot custody and a selected plus unselected connection in each
region. Source assertions reject widened sender/chat policy, native/relay field
conflation and changed saved definitions. Exact source/image EG receipts remain
separate from AM ownership evidence.

The compiled UI opens all four saved producers through each EG, inspects native
identity/region/separate-webhook guidance and disabled controls/forms notices,
checks the actual native group-policy choices, and loads both real relay
connection selectors. It verifies the selected connection, unselected entry,
app ID and deletion/reaction consent without saving. A write guard retains any
unexpected UI mutation failure. Actual capability/source documents and zero
scoped work counters are checked again afterward. No browser is launched by
the case; the existing parent's compiled browser and real EG proxy are required.

Nine source-inspection regressions cover regional custody separation, source and
inventory corruption, real loopback GETs through both transports, mismatched
serving receipts, source divergence, and cleanup failure retention. They do not
execute the compiled UI or product processes.

EG-043 step_2 executes `runLarkNativeIntake` before its native-document and selected
relay journeys. The intake sends fresh encrypted challenges and text callbacks through
each EG for both Lark and Feishu. Each accepted callback must join an independent
receiver capture, a sealed original event reference, actual non-owner AM request
receipts, provider input, public consumer context and one exact native regional
text delivery. The send's own app-token/bot-info capture must prove its credential
and bot custody. Native event references must refuse the separate relay consumer.
Invalid signatures/app/tenant, bot senders and denied senders exercise actual
authentication, unsupported-event and admission-refusal responses on both EGs.

The four accepted turns must preserve each regional conversation and Agent while
remaining isolated across regions. Final checks require exactly four platform
effects, completed native Activity, empty relay Activity, unchanged saved sources
and zero pending, unknown or late work across all four owned channels. Sender
shutdown is registered before dispatch; parent cleanup retains admission and
provider/platform reconciliation responsibility after cancellation or uncertainty.

Nine native-intake regressions check custody, sealed-event joins, public context,
delivery, Activity and actual encrypted requests received and independently
decrypted at two loopback servers. Those receivers are protocol test doubles;
neither the Rust handler nor the full intake orchestration has run against a
product environment. The complete fixture/capture contract is in
`fixtures.json.native_lark_intake_contract`. A supported parent binary binding
for the fixed regional native platform origins has not been established; Rust
transport injection alone does not prove the admitted image exposes that binding.
Missing binding stays BLOCKED without a second application or network workaround.

Prepared-target source audit (2026-11-17): product source
`cd91b713d74bb251506d094c3c20d87a329b4791` lacks the unsigned encrypted
URL-verification behavior required by this native journey. Its HTTP handler
requires all three signature headers before reading the body and returns HTTP 401
when they are absent. Parent source `78c61073f9` includes the challenge fix
`f92945090ae04e359c28af93a015b68f3dbf5f23` and the public-router regressions;
the prepared source has neither that behavior nor the router test file. This
is a source-confirmed incompatibility, not an observed image-run result. The
integration owner must provide an actual build with equivalent correct challenge
handling. The case continues to omit signature headers for encrypted challenges;
adding them would conceal the unsupported platform URL-verification path.

The native document journey now follows intake in step_2. It submits one fresh
authenticated file event per region, then reads its exact retained attachment
and full UTF-8 text through each EG's public context/attachment APIs. The source
revision, original message/key and sealed event must agree; the provider receives
the filename without a resource locator. Captured native resource GETs must use
the exact regional app and new token/bot custody, return the actual staged bytes,
and contain no relay connection authority. Both regions retain their existing
native conversation and Agent for the document response.

The same public attachment must refuse the separate relay consumer and the
other region's native consumer on both EGs. An absent attachment slot returns
typed404; page 2 of the one-page document returns the exact unavailable/not_found
view. These refusals must leave resource-call captures unchanged. Pending text
views alone permit bounded polling of the same GET; unavailable content, transport
uncertainty, bad receipts and cancellation cannot trigger another event.
Finality covers all six native answers, both document resources, empty relay
Activity, unchanged sources, zero unknown work and joined media jobs.

Eight document regressions cover exact file/context/page semantics, regional
resource custody, missing media transport, real loopback GETs, pending results,
cancellation and corrupt evidence. They do not execute the Rust media pipeline
or the complete document orchestration. NativeLarkFileSource has a separate
pinned media transport: a parent binding for the native token/send client alone
cannot satisfy it. The case checks the concrete
`fixtures.json.native_lark_document_contract` prerequisites before any intake.

The selected-connection relay journey now follows the six native turns. It sends
one signed rich-post callback per region through alternate EGs, with a fresh
embedded PNG and text marker. The normalized relay content stays `unknown`; exact
raw Lark facts, the sealed original attachment and actual AM/provider input must
join independently. Both EGs read the selected image through the public API.
Every resource call must use the selected connection's current metadata and
proxied bot identity, channel-key custody and exact original message/key/bytes.
This fixture uses directly listed endpoint metadata; it does not claim coverage
of the optional shared-endpoint authorization lookup.

After the automatic answer, a real public reply supplies a confirmed message
reference. Its sealed relay outbound ID joins the broker's outbound ID; its
separate native `om_` ID joins the platform delivery and public receipt. The
case reads capabilities through both EGs, sets and clears the bot's THUMBSUP
reaction, then deletes that message. Each confirmed intent is deliberately
replayed with the same key through the peer EG and must cause no further
platform effect. Unknown outcomes stop execution without another mutation.

Actual consumer refusals and rejected caller-supplied connection fields keep
native and relay authority distinct. A versioned owner update removes both
resource and message-effect consent; both EGs must then refuse the warmed image
and retained message without another resource read or mutation. The case restores
the exact original sources, preserving credentials/routing and checking both
public version fields. Parent cleanup retains failure/cancellation custody.

`expected_result` now re-reads completed Activity, restored sources/capabilities
and independent resource/effect inventories. It requires eight distinct responses
across four isolated native/relay Agents and conversations, ten deliveries, six
message effects, sixteen total effects and zero pending/unknown/late/media work.
All sixteen platform request IDs must join exact captures; actual AM receipts
must cover both ingresses. No fixture verdict can replace the executed journeys.
The exact additional parent methods and capture fields are documented in
`fixtures.json.lark_relay_resource_contract`.

Fifteen relay regressions include the complete relay orchestration through two
local HTTP protocol doubles, wrong selected scope/custody/IDs, confirmed replay,
unknown-mutation stop, cancellation, consent restoration and fresh finality after
step_2. These use synthetic AM/provider/seal/platform facts and do not execute
Rust or the compiled UI. Native-plus-relay product execution still requires the
parent's supported native client and pinned media bindings, existing local fake
NyxID broker, authentic captures, browser, and sole two-EG/two-AM campaign. No
binding workaround, new process or runtime PASS is implied by this source work.

EG-018 now validates the complete owned record set across the real 50-record
page boundary. The named parent setup requires 51..100 isolated owned records,
a versioned addressed channel, a retained record without an address, and a
foreign-owner sentinel. Each actual page must preserve known record identity,
owner, version, timestamp and redacted-definition digest, without duplicates,
foreign rows, cursor cycles or truncation. The DOM must render exactly those
records with their stored creator/timestamp metadata. Refresh through the other
observed EG resets the visible list before loading its second page.

The case opens the selected channel's actual producer/consumer nodes, compares
saved policy and binding facts, then opens the missing-address record and its
expanded JSON without inventing an address or topology. Opaque parent scanners
compare actual API bodies, page text and input values to runtime fixture
credentials; raw values never enter evidence. A write guard is installed on a
blank page before real navigation, and independent provider/platform counters
must remain unchanged. Both EGs read back unchanged selected/legacy records.
This inventory declares EG-only scope, with no fabricated AM ownership. Eight
new offline regressions cover exact inventory, ownership, metadata, address,
effect and setup-cleanup boundaries. The parent requirements are in
`fixtures.json.channel_inventory_contract`; authentic UI execution is still
unverified.

EG-027 now selects the actual saved source index and a distinct non-default
consumer, verifies configuration-only diagnostics and rendered check details,
and reads each node's independently specified first-text result. Graph selection
closes the retained inspector modal first, uses the complete accessible node
name, and opens the Tests section through its actual navigation control. Per-action
provider/platform counters reject a source test that starts work even if a later
test omits work. A browser guard permits exactly one bodyless POST per UI action,
disables retries and redirects, rejects unrelated writes and duplicate attempts,
and joins each accepted response to its real EG serving receipt. Cancellation
rejects the waiting action while cleanup joins the intercepted request and
releases its response.

The accepted-response-loss scenario drops only the downstream response after
retaining a real 200. Actual diagnostic captures must join the selected consumer,
binding, Agent, conversation and response to a successful non-owner AM admission
with complete serving/owner receipts. `multi-am-eg-evidence-v1` uses
`event_kind: node_test` and `event_identity: eg_request_id`: its initiating-event
hash identifies the real diagnostic HTTP request, never an invented platform
event. The case inspects a fresh native Activity stream on the other observed EG
without resubmission. It requires the same response/consumer/Agent/conversation,
completion and no external delivery before accepting reconciliation.

**EG-027 remains incomplete:** `test_turn.rs` at parent head `78c61073f9`
invokes the provider with `event_context: None` and does not persist a diagnostic Activity event. The
prior `reconcileTest` fixture verdict could conceal that gap. Missing Activity
identity now raises `node_test_unknown_activity_reconciliation_missing` after
recording actual UI/HTTP evidence, finality, effect counts and unchanged saved
configuration. Product persistence/reconciliation and authentic parent execution
are required; the case does not invent a durable activity result. The parent must
supply the real EG browser/cookie/CDP binding, an owned Custom-source channel with
a distinct additional consumer, `nodeTestCaptures`, scoped `counters`/`quiesce` and
`drain` as detailed in `fixtures.json.channel_tests_contract`. Fourteen new offline
regressions exercise these evidence boundaries and guard failures; they are not
a product or two-AM runtime PASS.

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

EG-044 now executes the managed-connection journey through the compiled Event
Gateway UI. The owner connection picker is loaded through the real EG browser
proxy and follows its `next_after` cursor; the owner-scoped list is checked for
ready, unbound rows without token-bearing URLs. The case configures the full
channel form, chooses the exact saved connection through the custom Picker,
sets the connection admission controls, and clicks the real Create channel
button. A one-shot route guard validates the UI-generated record and
idempotency UUIDs, exact profile/provider/admission configuration, empty
pre-expansion sources and provider-key hash, retains cleanup custody before one
forward, and joins the accepted response to the actual EG receipt. Peer EG
readback and the opposite browser inventory use the observed channel ID and
observed replica, while allocation counters prove one record/channel/binding
and no provider or unresolved work. Distinct bound/not-ready refs return the
backend's HTTP 400 `invalid_request` details; a foreign owner or NyxID route
reference is refused with 404. EG-044 declares `evidence_scope: eg_only`;
these receipts do not become AM owner proofs. See
`fixtures.json.managed_connection_browser_contract`; authentic fixture setup,
two-EG runtime and parent AM restart evidence remain outstanding.

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

EG-028 now reads the actual retained Activity inspector and native EventSource.
It selects the target from real channel-list responses, captures only the exact
same-origin SSE request/response/frame metadata through a CDP session attached
to the existing parent Chromium page, and checks every complete snapshot against
the rendered rows and consumer details. A held admitted event is released once
through the parent provider; its accepted, produced, completed and confirmed
delivery fields are joined to the retained event identity. The browser is taken
offline briefly to force native Last-Event-ID reconnect through the other observed
EG process, then one real stream request has its valid cursor changed to a
retention-expired cursor derived from the server cursor; the resulting server
`reset` frame and complete UI snapshot are required. No EventSource, response,
DOM, clock or transport body is stubbed, and unrelated transcript markers are
checked only for absence from Activity data. DOM snapshots reject duplicate,
partial, mixed or regressed rows; every stream has an EG request receipt and
separate EG process attribution. The named fixture must seed retained rows and
the held event, provide independent provider transcript capture and exact
completion finality, and expose `request`, `releaseCompletion`, `drain` and
`providerContext`; old activity result delegates are rejected. This is an
`eg_only` row and remains unverified until the parent supplies the authentic
two-EG browser lease and runs it.

EG-029 now prepares outbound text/media through the real Channel inspector. It
selects the saved destination from the actual channel row, enters the fixture's
text and up to four enabled public HTTPS attachments through compiled controls,
and guards the single Review message POST for exact source, route, body and
idempotency key before one real `route.fetch`. The returned 201 and EG serving
receipt are joined to the durable prepared receipt, which is read on the peer EG
and checked for matching expiry/status/identity. Platform capture must show no
delivery or confirmed recipient; the prepared operation remains cancellable under
cleanup custody. Direct `ctx.request` preparation, fixture result delegates,
predicted receipts, changed media or duplicate browser POSTs cannot pass. The
named setup must provide a saved UUID route, local disposable channel, exact
message/media body and supported cleanup; the row is `eg_only` and remains
unverified pending the parent browser/EG binding and controlled run.

EG-030 now sends and cancels two separate prepared receipts through the real
composer in the existing browser context. The fixture seeds only the product's
credential-free outbound journal for each already-prepared operation; the case
selects the actual channel, clicks Send message once, reloads, then clicks
Cancel once for the separate operation. One-shot browser guards validate exact
body, delivery identity and route before forwarding each mutation, and each
response is joined to its actual EG process receipt. A third prepared receipt
uses the named local fault control to lose one accepted send acknowledgement;
status readback must show one confirmed effect with the original key/body and
no resend. Peer EG reads, effect capture, drain and cleanup retain committed,
cancelled and uncertain outcomes separately. Direct prepare/send API calls,
fixture-result replies, inferred delivery or any duplicate request cannot pass.
The row remains `eg_only` and unverified until the parent supplies authentic
prepared receipts, browser journal/session binding, EG routing and cleanup.

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
source-bound topology blocks execution. Offline regressions cover these contracts
(six E1 command/receipt tests, four release-race lifecycle tests, and real local
transport Retry-After retention). No E1
runtime run, release action, additional application or native lane was launched.
The exact-image two-EG/two-AM run, all remaining fixture bindings and the separate
2->4->2 qualification remain parent-owned and unverified.

The two concurrent release POSTs now join both original outcomes before returning
control to fixture cleanup. Previously, one transport rejection let `Promise.all`
return while the other release could still commit. A regression reproduced that
premature cleanup through the shared runner. The case now preserves both failures
and any acknowledged sibling response in the private primary-failure record,
without retrying either request. Tests also verify that a separate cleanup failure
does not replace the release failure and that normal winner/loser responses keep
their actual replica order. These are protocol-double lifecycle checks, not an
executed product release or runtime PASS.

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
| EG-004 | Authored; unverified | real EG public docs/OpenAPI browser binding, local advertised base and source-matched receipts |
| EG-005 | Authored; unverified | browser |
| EG-006 | Authored; unverified | browser; measured fonts, unclipped text and reachable controls across Account, EG and Usage |
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
| EG-018 | Authored; unverified | named channel_inventory seed, opaque credential scanners and real EG browser; EG-only receipts |
| EG-019 | Authored; unverified | authentic parent fixture setup required |
| EG-020 | Authored; unverified | authentic parent fixture setup required |
| EG-021 | Authored; unverified | authentic parent fixture setup required |
| EG-022 | Authored; unverified | browser, user-fixtures, eg_browser; EG-only browser PUT receipt and channel_edit setup |
| EG-023 | BLOCKED; incomplete | producer removal web workflow missing; full UI journey still required |
| EG-024 | Authored; unverified | authentic parent fixture setup required |
| EG-025 | Authored; unverified | authentic parent fixture setup required |
| EG-026 | BLOCKED; incomplete | product handoff execution and hop/rate enforcement missing |
| EG-027 | BLOCKED; incomplete | diagnostic Activity persistence/reconciliation missing; real EG browser and joined nodeTestCaptures required |
| EG-028 | Authored; unverified | authentic retained/held activity fixture, parent browser CDP and two-EG SSE routing required |
| EG-029 | Authored; unverified | authentic saved-destination/media fixture, parent browser composer and EG routing required |
| EG-030 | Authored; unverified | authentic prepared receipts, parent browser composer/fault controls and EG routing required |
| EG-031 | Authored; unverified | browser, user-fixtures, eg_browser; EG-only browser DELETE receipt and named channel_delete setup |
| EG-032 | Authored; unverified | named relay_telegram; actual wire/model/delivery captures plus separate actual-platform proof |
| EG-033 | Authored; unverified | named relay_discord; actual wire/model/delivery captures plus separate actual-platform proof |
| EG-034 | Authored; unverified | named relay_whatsapp; actual wire/model/delivery captures plus separate actual-platform proof |
| EG-035 | Authored; unverified | named relay_verification setup; actual signed-wire captures, completion hold, AM receipts and platform counters |
| EG-036 | Authored; unverified | named relay_lark; actual source, wire, model and delivery captures |
| EG-037 | Authored; unverified | named relay_feishu; actual source, wire, model and delivery captures |
| EG-038 | Authored; unverified | named relay_slack; explicit normalized trust, actual model/delivery captures |
| EG-039 | Authored; unverified | named native_telegram; actual registration/wire/model/delivery captures and verified original event reference |
| EG-040 | Authored; runtime BLOCKED | named native_discord; real compiled EG browser, supported local transport and actual Gateway/journal/model/REST captures required |
| EG-041 | Authored; unverified | named native_whatsapp; actual Meta custody, challenge/signed-wire/provider/Graph captures and finality |
| EG-042 | Authored; unverified | eg |
| EG-043 | authored; runtime unverified | both-region source/UI, native intake/documents, selected relay image and message effects, consent revocation, restored configuration and joined finality; authentic parent bindings/runtime required |
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

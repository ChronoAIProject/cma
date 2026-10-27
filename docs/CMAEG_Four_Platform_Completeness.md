# Four-platform messaging status

Reviewed 2026-10-27 against the integrated CMA source, including native Discord
self-service, edits, verified threads, message effects and component controls,
and the default-off CMA native Lark/Feishu forms implementation.
This document replaces the historical transport-only descriptions; it does not
claim complete four-platform support or that every implemented feature is live.

“Implemented” means a composed CMA code path with local contract coverage.
Feature gates, source credentials, provider permissions and external verification
are separate requirements. The NyxID contract was inspected read-only at
`2ab239cecbda8e3fb9f69ae4fa84fd7a801d7e4f`; that pin is not proof that every
currently serving NyxID replica has the same behavior. No NyxID code change or
external platform send is authorized by this review.

## Current capabilities

| Capability | Telegram | WhatsApp | Lark / Feishu | Discord |
| --- | --- | --- | --- | --- |
| Ordinary ingress | Native webhook and authenticated relay; private/group admission | Raw-verified relay for private conversations | Verified relay private/group text and rich-post text | Native authenticated Gateway, owned supervision, durable inbox and routed execution; bounded source discovery and self-service creation. Relay slash commands are also supported |
| Inbound media | Source-bound image/file/audio/video retrieval, voice/sticker facts and album identity | Source-bound image/file/audio/video retrieval; voice/sticker, contact/location and quote facts | Standalone image/file/audio/video retrieval; default-off exact-source proxy binding can retrieve embedded rich-post images/videos, otherwise honest placeholders | Native exact-message metadata and attachment-bound official CDN retrieval; correlated relay media |
| Outbound media | Native Bot API image/file/audio/video uploads; gated relay uploads | Gated relay image/file/audio/video uploads | Gated relay image/file/audio/video uploads | Native multipart REST uploads; gated relay uploads |
| Existing threads | Native topic identity remains bound to the reply | Quote/context correlation; no CMA native-thread model | Declared thread contract with retained thread facts and reply/follow paths | Gated authenticated channel metadata, isolated thread partitions and exact existing-thread replies. Parent-message `thread` objects are not current membership |
| Edit a sent message | Native text/control updates; gated relay text edits | Unsupported by the current CMA relay contract | Gated ordinary text edits and issued-card updates | Gated native single-part text/link edits and separately gated native card replacement with durable receipt replay; gated relay text edits |
| Delete/react to a sent message | Gated effects on confirmed bot messages; a separate default-off gate adds effects-only references for one individual native upload. Subject to permissions, age and reaction limits | No CMA relay operation | Separate opt-in CMA adapter through the existing NyxID bot-service proxy: exact confirmed text/cards, own retained reaction ID, durable uncertainty fences; live acceptance unverified | Gated native effects on confirmed single-part bot text/link replies; separately gated effects-only uploads contained in one exact native message. Durable idempotency and per-message serialization; no arbitrary message IDs or relay effects |
| Interactive controls | Inline keyboards/choices and durable verified callbacks; named-consumer issuance has its own gate | Gated actor-bound buttons and single-choice lists; approved templates have a separate gate | Gated JSON 2.0 cards and durable verified callbacks; separately gated typed Form submissions | Gated native buttons/single-select controls, separately gated multi-select and modal Forms, and durable type-3/type-5 callbacks bound to the original actor, message, consumer and conversation. Relay controls remain blocked by the pinned upstream contract |
| Incoming edits/deletes/reactions | Owner-opted-in native retained private/group edits and attributable user reactions notify the original agents through durable admission. Channel-post edits and anonymous reactions remain CMA limitations; Bot API has no generic deletion update. See [native message events](CMAEG_Native_Telegram_Message_Events.md) | Native opt-in supports private text and reactions on retained incoming and confirmed outgoing messages; existing relay status/system/reaction input remains refused | No general message-edit/delete/reaction propagation | Default-off durable edits/deletes retain original consumers; separate owner reaction-intent opt-in supports add/remove/all/emoji-clear deltas on retained inbound and confirmed outbound messages, with exact original routing and bounded durable wait for confirming receipts and explicit expired-target refusals |

Native Discord thread creation, archive/reopen, join/follow and listing are not
implemented. Explicit history reads have a default-off, source-opt-in path through
`readEventContext`: at most 20 text messages preceding the original message in
its exact existing thread, with no pagination or write authority. Existing-thread replies do not imply those
operations. Native uploads and multi-part/uncertain sends do not issue the
single-message edit/effect authority described above. WhatsApp editing and groups
are limitations of this CMA integration, not universal claims about the platform;
WhatsApp account identity also does not prove a human sender.

Incoming WhatsApp reactions are filtered out by the pinned NyxID adapter before
callback construction. CMA cannot enable them by widening its relay parser or
declaring activity callback support. See the [pinned reaction-contract review and
CMA-only ingestion path](CMAEG_WhatsApp_Incoming_Reactions.md).

The separate default-off [native WhatsApp source](CMAEG_Native_WhatsApp.md) adds
private text ingress, retained incoming/outbound reaction authority and native
text replies without changing that relay. Its local signed-webhook/provider/send
tests pass and structured source/credential setup is available in the portal; native media, templates and connected Meta
acceptance remain required, so this does not complete WhatsApp support.

## Native Lark and Feishu forms

The CMA native form path supports one Form with 1–12 text, number, date or
single-choice fields. It uses the pinned NyxID adapter's existing native-card and
raw callback contract; no upstream change is included. Provider results use the
existing neutral `InteractionResult.values.fields` JSON map, with the complete
canonical submission saved before dispatch. Original actor, source, provider,
consumer, conversation, partition, message and revision remain bound through
restart, callback replay and replacement.

`CMAEG_LARK_FORMS_ENABLED` defaults to false in code; deployment policy enables it
after compatible readers and isolated Custom verification. It issues native forms only
when structural preflight and existing native-control context checks succeed.
Otherwise valid replies preserve their prior rendering and destination, including
plain-text forms alongside native Buttons/Choice and text replies in threaded,
published or nonhuman contexts. Fallback grants no form callback authority.
Authorization, provider-context and storage errors remain fail-closed; uncertain
writes or sends never trigger fallback or another send. Already-issued native
forms remain consumable and editable after issuance is stopped; historical text
forms cannot acquire native callback authority through an edit. Independent
controls mixed with a native form, multi-choice fields, native media and threaded
forms remain unavailable for native issuance. Existing media and unavailable-thread
refusals still apply. Local signed callback and recorded delivery tests establish
the composed CMA path; actual Lark and Feishu rendering/submission acceptance
remains unverified. See [native forms](CMAEG_Native_Lark_Forms.md).

## Native Discord forms

Native Discord Forms are separately default-off. Eligible forms use one launch
button and a type-9 modal containing up to five Label-wrapped TextInput or
StringSelect fields. Number and Date use explicitly labeled, strictly validated
text; their rendering report records degradation. Whole submissions preserve
the original actor, provider, consumer, conversation and destination. Invalid
values do not consume a card; users open a fresh modal to retry. Existing text
fallback for oversized or mixed forms is preserved before native side effects.
Retained callbacks and exact lost-inbox recovery survive disabling issuance.
Local contract fixtures pass through the real routed provider execution path;
real Discord client acceptance remains unverified. See
[native Discord forms](CMAEG_Native_Discord_Forms.md).

## Native Discord integration and its bounds

The application composition starts owned source workers and an inbox dispatcher
when the required runtime gates and provider configuration are enabled.
Bot credentials remain encrypted; authenticated discovery binds the actual bot,
Gateway endpoint and intents before the durable per-bot lease and IDENTIFY quota
permit a connection. Resume, checkpoint recovery, source revocation, reconnect
cooldowns and shutdown are implemented. The native path has no NyxID dependency.
Only one-shard bots are supported; a sharding requirement is explicitly refused.
Bot permissions, channel access and the privileged MESSAGE_CONTENT intent remain
real Discord prerequisites.

Self-service creation initializes default-consumer routing for an explicitly
authored native source when its gates and a replay-capable provider are present.
Discovery verifies exact current records from the sealed channel index and keeps
at most 64 selected records, including explicit canaries. It does not require
manually adding each new native source to a canary list, and it does not broaden
ordinary HTTP/custom ingress selection. Partial index reads cannot revoke a
worker by treating an unseen record as absent.

Native component callbacks retain one original consumer, provider binding,
conversation and partition. Opaque control IDs resolve options from the saved
card; callback-provided business values cannot change that authority. A sanitized
durable inbox and Activity receipt precede one type-6 acknowledgement attempt
within 2.5 seconds of the interaction's creation time. Uncertain acknowledgements
are not retried. Exact consumed callbacks can recover their inbox after that
deadline; new late clicks cannot consume a card. Buttons and single-select controls
also support verified existing thread types 10, 11 and 12. Issuance and fresh
callbacks authenticate the exact channel, type, guild and parent; the saved card,
Activity and reply retain that same thread. Exact consumed replay retains the
original proof. Multi-select has a separate default-off issuance gate and retains
the complete canonical selection for callback recovery, including an explicit
empty selection; old buttons/single-selects keep their existing result shapes.
Controls with native media remain unsupported. Native interactive card edits
have a separate default-off gate and retain one exact platform message through a
durable revision chain; text-only completion explicitly removes all controls.
See [native card edits](CMAEG_Native_Discord_Card_Edits.md).

Native receipt is persisted and observed before provider execution. Immutable
inbox snapshots preserve routing and receipt time across replay. Verified thread
metadata comes from exact authenticated `GET /channels/{channel_id}`, with bounded
reads and no per-channel cache; it is saved with the inbox. Missing or mismatched
proof refuses thread partitioning. Replies stay on the original thread channel,
and sealed thread authority cannot retarget to its parent.

Native edits and effects recheck current source, owner, consumer and bot custody.
Effect authority has a maximum 24-hour window; editing cannot renew it. Different
idempotency keys serialize on the same message, preventing concurrent reaction
replacements from leaving multiple bot reactions. Unknown writes remain unknown
and fenced instead of being retried under another key.

Source anchors: `discord/lifecycle.rs`, `source_discovery.rs`, `journal.rs`,
`inbox_reader.rs`, `thread_context.rs`; `channels/native_discord.rs`;
`delivery/discord.rs`, `discord_edit.rs`; `tools/message_effects/discord.rs` and
`discord/rest/effects.rs`, all under `api/crates/cmaeg/src/`.

## Implemented gates and deployment status

The checked-in `infra/cmaeg/configmap.yaml` records the desired staged activation:
native Discord authoring/runtime/discovery, verified thread metadata, edits,
effects and controls; routed recovery/admission; multi-consumer and dynamic-source
authoring; named-consumer controls; and native Telegram effects. The CMA provider
has explicit Responses v2 recovery paths. Nyxbot recovery remains absent because
its deployed durable GET replay contract has not been proved. Explicit record
lists remain empty, so ordinary existing HTTP/Custom ingress is not moved into
routed execution. Native discovery selects only explicitly authored native sources.

These desired values are not evidence of production activation. Deploy the new
images with the previous live configuration first, let predecessor Pods drain,
then apply the reviewed activation values and roll only the Gateway. Existing
outbound-message authoring, raw relay verification, thread-contract authoring and
inbound media stay enabled. The desired user-testing activation also enables
relay native uploads, relay text edits, Lark cards and WhatsApp interactions/templates.
Native Lark/Feishu and Discord forms plus Discord card effects were subsequently
enabled after PR #1148 deployment and fresh isolated Custom verification.
The observed NyxID catalog and matching serving source implement these wire
contracts; actual platform delivery/callback acceptance remains unverified until
an authorized test uses each bot. Enabling these features does not authorize test
messages to external recipients. Complete replies remain disabled pending actual
upstream delivery/receipt acceptance and retained-runtime compatibility checks.
The implemented complete-reply exchange/tool budgets are 15 s/18 s; native media
and media-bearing WhatsApp templates use 60 s/65 s outside routed execution.
Text-only templates retain their configured exchange timeout and 8 s tool wait.
Routed delivery retains its independent 20 s window. Compatible new runtimes
wait 70 s for tools; retained runtimes may still cut off at 10 s. These finite
budgets do not guarantee that every valid multipart send finishes in time.

Activation requires the following release sequence:

1. Deploy compatible serving and rollback readers before issuing new persisted
   metadata, message references, effect windows or routing records.
2. For native Discord self-service, enable native source and routing authoring,
   native runtime/discovery and routed recovery, and configure the selected
   provider's verified response-recovery paths. Explicit record canaries remain
   available. Discovery does not enable unrelated HTTP/custom routed admission.
3. Enable `CMAEG_NATIVE_DISCORD_THREAD_METADATA_ENABLED` for verified thread
   partitioning. Enable `CMAEG_NATIVE_DISCORD_EDITS_ENABLED` for new edit refs;
   native effects additionally require
   `CMAEG_NATIVE_DISCORD_MESSAGE_EFFECTS_ENABLED`. Upload messages instead use the
   separate default-off `CMAEG_NATIVE_DISCORD_MEDIA_EFFECTS_ENABLED` together with
   the message effects gate; one exact confirmed message only, without edit
   authority. Retained authority remains
   readable after issuance is disabled.
4. Verify each enabled feature against a specifically authorized test source and
   the actual provider contract. Local fixtures and code flags are not permission
   to send messages to external recipients. No external sends are authorized in
   the current work. Native control issuance separately requires
   `CMAEG_DISCORD_INTERACTIONS_ENABLED`; named-consumer controls also require
   `CMAEG_CONSUMER_INTERACTIONS_ENABLED`.

Exact settings and contracts are maintained in [Operations](CMAEG_Operations.md)
and the [protocol](CMAEG_Protocol.md).

## Provider boundaries that CMA flags cannot remove

- Pinned NyxID Discord receives signed Interactions HTTP requests; its
  `parse_gateway_message` helper has no Gateway socket producer. CMA's native
  transport supplies ordinary Gateway intake. The pinned relay component
  constant is 4, whereas Discord components are type 3 and type 4 is autocomplete;
  its outbound relay builder also lacks the native control contract. CMA must
  not treat those callbacks as user consent. No NyxID fix is included here.
- Lark rich-post image/video exists in NyxID's internal `embedded_attachments`,
  but its external callback and indexed download contract expose only
  `inbound.attachments`. CMA retains honest placeholders instead of inventing a
  download URL. The additive default-off `lark_resources` path instead uses the
  existing authorized NyxID bot-API proxy and independent receiving-bot evidence.
  The owner connection selector offers eligible personal/shared connections with
  verified regional endpoint evidence. Saved/manual bindings remain available;
  selection grants no new access and makes no live regional acceptance claim. See the
  [binding contract](CMAEG_Protocol.md#opt-in-lark-and-feishu-embedded-resource-binding).
- WhatsApp free-form replies remain subject to its 24-hour customer service
  window. Approved templates require provider approval and the matching relay
  contract; a CMA template flag does not grant either.

## Verification evidence and remaining acceptance

Production verification on 27 October 2026 completed for candidate
`f308d88f23754c2c55aef5a51372fd534c2a89d9`, built by the fully passing
[CI run 40486174299](https://github.com/ChronoAIProject/cma/actions/runs/40486174299).
Agent Manager, Event Gateway and Web converged before Discord multiselect and
card-edit issuance were enabled. The checked-in manifest records those enabled
values; their code defaults remain false. Lark forms and complete replies remain
disabled. The Lark connection endpoint returned HTTP 200, but the test owner had
no eligible connections, so actual connection selection remains unverified.

An isolated production Custom channel verified invalid-signature rejection,
completed ordinary and structured Form/multiselect replies, and Activity visible
at 237 ms without creating another provider turn on reconnect. The first
structured attempt timed out waiting for guarded approval and remains a failed
delivery; separate explicitly approved events completed. Structured durations
include approval time. These results establish Custom transport behavior, not
native platform rendering, callback or media acceptance. The fixture and its
temporary credential were subsequently retired through their public APIs.

Local WebSocket and HTTP fixtures cover native ownership/resume quotas, source
rotation, discovery/revocation, receipt-before-execution, immutable replay, media
custody/caps, exact thread partitions/replies, edits/effects, rate limits and
uncertain writes. Native ordinary and verified-thread fixtures execute the real
routed pipeline through a local provider HTTP server and Discord REST server;
repeated inbox scans and a lost acknowledgement produce one provider POST and
one reply. These are local adapter/contract results, not connected-platform tests.
The controls loopback fixture exercises a real provider POST and a reply to the
original card: one acknowledgement, one provider turn and one reply, with Activity
visible first and the interaction token absent from retained/provider bytes.

The earlier deployed Activity API sample, recorded in
[Deployment readiness](CMA_Deployment_Readiness.md#live-channel-activity-and-flow-workspace-publication-from-pr-1122-2026-10-27),
used an isolated production Custom Channel: the corresponding Activity snapshot
was received over SSE **196 ms after the event HTTP POST was sent**. A latest-Activity GET at
**402 ms** already included the in-flight event, before provider completion at
**15,921 ms**. Cursor reconnect restored the completed snapshot without another
provider turn. This demonstrates receipt visibility independent of provider
completion in one API/SSE sample; it is not a percentile, authenticated production
browser measurement or Telegram/WhatsApp/Lark/Discord delivery result. Browser
journeys were checked locally. No external platform messages were sent.

The subsequently deployed PR #1130 Web image keeps one live feed for the selected
Channel, shared between the map preview and Activity dialog. In its isolated
production Custom sample, the event appeared over SSE after **413 ms** and in the
browser after **420 ms**, before provider completion at **10,108 ms**. Reopening
the Channel displayed its latest record after **149 ms** and completed the
entrance animation after **367 ms**. Reconnect retained one provider turn. These
browser checks used production assets and API/SSE through a saved-token transport
adapter; ordinary OAuth cookie login was not exercised. The fixture and its exact
temporary key were retired. These are individual samples, not latency percentiles
or external-platform delivery evidence.

Remaining acceptance is explicit: complete the combined release checks and
controlled Custom Channel verification, then obtain authorization for platform ingress, media, reply,
thread/edit/effect and callback tests under each actual bot's permissions. Record
live gates and source/provider revisions separately from this source audit.
Overall four-platform support remains incomplete until those gaps and external
acceptance are resolved.


### Gateway followup release and current test limits

PR #1148 (CI40505257949) is deployed at Gateway digest
`sha256:1fd3345ce6f1bf7c7bbfb49af5f064848d28ab99e71f2da735b722431f68a2eb`.
Discord forms, Discord card effects and Lark forms are enabled. Agent Manager,
Web and runtime images did not change in that rollout. Health/readiness and
OpenAPI returned 200; existing user Pod/PVC identities and sampled histories
were retained. A pre-existing exit-2 crash loop continued independently, and
an unowned warm pool Pod/PVC was replaced; those differences were explicitly
reviewed rather than described as unchanged.

A fresh autonomous Custom fixture completed four events, including typed Form
and multiselect replyMessage output. Invalid HMAC returned 401. Activity appeared
after 251 ms in the measured run, and reconnect did not start another provider
turn. The exact temporary channel, sandbox and key were retired afterwards.
No Telegram, WhatsApp, Lark/Feishu or Discord messages were sent by this test.
Native platform rendering, callbacks and platform permissions remain separate
acceptance work; no claim of four-platform completion follows from this result.

# Four-platform messaging status

Reviewed 2026-10-27 against the integrated CMA source, including native Discord
self-service, edits, verified threads, message effects and component controls.
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
| Edit a sent message | Native text/control updates; gated relay text edits | Unsupported by the current CMA relay contract | Gated ordinary text edits and issued-card updates | Gated native single-part text/link edits through sealed exact-message references; gated relay text edits |
| Delete/react to a sent message | Gated effects on confirmed bot messages, subject to permissions, age and reaction limits | No CMA relay operation | No CMA relay operation | Gated native effects on confirmed single-part bot text/link replies; durable idempotency and per-message serialization. No arbitrary message IDs or relay effects |
| Interactive controls | Inline keyboards/choices and durable verified callbacks; named-consumer issuance has its own gate | Gated actor-bound buttons and single-choice lists; approved templates have a separate gate | Gated JSON 2.0 cards and durable verified callbacks | Gated native buttons/single-select controls and durable type-3 callbacks bound to the original actor, message, consumer and conversation. Relay controls remain blocked by the pinned upstream contract |
| Incoming edits/deletes/reactions | Do not start ordinary agent work | Status, system and reaction input is refused | No general message-edit/delete/reaction propagation | Native ordinary intake admits `MESSAGE_CREATE`; edits/deletes/reactions do not start work |

Native Discord thread creation, archive/reopen, join/follow, listing and history
retrieval are not implemented. Existing-thread replies do not imply those
operations. Native uploads and multi-part/uncertain sends do not issue the
single-message edit/effect authority described above. WhatsApp editing and groups
are limitations of this CMA integration, not universal claims about the platform;
WhatsApp account identity also does not prove a human sender.

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
deadline; new late clicks cannot consume a card. Controls in threads, multi-select,
controls with native media and interactive card edits are not implemented.

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
inbound media stay enabled. Relay native uploads, complete-reply attestation,
relay text edits, Lark cards and WhatsApp interactions/templates remain disabled
until the actual serving relay contract and its required acceptance are verified.

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
   `CMAEG_NATIVE_DISCORD_MESSAGE_EFFECTS_ENABLED`. Retained authority remains
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
  This is advanced explicit configuration, with no generic connection selector or
  automatic grants and no live regional acceptance claim. See the
  [binding contract](CMAEG_Protocol.md#opt-in-lark-and-feishu-embedded-resource-binding).
- WhatsApp free-form replies remain subject to its 24-hour customer service
  window. Approved templates require provider approval and the matching relay
  contract; a CMA template flag does not grant either.

## Verification evidence and remaining acceptance

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

The latest deployed Activity sample, recorded in
[Deployment readiness](CMA_Deployment_Readiness.md#live-channel-activity-and-flow-workspace-publication-from-pr-1122-2026-10-27),
used an isolated production Custom Channel: the corresponding Activity snapshot
was received over SSE **196 ms after the event HTTP POST was sent**. A latest-Activity GET at
**402 ms** already included the in-flight event, before provider completion at
**15,921 ms**. Cursor reconnect restored the completed snapshot without another
provider turn. This demonstrates receipt visibility independent of provider
completion in one API/SSE sample; it is not a percentile, authenticated production
browser measurement or Telegram/WhatsApp/Lark/Discord delivery result. Browser
journeys were checked locally. No external platform messages were sent.

Remaining acceptance is explicit: complete the combined release checks and
controlled Custom Channel verification, then obtain authorization for platform ingress, media, reply,
thread/edit/effect and callback tests under each actual bot's permissions. Record
live gates and source/provider revisions separately from this source audit.
Overall four-platform support remains incomplete until those gaps and external
acceptance are resolved.

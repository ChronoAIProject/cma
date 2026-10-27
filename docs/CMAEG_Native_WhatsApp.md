# Native WhatsApp messages and incoming reactions

The native `whatsapp` source receives original Meta Cloud API webhooks directly.
It is separate from `nyxid_relay` with `platform: "whatsapp"`; existing relay
definitions, callbacks and sends keep their existing behavior. The pinned relay
currently discards reactions before CMA receives them; see
[the upstream audit](CMAEG_WhatsApp_Incoming_Reactions.md).

This implementation supports private text messages, gated [inbound media](CMAEG_Native_WhatsApp_Media.md), incoming reaction updates
on retained incoming messages and confirmed CMA replies, gated [reply buttons and single-choice lists](CMAEG_Native_WhatsApp_Controls.md), and actual native
text, separately gated [JPEG/PNG/PDF/audio/sticker/video replies](CMAEG_Native_WhatsApp_Outbound_Media.md)
and [approved-template replies](#approved-template-replies) through Graph API `v25.0`.
A separate gate enables [own reactions to authenticated received messages](CMAEG_Native_WhatsApp_Reactions.md).
This does not establish complete WhatsApp support. Further video/audio/document profiles,
outbound editing and connected Meta client
acceptance remain required follow-ups. No production rollout, Meta app setting
change, or external send is part of this implementation.

## Reader deployment and authoring

`CMAEG_NATIVE_WHATSAPP_WRITES_ENABLED` defaults to `false` in code. The deployment
ConfigMap enables owner setup and testing after compatible Gateway/Web readers
and isolated Custom-channel acceptance were verified. This activation does not
claim connected Meta acceptance; the owner must still configure and test the
dedicated app. Keep compatible readers everywhere after enabling authoring:
older readers cannot decode the new source, credentials or sealed reply address.
Native authoring also requires
`CMAEG_MULTI_CONSUMER_WRITES_ENABLED`, `CMAEG_ROUTED_ADMISSION_ENABLED` and
`CMAEG_ROUTED_RECOVERY_ENABLED`. Every active consumer must use a provider with
durable response recovery. Retained work uses the existing global recovery scan;
native WhatsApp does not require adding every record to the legacy canary list.

Authenticated `GET /v1/channel-capabilities` reports optional
`relay_authoring.native_whatsapp_writes_enabled` and
`relay_authoring.native_whatsapp_auto_activation_enabled`. The first is the exact
native authoring flag; the second requires durable admission/recovery and routing
authoring. New setup requires both to be explicitly `true`; missing or null fields
on an older server do not grant support. Provider replay and Meta custody remain
per-channel checks. Unchanged saved native sources and credentials remain valid
for unrelated edits with authoring disabled.

Create through the authenticated channel API with a stable canonical UUID v4
`record_id`, the usual channel/profile/provider fields, and these additional
fields (the example contains placeholders only):

```json
{
  "sources": [{
    "type": "whatsapp",
    "app_id": "12345",
    "waba_id": "23456",
    "phone_number_id": "34567",
    "admission": {"type": "all"}
  }],
  "credentials": {
    "agent_key": "<existing provider agent credential>",
    "whatsapp": {
      "app_secret": "<Meta app secret>",
      "access_token": "<Meta system-user access token>",
      "verify_token": "<private random callback verification token>"
    }
  }
}
```

All IDs must be numeric Meta IDs. The three credentials are one atomic bundle;
rotation supplies all three. They are encrypted in the channel record and omitted
from public definitions. Missing native credentials refuse authoring. Secrets must
contain at least 16 printable bytes; access tokens are bounded at 16 KiB and the
other two secrets at 256 bytes. Source removal removes the retained native bundle.

Before saving a new/changed source or credentials, CMA makes two read-only Graph
requests: `debug_token` must prove a valid token belonging to the exact configured
app with both `whatsapp_business_management` and `whatsapp_business_messaging`
scopes; `/{waba_id}/phone_numbers` must contain the exact configured phone. Missing
proof, a different app or phone, unavailable verification, and absent scopes all
refuse before provider or record mutation. Membership verification checks one
bounded page of up to 100 phones; an unproven later page is refused. CMA does not
follow provider-supplied pagination URLs with credentials. Provider error messages
and token-bearing request URLs are never included in CMA error output.

New channels receive default-only durable routing; adding the source to an
unrouted channel initializes that routing. Existing routing is preserved. An
account's `from` value does not prove a human, so actor kind remains `unknown`.
Consumers must accept unknown actors through their effective `non_human` policy;
the configured default provider policy permits `cma_codex`. Changing that policy
can explicitly refuse ingress. Private chats have no thread partition, so
`partition.key: "thread"` is refused for this source.

## Dedicated Meta callback

The channel response returns `endpoints.whatsapp_webhook_url`, ending in
`/v1/whatsapp/{channel_id}`. Configure that URL and the same verification token on
a **dedicated Meta app** and subscribe its WhatsApp `messages` webhook externally.
Meta callback configuration is app scoped: sharing an app with a relay or another
channel can replace the callback and redirect deliveries. CMA does not modify
Meta app settings and does not claim global uniqueness of an app across channels.
The owner must provide the dedicated app and perform external configuration.

GET challenge handling requires exactly one `hub.mode=subscribe`, verification
token and nonempty challenge. Token comparison is constant time. POST accepts one
`x-hub-signature-256` and verifies HMAC SHA256 over the original raw request bytes
using the configured app secret before extracting messages. It independently
checks WABA entry ID and receiving phone metadata. Payload-supplied app identity
never substitutes for the custody proof established at authoring.

Status-only notifications and unsupported message types are acknowledged without
provider work. Supported messages outside the seven-day timestamp bound or more
than five minutes in the future are refused. Admission policies use the raw
sender as the private conversation. Authenticated policy/target refusals increment
the webhook `refused` count and create owner Activity refusal diagnostics;
temporary durable-admission failures return a retryable HTTP error.

## Reaction authority and replay

Meta reactions become `ActivityKind::Event` with CloudEvent type
`cma.message.reaction.updated.v1`. `subject` and `data.message_id` contain the
native target ID. Data includes `original_event_id`, `original_author`,
`new_reactions` and `provider_timestamp`. A nonempty emoji is the sender's current
replacement state. Meta's [reaction webhook reference](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/reaction)
documents removal by omitting the `emoji` field. CMA maps that omission and an
explicit empty string to `new_reactions: []`; a present null, non-string,
control-containing or oversized value remains invalid. `old_reactions` is absent
because Meta does not supply it. The reacting account remains `unknown`; the
original author is a bot only when an exact confirmed CMA send receipt proves
that fact. This inbound contract does not establish outbound reaction removal.

Incoming targets must exist in a retained, admitted ordinary-message plan for
the exact source and private sender. Outgoing targets require an encrypted
immutable confirmed-send receipt with exact app/WABA/phone, recipient, native
message ID, original admitted event, consumer, provider, partition and conversation.
That reaction reaches only the receipt's consumer, including when the confirmed
reply was triggered by a retained reaction event. It never broadcasts to other
consumers that happened to receive the original incoming message. Source removal,
provider changes and expired evidence revoke target authority.

Events retain the original consumer/provider/partition/conversation selection;
current routing rules do not retarget them. The stable idempotency key hashes the
exact app/WABA/phone and Meta message ID. The first full verified raw envelope is
retained. A rebatched duplicate compares the same message facts and reuses that
first envelope for replay; changed facts under one ID conflict. Admission is
durable before acknowledgement or scheduling. Within one webhook batch, text is
admitted before reactions so that an accompanying original can establish target
authority.

Retention is at most seven days and can be shorter when the initiating plan
expires earlier. Recent unknown targets return retryable HTTP 503 with
`Retry-After: 5` for a bounded 120-second grace after the provider timestamp. This
covers an outbound message becoming visible before its receipt commit, or a
reaction arriving before its original in another callback. A retry dispatches
only after exact durable target evidence exists. After grace, unknown targets
are explicitly refused and acknowledged; expired evidence refuses immediately.
There is no pending-target queue. Distinct out-of-order
reaction events preserve provider timestamps. CMA does not materialize a current
reaction state or claim a total order for equal timestamps.
Temporary failures reading a receipt's original admitted plan also return 503;
a successful later read can admit the same reaction exactly once. A missing or
expired original remains unavailable and cannot create a fresh route.

## Native reply confirmation

Text uses `POST /v25.0/{phone_number_id}/messages`, the original private recipient,
and a native context message ID. Current exact source credentials are required
again at send time. Confirmation requires the WhatsApp product marker, one exact
recipient and one usable native `wamid` receipt, then durable receipt storage.
Each rendered text part records its own target receipt. Uncertain acceptance,
receipt-storage loss, and partial delivery remain `unknown`; CMA does not resend
automatically after an uncertain native effect. Free-form replies remain subject
to Meta's customer service window; approved templates have a separate gate.

Validation uses original Meta-shaped envelopes, injected Graph transports and
local CMA/provider HTTP fixtures. These prove local contracts and do not prove
acceptance under a live app's current permissions or a real WhatsApp client.

## Approved-template replies

`CMAEG_NATIVE_WHATSAPP_TEMPLATES_ENABLED` defaults to false in code and the source
ConfigMap. It is independent of the existing NyxID template flag and native media
upload flag. Enabling it advertises the existing `capabilities.whatsapp_templates`
only for verified private native contexts whose sealed source and recipient still
match current source credentials. The existing `OutboundMessage.whatsapp_template`
shape supplies approved name/language, text/image/video/document header, body
text/currency/date-time substitutions and URL buttons at indices 0–2. See the
[full typed contract](CMAEG_Protocol.md#approved-whatsapp-template-replies).

Media headers use numeric Meta IDs or public HTTPS references. The shared template
URL policy refuses private/special addresses and local/internal hosts before any
send commitment; Meta checks ownership, approval, language and placeholder
compatibility. Template media is not uploaded or decoded by CMA. Text templates
keep the eight-second native transport budget; media templates use at most
60 seconds while existing routed execution deadlines remain authoritative.

One native request retains the original phone, recipient and context message.
Current source/credential revision and default or named consumer authority are
checked at the routed send fence. Exact provider confirmation and durable receipt
storage are both required for a confirmed result; the receipt retains incoming
reaction correlation. Uncertain acceptance or receipt loss stays unknown and
replays without resending. No edit reference, owner-initiated native send or
interactive callback authority is added. Local fake-transport and composed routed
tests cover the contract, revocation and receipt replay; connected Meta acceptance
remains outstanding before rollout.

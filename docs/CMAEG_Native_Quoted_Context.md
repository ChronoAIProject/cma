# Native quoted-message context

`Activity.kind.quoted_message` carries bounded, untrusted native context on an
ordinary message. The enclosing Activity supplies its authenticated source and
conversation. This field is descriptive data: it is not a `MessageRef`, reply
address, attachment locator, history cursor, recipient, or permission to fetch or
act on the referenced message.

```json
{
  "type": "message",
  "text": "Yes, that passage",
  "attachments": [],
  "mentions_bot": false,
  "quoted_message": {
    "relation": "reply",
    "message_id": "81",
    "content": {
      "text": {
        "text": "The passage selected by the sender",
        "origin": "selected_quote",
        "truncated": false
      },
      "kinds": ["image"]
    }
  }
}
```

The current message text, actor, source, conversation, thread, partition, routing,
`mentions_bot`, `reply_to`, and sealed outgoing reply destination retain their
existing meaning. A quote containing `@consumer`, `/to consumer`, a URL, a bot
name, or instructions cannot select a consumer, confer identity, or authorize a
tool. Provider input renders it in a separate untrusted data block after routing.

## Actual native coverage

| Source | Relationship and reference | Payload-supplied content | Limits |
| --- | --- | --- | --- |
| Telegram | `reply_to_message.message_id`, `relation: reply` | Prefer the sender's `quote.text`; otherwise parent `text` or `caption`. Descriptive image/video/audio/file/sticker/contact/location/poll kinds. | Same chat and message thread only. Ordinary `message`/`channel_post` on a selected durable routed path. Excludes `external_reply`, forwarded origins, recursive parents and aliases. |
| Discord | Actual type 19 REPLY with DEFAULT `message_reference`, `relation: reply` | Matching `referenced_message.content`, plus attachment MIME-kind, sticker and poll descriptions. | Exact current channel; any supplied reference or parent guild must match the authenticated current guild. The optional reference guild may be absent. Absent/null referenced message gives no content; it does not establish existence or deletion in the public contract. Excludes forwards, crossposts, thread-starter and context-menu message types. |
| Lark / Feishu | Receive event `event.message.parent_id`, `relation: reply` | None: the ordinary receive event supplies the parent ID only. | `root_id` and `thread_id` remain separate facts. No inferred parent text, author or thread membership. Both regional native apps use the same bounded shape. |
| WhatsApp | Plain ordinary private `context.id`, `relation: context` | None: the incoming context contains an ID/from fact, not the prior body. | Requires a bounded native ID and valid context sender shape; the context sender never replaces the current actor. Excludes groups, controls, product referral and forwarding context. Current official evidence establishes incoming context fields; this contract deliberately does not assert that every plain context is a quoted reply. |

`content` is absent for a reference-only payload. A supplied parent message can
have a content object with neither text nor supported kinds; that is not an
assertion that all of its content was normalized. Text is at most 4096 UTF-8 bytes,
truncated on a character boundary with `truncated: true`. Origins are
`selected_quote`, `parent_text`, or `parent_caption`. At most eight distinct
content kinds are retained. Entities, author identity, nested quotes, media IDs,
media URLs, dimensions, and arbitrary content members are not copied.

Unsupported, malformed, self-referential or cross-scope optional quote facts do
not become context. Existing admission validation still decides whether the
current message itself is valid. No provider fetch or retained-message lookup is
performed. The relationship does not prove that the parent exists, that its text
is current, or that any consumer could independently read its history.

## Sealing, replay and reader-first rollout

`CMAEG_NATIVE_QUOTED_CONTEXT_ENABLED` defaults to `false`. Deploy compatible
Gateway, provider/agent, SDK, context-reader and rollback readers before enabling
this writer. The checked-in deployment configuration also leaves it false. Old
strict readers can reject the new optional field and private projection markers;
optional OpenAPI fields alone do not make a rollout safe.

New enabled routed admissions retain `quote_projection: 1`, including messages
that contain no supported quote. An absent version means legacy projection,
permanently. Replays load that retained version before projection and resealing;
the current gate never upgrades old raw envelopes. Quote changes under an enabled
admission conflict. A competing writer reloads and obeys the winning retained
version. The accepted Activity and original consumer selection remain immutable.

Discord additionally pins a bounded versioned snapshot at initial durable inbox
capture. Old inboxes remain legacy even if dispatch occurs after enabling the
writer. Changed quoted facts in versioned inbox redeliveries conflict. Recovery
reads retained snapshots with the writer disabled.

WhatsApp keeps its original normalized `Message` serialization and encrypted
message-change proof hashes unchanged. Rebatched deliveries keep the first raw
signed envelope. Version-aware batch preflight rejects conflicting quote facts
before any message admission when the winning projection retains quotes, even
with the writer disabled. Legacy admissions continue to ignore optional quotes.
The check uses only the configured account, phone and supported private scope.
Lark retains its existing bounded `Message` source bytes;
the temporary parent fact is not added to that legacy serialization. New quote
data resides in the sealed Activity and versioned admission instead.

Every selected consumer's event digest binds the projected context. Editing the
quote invalidates that digest; reading it still requires the original event and
consumer authority. Credential rotation and source/owner custody checks retain
their existing requirements. No extra capability appears in context responses.

The owner activity feed exposes availability, truncation and descriptive kind
metadata only. It does not copy quoted text or parent IDs into a feed that
previously omitted message contents. Authorized event-context readers and the
Rust/TypeScript SDKs expose the actual bounded quote.

## Official field evidence

Checked on 2026-10-28:

- [Telegram Bot API Message and TextQuote](https://core.telegram.org/bots/api#message):
  `reply_to_message` is same-chat and same-thread; `external_reply` can come from
  another chat/topic. `TextQuote.text` is the selected excerpt and its position is
  sender-specified, so it is not treated as independent proof of the parent text.
- [Discord Message reference, pinned official specification](https://github.com/discord/discord-api-docs/blob/7dfc89db5b951ec2fb2637a00d1422c341cef5d8/developers/resources/message.mdx):
  reply type 19, reference DEFAULT/FORWARD distinction, reference scope, and
  absent/null `referenced_message` semantics.
- [Lark receive event](https://open.larksuite.com/document/server-docs/im-v1/message/events/receive.md)
  and [Feishu receive event](https://open.feishu.cn/document/server-docs/im-v1/message/events/receive.md):
  parent, root and thread IDs are separate native fields.
- [WhatsApp official SDK incoming webhook types, pinned](https://github.com/WhatsApp/WhatsApp-Nodejs-SDK/blob/58ca3d5fceea604e18393734578d9a7944a37b15/src/types/webhooks.ts):
  incoming `context.id`, `context.from`, forward flags and product referral fields.
- [WhatsApp current incoming webhook reference](https://developers.facebook.com/documentation/business-messaging/whatsapp/reference/webhooks/whatsapp-incoming-webhook-payload)
  and [text webhook reference](https://developers.facebook.com/documentation/business-messaging/whatsapp/webhooks/reference/messages/text/):
  their documented context example is a Message business product-button referral,
  which this projection excludes. [Contextual replies](https://developers.facebook.com/documentation/business-messaging/whatsapp/messages/contextual-replies/)
  documents outbound `context.message_id`; that outbound field is not accepted as
  an incoming alias or proof of incoming reply semantics.

These are contract and authenticated-payload tests. No native messages were sent,
no live provider acceptance was claimed, and the writer has not been activated.

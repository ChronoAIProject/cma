# Native Telegram forum topic creation

This increment creates one empty named topic in the exact forum supergroup of
an admitted native human message. It uses the existing neutral REST/MCP
`createEventThread` operation and returns a scoped reference usable by
`manageThread`. It defaults off; local transport tests do not establish native
connected-account acceptance or production activation.

The operator enables `CMAEG_NATIVE_TELEGRAM_TOPIC_CREATION_ENABLED` only after
compatible source and sealed-reference readers are deployed. The owner separately
enables `topic_creation:true` on the Telegram source. The portal labels this
**Create new forum topics** and explains that the grant includes rename, close
and reopen of those created topics. This is independent of `topic_lifecycle`,
which manages the original admitted topic, and of incoming message events.
Managed connections retain their bot identity and credentials and can update
the new consent through the normal source editor. Empty-source updates retain
it; new connections default it off. Revocation remains available with authoring
disabled. Automatic default-only routing preserves the provider binding and
requires durable admission/recovery, provider response replay support and the
existing persistent thread-fence store.

## Neutral request and result

`POST /v1/events/{event_ref}/threads` requires the original event reference,
agent credential and one stable `Idempotency-Key`. The Telegram body is:

```json
{"name":"Release planning"}
```

The MCP call is `createEventThread` with `event_ref`, this `request`, and
`idempotency_key`. Names contain 1–100 printable characters. Telegram itself
supports up to 128; CMA retains the shared limit. Omit `auto_archive_duration`
for Telegram. Discord still requires its supported explicit duration and keeps
the same serialized request and durable replay hashes. Null, unknown properties,
caller-selected chat/topic IDs and icon settings are refused.

The existing receipt contains `outcome.status:"confirmed"` and a `thread` with
the created topic ID/name, original chat in `parent_id`, an opaque `thread_ref`
and the original event expiry. At creation `archived` and `locked` are both
false; this snapshot does not grant Telegram archive/lock operations. A confirmed
write whose local custody was revoked in flight is reported without a fresh
reference. References use a distinct Telegram v2 payload; existing Discord v1
payloads and bytes remain unchanged. Both SDKs preserve the opaque reference.
The Rust SDK request field changes from `u16` to `Option<u16>`: existing Discord
source callers use `Some(duration)`, while Telegram uses `None`. This source-level
migration does not change existing Discord HTTP request bytes.

Use `manageThread` with `target:{"type":"thread","thread_ref":"…"}` and
`close`, `reopen`, or `rename` with an explicit `name`. Creation consent and its
gate authorize only that created topic. The old `topic_lifecycle` grant is not
required. Revoked or changed source, owner, selected consumer/provider/credentials
or bot custody invalidates follow-up authority. The reference never grants
sibling enumeration or an arbitrary platform destination.

Creation does not move the original message or change its reply destination.
It creates an empty topic. A later admitted human message in the new topic has
the ordinary reply path. No separate send-to-thread capability is implied.

## Authority and permission checks

Authority requires the retained native webhook bytes and committed routing plan,
the original selected consumer and a current matching opted-in source. Original
`chat.type:"supergroup"` and `chat.is_forum:true` must positively establish the
forum. Both numbered-topic and General-origin messages are eligible. Creation
from General does not manage General and does not require its separate lifecycle
gate. Existing General management behavior remains unchanged.

Private chats, normalized relay events, edits, channel posts, callbacks,
business/guest messages, anonymous `sender_chat` messages and bot actors do not
grant creation authority. Enabling consent after admission cannot retroactively
grant it. Optional context discovery advertises `capabilities.topic_creation:true`
only after local authority restoration, without contacting Telegram. It has a
100 ms budget and reports scope, not live platform permissions. Telegram-only MCP
catalogs expose creation while its gate is enabled and never expose listing.

Before a write, the gateway verifies its bot through `getMe`, the exact current
forum supergroup through `getChat`, and its own current administrator
`can_manage_topics` permission or owner status through `getChatMember`. It then
rechecks local custody and dispatches exactly one `createForumTopic` containing
only the retained chat ID and requested name. Follow-up management repeats the
current permission and custody checks and targets only the sealed created ID.

## Idempotency and uncertain creation

Same-key replay returns the stored receipt; a changed name under that key
conflicts. The creation fence is scoped to bot and chat, independently of
existing exact-topic mutation fences. An uncertain creation blocks new creation
keys and events for that chat across restart and original-event expiry. Other
chats and existing-topic management remain independently scoped.

Telegram's reviewed creation method exposes no client idempotency key, deterministic
topic ID or generic topic lookup/list operation. Therefore timeout, malformed
success, cancellation after dispatch, or unpersisted completion remains
`unknown`; there is no inferred successful lookup and no automatic retry. Do not
retry an unknown result with another key. The non-TTL creation fence has no
automatic expiry or reset endpoint. Definite rejection and rate limits release
the fence after receipt persistence. One eight-second budget covers storage,
permission reads, dispatch and completion, reserving time for the durable receipt.

The existing encrypted interaction and non-TTL thread-fence stores are reused;
there is no new collection, migration or runtime grant. Rollback must retain a
reader for `topic_creation` and Telegram v2 references after activation. Existing
ordinary-message custody excludes the independent topic grants, so enabling
creation does not rewrite previously issued ordinary-message reference hashes.

## Provider support and remaining scope

The official [createForumTopic](https://core.telegram.org/bots/api#createforumtopic)
contract supports forum supergroups and private chats with a user. Private topic
creation is provider-supported but outside this CMA increment. Optional icon color
and custom emoji are also provider-supported but not exposed here. The reviewed
Bot API documentation has no generic topic list method; CMA does not synthesize
one or confuse the separate MTProto API with its Bot API transport.

Provider documentation was fetched on 2026-10-29. Its full-page SHA-256 was
`c58b27685c30087a2388fbe15dcf5ab69704249faef7f6b8fe56a68fb5d0238c`.
Additional references: [ForumTopic](https://core.telegram.org/bots/api#forumtopic),
[getChatMember](https://core.telegram.org/bots/api#getchatmember), and
[getForumTopicIconStickers](https://core.telegram.org/bots/api#getforumtopiciconstickers).

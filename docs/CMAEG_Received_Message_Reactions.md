# Reactions on exact received Telegram and Discord messages

Native Telegram and native Discord can issue reaction-only authority for the
exact original human message that triggered a retained routed turn. The existing
`received_message` context member contains a sealed `message_ref` and its effects:
`scope=received_native_user_message`, `reaction=true`, `delete=false`, and
`max_reactions=1`. Pass that reference to `readMessageCapabilities` and
`setMessageReaction`. The public request and SDK types are unchanged. One reaction
sets or replaces the bot's own reaction; an empty array removes it.

`CMAEG_NATIVE_TELEGRAM_RECEIVED_REACTIONS_ENABLED` and
`CMAEG_NATIVE_DISCORD_RECEIVED_REACTIONS_ENABLED` are separate, default-off issuance
and discovery gates. Each works independently of sent-message edit/effect gates.
Telegram's received-message gate also retains the existing native admission proof
on newly admitted messages. Neither gate modifies source configuration or enables
incoming reaction notifications. The checked-in deployment manifests are unchanged.

## Authority and eligibility

Both context paths require a verified original `Activity`, the immutable tool
reply binding and its original retained routing plan. The original event,
activity, native source, selected default or named consumer and provider must
match that plan. Fresh resolution then verifies current custody. Missing, expired
or unavailable retained metadata omits the optional capability; ordinary verified
context still works. No authority is synthesized from a caller-supplied native ID.
Historical Telegram events without their original native admission proof cannot
acquire this scope.

Only ordinary human `message` activities qualify. Bot, system or unknown actors,
commands, interaction results, reaction/edit notifications, handoffs, relay
sources and albums do not. Private and group conversations are supported. Telegram
topic and Discord thread identities remain bound to the original exact target.
A Discord thread ID, when present, must be its native channel ID.

The sealed scope binds the original event digest, event ID, native message ID,
actor, bot, chat/channel and thread, channel record, owner, source definition,
selected consumer/provider, routing and current credential/key hashes. A source,
owner, routing, provider, consumer-key or credential change revokes an existing
reference. A legitimate current credential revision may issue a fresh reference
only when the retained original source/provider binding still matches. Its expiry
is the earlier of original event expiry and 24 hours after the source timestamp;
reissuing never renews that source-time limit.

This authority grants no message edit or deletion and no reaction operation on
another user's behalf. Those tool calls cannot reinterpret it as a sent-message,
card, relay or WhatsApp scope. Existing WhatsApp reaction behavior is unchanged.
Native Lark/Feishu received-message reactions remain a separate capability gap.

## Native effects and durable retries

Telegram sends one `setMessageReaction` request with the retained numeric chat
and message. Unicode or custom emoji and `is_big` use the existing request
contract; paid reactions are unavailable. Telegram controls permitted emoji and
bot permissions. Telegram has no generic exact-message fetch used here, so the
authenticated original retained admission is required. Albums are excluded
because Telegram documents that a reaction targeting a media group can apply to
the first non-deleted member, which would lose exact-item authority.

Discord verifies the token's bot, fetches the exact retained channel/message and
requires the original human author, an ordinary message or reply, and no webhook
or bot/system author. It resolves custom emoji in that channel's guild and uses
URL-encoded reaction segments. Replacement removes only ordinary reactions whose
`me` flag is true via individual `DELETE .../reactions/{emoji}/@me` requests, then
adds the desired reaction with `PUT .../@me`. Other users' reactions and burst
reactions are untouched. `is_big` is refused. Current custody is checked again
after metadata reads and immediately before every mutation.

Every call uses the existing stable idempotency key. The same key and request
replay the retained receipt; changed content conflicts. The receipt identity is
stable across freshly sealed references and credential rotation. A durable target
fence serializes different keys and consumers for the same bot/chat/message.
Discord shares its existing native per-message mutation namespace. Telegram uses
an incoming-message reaction namespace. Both receipts and fences last until 24
hours after the original source timestamp, even when the first reference expires
earlier.

An ambiguous write, partially completed Discord replacement, or lost final
receipt keeps the target fenced. Neither a new key, process restart nor a freshly
issued reference permits another write through that fence. A definite pre-write
rejection or rate limit is retained and not retried automatically. A successful
API response is evidence of native API acceptance; connected client presentation
has not been verified by these local tests.

## Reader and rollback floor

The minimum compatible Gateway/core reader is the implementation containing
`MessageRefPayload.received_reaction`, the `received_reaction_v1` receipt namespace
and `telegram_received_mutation_v1` fence namespace. This is a new reader floor,
independent of the earlier WhatsApp received-reaction reader floor. The public
`received_message` context and reaction request types already exist, so no new
SDK or OpenAPI field is required.

Deploy that reader everywhere, including retained rollback images, before enabling
either issuance flag. Older sealed-message readers reject the new field and cannot
execute its authority. Historical references omit it and retain their serialized
shape. Gate-off stops issuing and advertising new references; compatible readers
continue executing retained references through their original expiry and current
custody checks. Gate-off alone does not make older images a valid rollback target.
Keep compatible readers and durable receipt/fence records through the last issued
scope's source-time ceiling.

## Validation and official contract evidence

Local coverage composes retained routing and tool binding with the public context,
capability and effect paths. It covers default/named consumers, exact private and
topic/thread targets, own-reaction set/replace/clear, scope tampering, custody
revocation, gate-off execution, replay/remint, concurrent consumers, rate limits,
partial writes and lost receipts. Separate transport checks require Discord's exact
bot/message/human author and recheck custody between mutations. This is source and
local test evidence; no live Telegram or Discord send, NyxID change, deployment or
runtime flag change is included.

Official documentation retrieved on 28 October 2026:

- [Telegram Bot API: setMessageReaction](https://core.telegram.org/bots/api#setmessagereaction)
  documents one bot reaction, custom-emoji limits, no paid reactions, `is_big`,
  `True` on success, and media-group retargeting. SHA-256 of the retrieved HTML:
  `9b615d3c0826b0fe166bddde53ebc9519dfa13252e73e4d0664fea6376b1cfc1`.
- [Discord Message resource](https://docs.discord.com/developers/resources/message.md)
  documents Create Reaction and Delete Own Reaction (`204` success), URL encoding
  and custom emoji `name:id`, `READ_MESSAGE_HISTORY` and conditional
  `ADD_REACTIONS` permission, and separate `me`/`me_burst` flags. SHA-256 of the
  retrieved Markdown:
  `888f00758fd6e7fc5a5e6229745a6dd83aedc4a15ec4492c979752498550f2ff`.

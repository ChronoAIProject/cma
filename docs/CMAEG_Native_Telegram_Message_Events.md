# Native Telegram channel posts, message edits and reactions

Authenticated `edited_message`, `message_reaction`, and `message_reaction_count` updates can
notify the agents that received the original private/group message. The separate
channel-post opt-in also admits retained channel edits and reactions. These use
`Activity.kind.type = event`, preserving the original consumers, provider
bindings, conversation IDs, partition and topic. An edited `/to` command or a
different reacting user cannot select another agent or conversation.

Owners enable this on a direct native Telegram source with `message_events: true`
in the console or authenticated channel create/update API. Omission remains false.
The gateway advertises `native_telegram_message_events_enabled`; new opt-ins require
`CMAEG_NATIVE_TELEGRAM_MESSAGE_EVENTS_WRITES_ENABLED`, routing authoring, durable
recovery and admission, and a provider configured for response recovery. No
operator channel-record allowlist is needed. Opt-in creates default-only routing
when absent, retaining the channel's existing default binding, profile and credential.
Existing routing is preserved. Managed connection sources currently retain their
existing behavior; this opt-in is available through direct native source authoring.

This path requires durable routed ingress and a retained original message.
It never falls back to an ordinary message or the legacy launcher.
Channels without the opt-in retain their ordinary admission and latency, with no
locator reads or writes. For an opted-in channel, the gateway acknowledges the
original webhook with `202` only after both its admission and encrypted message
locator are durable. Locator writes and conflict verification have a 250 ms
budget. An unconfirmed write or readback returns `503`, causing Telegram to retry
that same update; the already committed admission reuses the exact agent turn
instead of creating another one. A create conflict is accepted only when the
saved version-one locator exactly matches the original identity, source revision,
target and expiry. Conflicting context is never overwritten.

The locator contains no message body or credential and grants no authority on
its own; the complete committed original routing manifest must still exist and
validate.

The native webhook secret authenticates the update before interpretation or
lookup. The locator binds the channel record, owner, bot, chat, platform message,
original event and source/credential revision. It expires no later than seven
days after the original message, without renewal. Bot-token, webhook-secret or
source-admission changes revoke it. Missing/expired context is reported as
`message_context_unavailable`; an unavailable record service or an uncommitted
original plan is a retryable 503. Changed bytes under the same update ID conflict
with the durable admission rather than create a second notification.

## Edit representation

An edit requires the exact original author, chat kind, message ID, creation time
and topic, plus a valid `edit_date`. The existing text/media observer validates
the new version; the media gate still applies. The original admitted mention or
reply-to-bot evidence controls group admission even if the edit removes that
addressing. Current sender and chat policy still applies.

The CloudEvent type is `cma.message.updated.v1`, its source is
`telegram:bot:<bot_id>`, and its subject is the target platform message ID.
`data` contains `message_id`, `original_event_id`, `original_author`, and
`patch.text` / `patch.attachments` describing the complete normalized new content.
The Activity timestamp and CloudEvent time are the edit time, not the original
message's creation time. This is a new notification; it does not rewrite a
previous provider transcript or undo work already performed.

## Native channel posts and edits

Direct native Telegram sources may separately set `channel_posts: true`; omission
remains false, including for existing `message_events` sources. Channel posts require
`message_events: true`, its existing authoring gate and durable routing/recovery.
The console exposes the two settings separately; disabling message events also removes
the channel-post opt-in. Source and source-request SDK fields are optional. Managed
connection sources remain opted out. Roll out compatible readers before saving this
new source field; disabling the setting does not make an older source reader compatible.

An authenticated `channel_post` must identify a negative numeric channel chat. An
optional `sender_chat` must identify that same channel. CMA records
`Actor { id: "telegram:chat:<channel_id>", kind: "unknown" }` and a `channel`
conversation. The optional `from`, author signature or title never establishes a
human publisher. Human sender allowlists and sender-deny policies refuse this
identity, including a sender allowlist containing the synthetic channel actor ID.
Open sender admission is required; chat allowlists and non-private group/addressing
rules still apply. `reply_to_bot` admission is not inferred from channel post metadata.

Text, captions, native attachment locators and media album IDs use the existing
normalization bounds and inbound media gate. Channel posts have no message thread:
the Bot API limits `message_thread_id` to private/supergroup chats. Channel topic,
direct-message topic, business-message and automatically forwarded discussion-group
shapes are refused. Anonymous `sender_chat` messages under ordinary group `message`
updates remain outside this extension. No NyxID/relay observer changes are included.

`edited_channel_post` uses the same encrypted seven-day locator and committed original
routing as other message edits. It must match the exact channel, channel actor,
message creation time and message identity, with a valid edit time and no topic.
Edits cannot switch update kind, channel or original consumers through new text or
`/to` commands. Removing the opt-in or changing source credentials/admission revokes
the retained context. Original mention evidence is preserved for edit admission.

Ordinary replies stay bound to the sealed original channel and the channel's bot
credential. They publish to that channel through the existing native send path and
require Telegram's channel posting permission; this feature does not add that
permission or prove it by an external send. No discussion-group/comment destination,
channel direct-message topic, cross-chat reply or quoted-reply authority is inferred.
Unknown channel actors do not receive human Telegram message-effect authority.

## Reaction representation

The Bot API's `message_reaction` update supplies the exact chat, message ID,
reacting user, date, and complete old/new reaction arrays. The Activity actor is
the reacting human; `data.original_author` remains the original message author.
The event type is `cma.message.reaction.updated.v1`. Its data contains the same
message/original-event fields plus `old_reactions` and `new_reactions`. An empty
new array represents removal. Unicode emoji, custom emoji IDs and the Bot API's
paid reaction type retain their distinct shapes. Duplicate or malformed reaction
entries are refused; reactions are canonicalized as sets.

Telegram does not include original text or topic in a reaction update. Those
facts come only from the verified original admission. Both original-author and
attributable-reactor admission are checked. No guessed author, empty prior-state
inference, or current routing decision substitutes for that context.

## Anonymous reaction facts

An `actor_chat` reaction preserves the represented chat as `data.actor_chat`
(`id` and `type`). Its Activity actor is `unknown`, with the distinct identity
`telegram:chat:<chat_id>` and an optional bounded chat title. It never adopts the
original message author or a numeric human identity. Supplying both `user` and
`actor_chat` is refused.

A `message_reaction_count` notification uses the same reaction-updated event type
with `data.observation = aggregate` and a complete `data.reaction_counts` array
of `{type, total_count}`. It does not invent `old_reactions`, a user delta, or an
exact per-person receipt. Its actor remains `unknown`, identified as
`telegram:anonymous:<chat_id>`. Empty counts are valid; reaction types must be
unique and valid, with at most 64 entries and nonnegative 32-bit integer counts.
Telegram may batch/delay these aggregate updates. The source event timestamp is
retained rather than presented as a real-time per-person reaction timestamp.

Both anonymous paths require an open sender policy. A human sender allowlist or
deny policy refuses them because Telegram supplies no verifiable human identity.
Original-author, current chat/group policy, retained source/credential and exact
original consumer/partition/topic checks still apply. No new consumer is chosen.
Duplicate update IDs replay their exact admission; changed content under an
already admitted ID conflicts. The existing `message_events` opt-in is required;
channels without it retain their existing behavior.

Reactions on a retained channel post additionally require `channel_posts: true`.
The original channel author remains unknown and is rechecked with open sender
and current channel admission. Human reactors retain human admission; anonymous
reactors and aggregate counts retain the unknown-actor rules above. Original
consumers, provider bindings and the channel destination remain unchanged.

## Setup and remaining limits

New native webhook registrations request `message`, `edited_message`,
`channel_post`, `edited_channel_post`, `callback_query`, `message_reaction`, and
`message_reaction_count`, preserving pending updates. Existing
webhooks need an explicit registration refresh before Telegram sends the added
update types. Telegram requires the bot to be a chat administrator and requires
the respective reaction type explicitly in `allowed_updates`; it does not send reactions
set by bots. A production registration change is separate from this code change.

| Input | Status and reason |
| --- | --- |
| Private/group message edit | Supported for a retained durably admitted original, with existing text/media and actor admission |
| Attributable user reaction | Supported with the same original-message requirement and Telegram administrator/update subscription prerequisites |
| `channel_post` / `edited_channel_post` | Native explicit `channel_posts` opt-in; unknown channel identity, current admission and retained original edit routing |
| Anonymous `actor_chat` reaction | Supported for retained originals with open sender policy, preserving the represented chat and unknown human identity |
| `message_reaction_count` | Supported as bounded aggregate facts for retained originals with open sender policy; no human or per-user delta is inferred |
| Generic message deletion | No general Bot API deletion update exists. `deleted_business_messages` belongs to a separate business-connection contract and is not ordinary deletion coverage |
| Message predating retained context, legacy-only ingress, or expired context | No notification authority is inferred; ordinary messaging remains available |
| NyxID Telegram relay | Unchanged; these native webhook extensions do not claim relay support |

The contract was checked against the official [Bot API Update](https://core.telegram.org/bots/api#update),
[MessageReactionUpdated](https://core.telegram.org/bots/api#messagereactionupdated),
and [MessageReactionCountUpdated](https://core.telegram.org/bots/api#messagereactioncountupdated)
definitions captured on 2026-10-26. Local fixtures exercise authenticated webhook
and encrypted-record behavior. No external Telegram send or production webhook
change is part of this implementation.

Channel-post source work was checked against the official Bot API `Update`, `Message`
and `ReplyParameters` sections saved at
`/private/tmp/cmaeg-received-reactions-evidence-20261028/telegram-bot-api.html`
(SHA-256 `9b615d3c0826b0fe166bddde53ebc9519dfa13252e73e4d0664fea6376b1cfc1`).
Local tests cover channel identity, explicit opt-in, human/chat allowlists, media gating,
wrong topics/forward shapes, sealed original destination, durable webhook admission
and retained edit routing. Source preparation has not registered a webhook or sent a
Telegram message; live channel acceptance remains separate.

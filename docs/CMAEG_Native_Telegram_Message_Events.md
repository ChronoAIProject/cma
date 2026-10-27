# Native Telegram message edits and reactions

Authenticated `edited_message` and attributable `message_reaction` updates can
notify the agents that received the original private/group message. They use
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
locator reads or writes. For an opted-in channel, after successful admission the
gateway makes a bounded, best-effort write of an encrypted message locator.
Failure of that write does not fail or retry the ordinary message. The locator
contains no message body or credential and grants no authority on its own; the
complete committed original routing manifest must still exist and validate.

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
reactor admission are checked. No guessed author, empty prior-state inference,
or current routing decision substitutes for that context.

## Setup and remaining limits

New native webhook registrations request `message`, `edited_message`,
`callback_query`, and `message_reaction`, preserving pending updates. Existing
webhooks need an explicit registration refresh before Telegram sends the added
update types. Telegram requires the bot to be a chat administrator and requires
`message_reaction` explicitly in `allowed_updates`; it does not send reactions
set by bots. A production registration change is separate from this code change.

| Input | Status and reason |
| --- | --- |
| Private/group message edit | Supported for a retained durably admitted original, with existing text/media and actor admission |
| Attributable user reaction | Supported with the same original-message requirement and Telegram administrator/update subscription prerequisites |
| `edited_channel_post` | Bot API supports it; current CMA original channel-post/sender-chat admission does not. Refused as `system_sender`; this is a CMA limitation |
| Anonymous `actor_chat` reaction | Bot API supports it; current CMA path requires an attributable human. Refused as `unknown_sender`; this is a CMA limitation |
| `message_reaction_count` | Bot API supports delayed aggregate anonymous counts; current CMA path does not assign these to a fabricated user. Refused as `system_sender`; this is a CMA limitation |
| Generic message deletion | No general Bot API deletion update exists. `deleted_business_messages` belongs to a separate business-connection contract and is not ordinary deletion coverage |
| Message predating retained context, legacy-only ingress, or expired context | No notification authority is inferred; ordinary messaging remains available |
| NyxID Telegram relay | Unchanged; these native webhook extensions do not claim relay support |

The contract was checked against the official [Bot API Update](https://core.telegram.org/bots/api#update),
[MessageReactionUpdated](https://core.telegram.org/bots/api#messagereactionupdated),
and [MessageReactionCountUpdated](https://core.telegram.org/bots/api#messagereactioncountupdated)
definitions captured on 2026-10-26. Local fixtures exercise authenticated webhook
and encrypted-record behavior. No external Telegram send or production webhook
change is part of this implementation.

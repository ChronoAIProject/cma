# Native Discord incoming message changes

CMA can retain native Discord `MESSAGE_UPDATE`, `MESSAGE_DELETE` and
`MESSAGE_DELETE_BULK` dispatches as durable notifications to the consumers that
already received the original message. Issuance is separately default-off through
`CMAEG_DISCORD_MESSAGE_EVENTS_ENABLED=false`. This is independent of outgoing
message-edit/delete tools and card effects.

## Admission and retained authority

An incoming change requires the original message's unexpired native inbox, exact
channel and guild identity, the same source revision and an originally admitted
human message. An explicit author on an edit must match the original author and
must not identify a bot/system sender. Unknown originals, malformed changes,
refused original messages and embed-only updates without an edit revision do not
become ordinary agent messages.

The original routed admission must also be committed before provider work begins.
A change received while that admission is still pending stays in the durable
inbox and retries; there is no direct-launch fallback. A source that never admitted
the original does not gain authority from the later change. Both inbox dispatch
and execution recheck current owner, source, consumer, provider and credential
availability. Changing edited mentions or routing defaults never selects new
consumers.

The shared verified-message-event path reloads the original immutable routing
plan. It keeps every original consumer, provider, partition and conversation ID,
including existing Discord thread evidence. The notification preserves the
original routing resolution as selection provenance and sets addressing bytes to
zero. It is a new event within the original agent conversation, not another
ordinary message or a claim that previous agent side effects were undone.

## Neutral event representation

The existing `ActivityKind::Event` carries a CloudEvent. No new top-level Activity
variant or provider-specific callback protocol is introduced.

- `cma.message.updated.v1`: `subject` is the affected Discord message ID;
  `data` contains `message_id`, `original_event_id`, `original_author` and `patch`.
  The patch includes only supplied `text` and/or normalized `attachments`.
  Event attachments retain Discord message/channel/file identities. The actor is
  the original human author, verified against any supplied author field.
- `cma.message.deleted.v1`: target fields identify the affected original message;
  the actor is explicitly `unknown`. Discord deletion payloads do not expose who
  deleted the message or when. CloudEvent `time` is omitted; Activity receipt time
  is not presented as the platform's deletion timestamp. The original author is
  affected-message context only.

Edit event identity uses bot ID, message ID and the normalized native
`edited_timestamp`. Delete identity uses bot ID and message ID; bulk and single
notifications therefore deduplicate together. Gateway sequence is never a
message revision. A same-revision retry restores the first accepted body instead
of generating another event. Bulk delete is bounded to 100 distinct valid IDs;
only known original targets are retained.

## Durability, rollout and limitations

New records use `discord_message_event_inbox_v1` and a separate event identity
namespace. They never replace the ordinary message inbox or forge a snowflake.
Immutable chunks precede the inbox manifest, and the Gateway checkpoint advances
only after durable custody succeeds. Lost write/ACK responses are recovered by
reading the exact retained record. Dispatch has the existing 30-minute admission
window; retention never exceeds the original inbox's seven-day boundary.

Deploy compatible inbox and routed-plan readers before enabling issuance. Turning
the flag off stops new issuance while retained events remain recoverable. A
binary without the new `native_message_event_payload_v1` reader cannot safely
replace serving readers while such admissions remain retained. The checked-in
configuration remains off; this change does not alter live configuration.

## Native reaction opt-in

The independent `CMAEG_DISCORD_REACTIONS_ENABLED=false` authoring/issuance gate
allows an owner to select reaction intents in the native Discord source editor.
The legacy `37377` intent mask remains unchanged. Guild reactions require
`GUILD_MESSAGE_REACTIONS` (`1024`) with guild messages; direct reactions require
`DIRECT_MESSAGE_REACTIONS` (`8192`) with direct messages. The public authoring
capability is `native_discord_reactions_enabled`. An exact saved source can be
retained with the gate off; new or changed reaction sources require it on.

All four native dispatches are represented by
`cma.message.reaction.updated.v1`: `MESSAGE_REACTION_ADD`,
`MESSAGE_REACTION_REMOVE`, `MESSAGE_REACTION_REMOVE_ALL` and
`MESSAGE_REACTION_REMOVE_EMOJI`. Data includes the affected `message_id`, original
admission `original_event_id`, `target_kind`, `target_author`, `operation` and
`representation: "delta"`. Emoji identity and supplied normal/burst reaction type
are retained where relevant. The event does not claim a complete old or new set
of reactions. Clear-emoji covers the platform's emoji-clear operation, without
inventing a particular reaction type.

Add/remove actors require Discord's actual guild member user object or a bounded
read-only native user lookup for the payload's `user_id`. Bot/system reactions
are refused, including this receiving bot. Sender/chat restrictions are checked
against the current identical source. Mention/reply addressing comes solely from
the original retained message, and never selects new consumers. Clear-all and
clear-emoji have an explicitly unknown actor because Discord provides none;
these are affected-message notifications, not a claim about an authorized sender.
None of these dispatches supplies a producer timestamp. CloudEvent `time` is
absent; Activity `occurred_at` is the immutable first durable intake time.

Reaction delivery identity hashes the native source record, bot, exact source
revision, fenced Gateway session and dispatch sequence. Replayed dispatches retain
one immutable payload; conflicting same-delivery payloads are refused. A later
add/remove/add sequence produces three events. A new Gateway session never
collides with an earlier session's sequence. Raw session identifiers are not
exposed to providers or logs.

Reactions to an admitted inbound message retain all its original consumers.
Reactions to CMA's own reply require a sealed `discord_confirmed_outbound_v1`
receipt for the exact bot, native channel and Discord-returned message ID. Each
confirmed text/media part and confirmed controls/form reply gets its own receipt.
The receipt pins the original admission and exactly the sending consumer,
provider, partition, conversation and source revision. The notification reaches
that one consumer even if the original message was broadcast. No current routing
rule is reevaluated. A confirmed send with an unconfirmed receipt is reported as
unknown and is never resent to repair custody. Replies to prior notifications
can also be reaction targets, provided their original admitted plan is retained.

The original inbox/plan or outbound receipt must remain within its seven-day
retention; reaction dispatch retains the existing 30-minute admission window.
Strict source revision checks deliberately exclude messages captured before
reaction opt-in or before changing a source/credential. Known expired or changed
custody is refused. External bot messages, unconfirmed sends and messages for which no exact receipt was
retained wait as `discord_reaction_pending_v1` raw inbox custody for at most 60
seconds. The Gateway checkpoint advances after that durable custody, without
waiting for target confirmation. The inbox worker retries with persisted 1, 2, 4
then 8-second backoff. Once the exact confirmed receipt appears, the retained
reaction produces one notification in the original consumer conversation with
its original intake time. It does not acquire replacement target authority.
Unresolved external or missing targets terminate with the explicit durable
`discord_reaction_target_confirmation_timed_out` refusal. Changed source custody
is `discord_reaction_target_authority_changed`. Unknown original messages are
never fetched and reinterpreted as a newly admitted message.

Enable only after compatible inbox, routing and confirmed-receipt readers are
available. Disabling issuance keeps retained inboxes dispatchable. Rollback to a
reader that predates these retained shapes is unsupported until their work has
settled and retention has elapsed. No live flag or Discord source was changed by
this implementation.

A missing old inbox, expired original custody, messages that predate native
capture, unsupported system messages and edits after source revocation do not
create replacement conversations. Delete notifications do not cancel already
admitted work or retract provider effects.

## Pinned platform evidence and verification

Official Discord API documentation is pinned at
`discord/discord-api-docs@6128d99116af8357db1e4966ebd7f099817a0e04`:

- `developers/events/gateway-events.mdx`, SHA-256
  `5daa3d532045c8e5365cf8ac97e60d2f79f76acc31132107b8f97f694f6711fd`:
  message update/delete/bulk-delete and reaction event fields.
- `developers/resources/message.mdx`, SHA-256
  `c296fe9263b887f0f3cf840927466826cd4e5c9182aebe6613decc28e5d80611`:
  message identity, edit timestamp and attachment objects.

Local tests cover durable replay, lost inbox/ACK responses, original-admission
ordering, source revocation, bounded expiry, same-revision conflicts, unknown
actors, attachment patches, exact thread retention and issuance-gate behavior.
A composed local provider/Discord REST fixture verifies ordinary input plus edit
and delete produce one provider request each in the original conversation and
reply channel. Reaction fixtures cover every confirmed split text/media reply,
consumer narrowing, failed receipt persistence, bounded unknown-target timeout,
and a reaction arriving before the outbound receipt. The composed fixture then
reacts to the bot's reaction reply, proving the retained Event chain preserves the
original provider conversation and native thread without duplicate work. Real Discord client delivery has not been exercised by this
change; no external messages or deployment are part of its verification.

## Presentation integration

The full provider Activity exposes the explicit CloudEvent type and affected
message fields. The channel activity projection retains optional Activity kind,
CloudEvent type and explicit source occurrence time alongside source, actor,
event ID, conversation and gateway receipt time. It does not copy message bodies
or event payloads into the activity log. The map preview and Activity view show
message-edited and message-deleted labels; deletion actors and source timestamps
remain explicitly unreported when Discord supplies neither. Historical records
without these optional fields retain their generic event presentation. This
projection does not invent a user-facing human message for a change notification.

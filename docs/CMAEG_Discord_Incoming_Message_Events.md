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

Reactions are **not included** in this Discord feature. They need explicit
reaction Gateway intents and trustworthy actor facts for add/remove events;
remove-all and remove-emoji payloads do not identify an actor. Outgoing message
reactions and incoming reactions to CMA's own replies require their own confirmed
receipt authority and are not implied by original incoming-message custody.

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
reply channel. Real Discord client delivery has not been exercised by this
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

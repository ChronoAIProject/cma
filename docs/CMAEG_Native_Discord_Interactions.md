# Native Discord buttons and single-select controls

Native Discord can render neutral `buttons` blocks as Discord buttons and
`choice` blocks with `multi: false` as Discord single-select menus. Controls are
issued only when `CMAEG_DISCORD_INTERACTIONS_ENABLED=true`; the flag defaults to
false. Disabled issuance preserves the existing numbered-text rendering. Named
consumers also require `CMAEG_CONSUMER_INTERACTIONS_ENABLED=true`.

Deploy all serving and rollback readers before enabling issuance. New cards add
an optional native Discord identity, callback inbox bodies add a component
snapshot, and durable callback execution uses
`native_discord_interaction_payload_v1`. Historical card, inbox and ordinary
routing payload serialization is unchanged. Older readers refuse the new stored
callback contracts. Disabling issuance does not disable retained callback reads
or recovery.

## Supported contexts and limits

Controls require a verified human native Discord event, a ready source, durable
routing, and an executable provider binding. A provider's event-context response
must match the sealed event digest before a card can be prepared. Published agent
events and thread contexts are ineligible in this initial implementation.
Before creating a card, delivery verifies the exact channel through one
authenticated Discord channel read with a two-second budget. Only ordinary DM
or server channels matching the verified conversation kind qualify. Missing,
unavailable or thread metadata refuses issuance even when the original event
has no thread marker; this check is independent of the thread metadata gate.

There are at most five action rows, with at most five buttons per row or one
single-select menu per row. Neutral message validation currently bounds a select
to twenty options, below Discord's twenty-five-option limit. Multi-select,
modals, user/role/channel selectors, native attachments combined with controls,
and interactive message edits are not supported. A control message must fit one
ordinary Discord text part. Non-control blocks retain their existing text
rendering and degradation report. Mentions remain suppressed.

Discord `custom_id` fields contain only a versioned card UUID, revision and
action index. Select option values contain only opaque indices. Provider option
IDs and values are resolved from the encrypted card after authorization; client
labels and values cannot supply business action authority.

## Callback identity and acknowledgement

This implementation receives Gateway `INTERACTION_CREATE` events with
interaction `type: 3`. The Discord application's HTTP interactions endpoint must
not be configured to divert callbacks away from the Gateway. This adds no NyxID
callback endpoint and requires no relay changes.

A callback must match all retained identities before it can consume a card:

- The authenticated native source, application/bot ID and current source revision.
- The original human actor, conversation and delivered bot message ID/author.
- The card revision, button or single-select component type, and an exact option.
- The original consumer and provider binding, including the default consumer.
- The card's expiry, confirmed delivery reference and unconsumed state, or the
  exact already-consumed callback during recovery.

Current source admission is applied to the callback's authenticated actor and
conversation. A button click supplies explicit addressing evidence. Credential
rotation or a changed source revokes the retained source revision; removing or
rebinding its consumer also refuses the click.

The Gateway records the consumed click and sanitized native inbox before any
provider execution. It then attempts a type-6 `DEFERRED_UPDATE_MESSAGE`
acknowledgement at the exact official
`/api/v10/interactions/{interaction_id}/{interaction_token}/callback` endpoint.
The request carries no Bot authorization header. The interaction token is removed
from retained inbox bytes and never appears in Activity or provider context.

The acknowledgement budget ends 2.5 seconds after the interaction snowflake's
creation time, leaving margin within Discord's three-second deadline. A new late
click cannot consume fresh authority. An exact click that was consumed before a
crash can finish its inbox write after the deadline. Storage or network delays
can prevent an acknowledgement; durable work remains recoverable. A create-only
acknowledgement claim prevents retries after an uncertain response or process
restart. No message content is sent through this acknowledgement endpoint.

## Durable execution and rollback

Native callback admission is a separate typed entry into the routed scheduler.
It restores one retained consumer, provider binding, conversation and partition;
it does not run default targets, addressing rules or fan-out again. The source
inbox and Activity receipt precede provider execution. Repeated Gateway dispatch
restores the same stored activity and admission, and it cannot fall back to the
legacy direct launcher. Ordinary HTTP routed selection remains unchanged.

No new callback is authorized by an unconfirmed outbound send. If Discord accepts
a control message but its receipt or card binding cannot be confirmed, delivery
is reported as unknown and its card cannot authorize a callback. No automatic
resend follows an uncertain message write or acknowledgement.

To pause new controls, turn off `CMAEG_DISCORD_INTERACTIONS_ENABLED`; retain readers
until existing cards and callback work have expired or settled. Production
activation and live platform proof are separate from the local loopback tests.

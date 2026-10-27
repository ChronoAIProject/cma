# Native Discord card deletion and reactions

The existing `messageEffectCapabilities`, `deleteMessage` and `setMessageReaction`
tools accept a retained native Discord card reference after compatible readers
are deployed and `CMAEG_DISCORD_CARD_EFFECTS_ENABLED=true`. The flag defaults to
false, including the checked-in CMA configuration. REST, MCP, OpenAPI and SDK
request/response shapes are unchanged. No arbitrary Discord channel or message ID
is accepted as effect authority.

The capability request can adopt effect support on an eligible saved card. The
adoption is stored with CAS; an effect can also adopt it on first use. Gate-off
blocks first adoption. It does not revoke retained capability, exact receipt
replay, successor revisions that inherit capability, or accepted callback
recovery. Expiry always remains the original card expiry. Adoption cannot extend
the message lifetime or create an ordinary text-message effect window.

## Authority and replay

The encrypted card and sealed reference select one exact card ID, revision,
platform message, route, event and consumer. The saved actor, conversation,
thread identity, original routing decision, conversation ID, partition key,
provider binding and expiry are preserved. Current owner, agent key, consumer
presence/nonpending credential, provider binding, native source revision and bot
custody are checked before receipt replay and again after durable admission.
Bot-token rotation changes the native card source revision and revokes the card.
Ordinary routing or partition changes never retarget it.

Before new platform dispatch, the bot is authenticated and the current native
channel metadata must match the retained ordinary channel or thread identity,
including thread type, parent and guild. Source/key/provider revocation during
claiming or metadata reads does not authorize a write.

Delete and reaction share a durable idempotency scope derived from immutable card
authority and exact card ID/revision/destination. The scope excludes ciphertext,
current routing, consumed callback and operation state. An exact old request
replays after replacement or deletion, provided current custody remains valid.
A different body or operation with the same key conflicts. A successor has a
separate receipt scope. A fresh request on a superseded/deleted card is refused.
An unfinished or ambiguously acknowledged claim returns Unknown without REST.

## Mutation and callback ordering

Text effects, card effects and card PATCH share the existing encrypted native
message lock, keyed by bot/channel/platform message across revisions and keys.
The destination lock precedes the card CAS. A definite outcome releases it only
after required card state and receipt writes succeed. An unknown transport
outcome, partial reaction replacement, abandoned admission or lost final storage
acknowledgement keeps it held through original expiry. Another key, a successor,
a gateway restart or a timer cannot make the operation safe to resend.

DELETE first records a pending deletion tied to its receipt claim and supersedes
the card in the same CAS. This competes with callback consumption and edit
admission. If delete wins, fresh callbacks and edits are refused. If a callback
wins, delete reloads and preserves that exact consumed event. That callback alone
may finish its original inbox after pending, confirmed or uncertain deletion,
even after the ACK deadline. Different callback IDs, events, actions or canonical
selections do not receive this exception.

Confirmed deletion is terminal; unknown deletion remains fenced. A definite
no-write rejection or rate limit can restore controls only after a successful
final CAS, retaining any already-consumed callback. A failed final CAS returns
Unknown. Reactions never consume or supersede controls and can coexist with
callback consumption while remaining serialized against other message mutations.

The public reaction bound remains zero or one ordinary reaction owned by this
bot. Replacement/clear removes only the bot's own reactions, never another user's
reactions. `is_big` is rejected. Custom emoji retain the existing exact guild
lookup and URL encoding. Successful Discord delete/reaction writes require 204.
No possibly accepted write is retried.

## Reader rollout and validation

Deploy these callback and mutation readers everywhere, including rollback
candidates, before enabling admission. Historical cards omit `discord_effect`
and deserialize unchanged. Older readers ignore unknown fields, older edits do
not share the lock, and older callbacks cannot recover all consumed deleted
cards. Disabling the flag is therefore **not** permission to roll back below the
compatible-reader version. Keep compatible readers until all adopted card
authority has expired.

Local tests exercise encrypted storage, authority revocation, REST/MCP receipts,
CAS/receipt acknowledgement loss, callback recovery and shared mutation fencing.
Live Discord acceptance is a separate activation check. The platform provides no
transaction joining message deletion, callback delivery and record persistence;
a callback already durably accepted cannot be retracted by later deletion.

The official endpoint contract is pinned to
`discord/discord-api-docs@6128d99116af8357db1e4966ebd7f099817a0e04`,
`developers/resources/message.mdx`, SHA-256
`c296fe9263b887f0f3cf840927466826cd4e5c9182aebe6613decc28e5d80611`.
Create/delete-own reaction and single-message deletion have no exclusion for
component-bearing messages. Create reaction requires `READ_MESSAGE_HISTORY`,
and `ADD_REACTIONS` when the emoji is not already present. This bot's own message
deletion does not grant or require authority to moderate other authors' messages.

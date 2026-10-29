# Existing-Agent inbound media continuations

The original event remains one immutable Responses command. Its initial media
preparation budget remains bounded; completing a transcript later must not
resubmit that event or interrupt the Agent's current turn.

## Authority

Gateway attests the exact retained event, original selected consumer/provider,
activity digest and current source authorization. The sealed event reference is
a narrow read-only capability: it is accepted in a POST body, never a URL, and
the response contains only a boolean. Invalid, expired, removed or changed
targets fail closed. No original content, workspace paths or credentials are
returned. Reads never acquire the original response worker's lease.
The request's expected expiry must exactly match the sealed capability's Unix
expiry, so Manager can cap deferred continuation lifetime at that verified bound
without changing the ordinary message queue lifetime.

Agent Manager separately requires the original durably admitted command and
its actual retained binding generation. An omitted Gateway generation is not
generation 1. Existing-Agent ordinary delivery currently omits the Gateway
generation; newly admitted continuations use the actual Manager-verified
generation frozen in the original command. Legacy commands without sufficient
parent provenance receive no continuation and retain ordinary turn behavior.

The original command's full event context is authoritative even when large
input media causes metadata to use the existing context lookup path. A metadata
event_ref is an optimization, never a prerequisite or substitute for that
retained context.

Manager checks current source attestation at admission and again at deferred
dispatch through the trusted configured Gateway origin, with redirects disabled
and bounded requests. It also revalidates the exact binding, generation,
conversation and Agent immediately before dispatch. No human token or persistent
Agent key is copied into the continuation envelope.

## Durable delivery

After freezing the original turn and before its first provider submission,
Gateway persists a sealed active-media scheduling head bound to the exact
event, consumer, frozen request digest and original retention. Failure to commit
this index prevents the original POST. The scheduling hint never grants event,
source or Agent authority; the worker independently requires the original
Accepted/Completed checkpoint and checks current authorization before effects.
The bounded index is separate from ordinary execution history and is removed
only after every attachment has retained terminal delivery or refusal evidence.
Independent recovery continues after the original response reaches Completed.
Original files are fetched with the existing bounded source service and uploaded
using Manager's atomic no-follow attachment publisher into the Agent's persistent
/home/attachments directory. Record storage contains bounded receipts/text, not
file blobs. Upload IDs and supplement command keys derive from the immutable
parent and artifact identity. A repeated identity with changed bytes conflicts.

Gateway freezes the exact original bytes' digest and descriptor before uploading.
On retry it first reads Manager's durable upload receipt, allowing a completed
upload to recover even if the original platform file is no longer retrievable.
Audio/video transcripts carry the digest of the original bytes consumed before
any transcoding. Gateway refuses to bind a transcript to a different upload.
Other files are delivered as originals, allowing the Agent to read them itself.

Manager accepts attachments only through a separate binding-event route, after
proving the parent command. Ordinary owner upload authorization remains unchanged.
The client cannot choose an Agent or filesystem path. Completion freezes a
bounded message containing verified persisted paths and derived transcript text,
then durably queues it with Append ordering and explicit server-side provenance.
An active turn continues; the original event is never replayed.

A supplement's HTTP 202 is only admission. Gateway authenticates the resulting
Responses snapshot against its exact returned provenance metadata and existing
Agent ID, then persists terminal text in bounded sealed chunks and separate
delivery outcomes before sending to the original authorized destination. Native
sends use their own fenced delivery identity. Lost/ambiguous local or webhook
intents remain explicitly unknown and are not automatically resent.

Custom streams preserve the existing `cmaeg.completed` contract for ordinary
events. Only newly frozen media-enabled events use `cmaeg.response.completed`
for the original response, carrying its response ID. Supplementary terminal
results use `cmaeg.media.response` with response ID, parent response ID, artifact,
delivery ID, status and text. One final `cmaeg.completed` closes a fully settled
stream; failures use `cmaeg.failed`. Such streams have a 120-second local bound;
`cmaeg.media.pending` closes a still-pending stream without claiming delivery.
Recovery continues durably after that local deadline or client disconnect.

Owner-authorized channel activity retains separate media response IDs/statuses
and their ordinary Agent Manager response URLs. Following a URL still requires
Agent Manager's existing response authorization. Channel viewers gain no wider
conversation access, and private response text is not copied into activity.
Because recent activity is bounded, the owner can also read
`GET /v1/channels/{channel_id}/events/{event_id}/media-responses`. This paged,
metadata-only endpoint reads retained obligations directly and remains available
after the original event capability expires, for the original retention period.
The new response-scoped Custom controls include this retrieval URL. It uses
the existing creator authentication and channel ownership checks.

Revocation prevents further fetches, uploads and supplemental dispatch. Files
already delivered to the owner's PV are not retroactively deleted. Uncertain
writes retry only the same durable operation; missing recipients, expired
references and changed generations settle safely without redirecting work.

## Release checks

Tests must cover exact parent/digest/consumer binding, missing or expired
references, source removal, generation changes, Custom events, compact metadata,
active-response lease preservation, crashes after upload/command admission,
same-key replay, changed-content conflict, revocation after admission, bounded
payloads and continued active turns. Deploy compatible readers before enabling
new media-obligation writers. `CMAEG_MEDIA_CONTINUATIONS_ENABLED` defaults false
and requires routed recovery, the existing media service and the optional
Gateway-owned Mongo index. A ready index keeps its reader active when the writer
flag is later disabled. No new Manager record API or infra-side development is
required. Production activation requires the complete release checks above;
the attestation primitive alone does not enable media delivery.

Once the first continuation is admitted, retain compatible Agent Manager and
Gateway images as the rollback floor. Manager command envelopes may contain
`continuation`, and command work state may retain `deferred_attempts`; older
strict readers reject these fields. Disabling the Gateway writer flag prevents
new scheduling heads but does not remove accepted work or make those older
Manager readers safe. The optional index keeps recovery running while ready.
Its seven-day retention is independent of Manager command/history retention and
cannot establish when an older Manager becomes safe to restore. Preserve the
records and use a compatible forward fix; see
[Manager storage and rollout](CMA_Event_Media_Continuations.md#storage-and-rollout).

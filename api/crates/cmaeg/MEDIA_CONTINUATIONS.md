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

Before freezing the original turn, Gateway persists sealed media obligations
bound to the exact event, consumer, original request and attachment index.
Independent recovery continues after the original response reaches Completed.
Original files are fetched with the existing bounded source service and uploaded
using Manager's atomic no-follow attachment publisher into the Agent's persistent
/home/attachments directory. Record storage contains bounded receipts/text, not
file blobs. Upload IDs and supplement command keys derive from the immutable
parent and artifact identity. A repeated identity with changed bytes conflicts.

Manager accepts attachments only through a separate binding-event route, after
proving the parent command. Ordinary owner upload authorization remains unchanged.
The client cannot choose an Agent or filesystem path. Completion freezes a
bounded message containing verified persisted paths and derived transcript text,
then durably queues it with Append ordering and explicit server-side provenance.
An active turn continues; the original event is never replayed.

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
new media-obligation writers. This document describes the intended implementation;
the attestation primitive alone does not enable media delivery.

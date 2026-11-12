# Bound rejected runtime correlation IDs

Inbound permission and subscription-credential calls accept scalar correlation
IDs only when their JSON encoding fits the existing 256-byte limit. The legacy
predicate serializes the whole value before checking its length. Permission
handling also clones the ID first, even when its type or size is invalid.

With `CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED=true`, the predicate runs the
pinned JSON serializer into a fixed 256-byte stack buffer through `std::io::Cursor`.
The buffer cannot grow. Scalar type checks precede serialization, and buffer
exhaustion refuses the value. Using the same serializer preserves exact quote,
escape, UTF-8 and numeric formatting behavior. Permission handling then copies
only an accepted ID. The disabled path keeps its original selection and encoding;
the independent restored-locator predicate is unchanged.

The policy remains false by default and OFF on Thursday's train. No new limit,
pool or activation is introduced. A valid permission still produces its existing
passively scoped domain request, and a valid subscription call keeps its exact
response ID. Invalid IDs produce no domain request or subscription response.
The existing protocol-only retained receipt is preserved; dropping it cannot
fabricate a durable ACK.

## Qualification and limits

Contracts cover exact encoded boundaries, mixed Unicode/control/quote/backslash
escapes, numbers, invalid containers, missing IDs and maximal 4 MiB rejected
fixtures. Allocation checks include the fixed serializer failure cost and verify
zero final tracked bytes. Valid boundary IDs copy only their selected payload.
Retained-flow contracts cover permission publication/refusal, subscription reply
identity, disabled behavior and absence of fabricated ACKs. Local gate results
and allocation values are recorded after qualification.

This change covers selected inbound rejection work. General typed permission
decoding, accepted response construction, codec/write buffers, restored-locator
working storage, source/ACK/correlation lifetimes, desktop and durable working
copies remain separate owners under #1702. Component maxima remain 117 MiB;
complete aggregate coverage, useful capacity and process RSS remain unqualified.

Rollback follows the existing JSON-retention policy for new attachments. No
production activation, traffic, Sandbox lifecycle or model turns occur here.

Part of #1472. Implementation child #1738 under #1702.

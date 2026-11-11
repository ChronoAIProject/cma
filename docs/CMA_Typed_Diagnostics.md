# Typed runtime decoding diagnostics

When `CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED=true`, session updates,
permission requests and terminal response values use a borrowed value adapter
whose Serde error type does not format untrusted values. A large invalid
discriminator or scalar therefore cannot become an independent diagnostic
String. This also applies to errors discarded by tolerant schema fields.
The existing policy is false by default and remains OFF on the Thursday
13:00 train. Attachments use their pinned policy.

The adapter reuses Serde's map, sequence and enum access helpers over the
already admitted JSON tree. It supports the pinned protocol's string-keyed
maps and preserves its owned schema outputs, metadata, options, numeric types,
enum representations and tolerant-field behavior. It is an internal adapter
for those three typed entry points. The disabled path invokes the original
JSON value deserializer and retains its existing detailed diagnostics.

Enabled decoding errors have a fixed message: `runtime value does not match
the supported schema`. Existing caller behavior remains: unsupported updates
are ignored, unreadable permissions are refused, and malformed terminal
results produce the existing generic failure. There is no new ACK, source
cursor, retry or capacity-refusal policy in this slice.

## Qualification and remaining scope

Equivalence tests compare successful values and failure behavior against the
legacy decoder, including content, tool metadata, nullable fields, enums,
permission shapes and terminal usage. Allocation fixtures cover a maximal
admitted invalid discriminator and a large invalid field whose diagnostic is
discarded by schema fallback. Final measurements are recorded after gates pass.

This bounds the construction of Serde diagnostics for the specified paths.
Valid typed values and intermediate schema containers still allocate storage;
their pre-allocation admission and ownership remain separate work. Handwritten
schema code that preformats a message before invoking Serde's error trait would
also require its own audit. General typed/prepared/session/context ownership,
protocol responses, codec/fragment buffers, desktop, durable assembly, independent
working copies and full ingress/RSS qualification remain open under #1702.

Rollback follows the existing JSON-retention policy for new attachments. No
production activation or campaign is performed by this source change.

Part of #1472. Implementation child #1729 under #1702.

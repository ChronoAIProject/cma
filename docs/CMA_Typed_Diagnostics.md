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
discarded by schema fallback. All 655 focused tests pass, including existing
startup, permission, recovery, retention and recorder regressions. Workspace
formatting, strict all-target/all-feature CMA/core/adapter clippy, source-size,
configuration/documentation and secret-history checks also pass.

The qualified source is Git commit `6bdb1867fdf181fea63ce6952c463242a8bdee37`;
its API tree is `766674882bf72bf3a600b0e8cfe420834bc7f1fa`.
Requested allocation measurements on aarch64-apple-darwin with Rust 1.96.1:

| Admitted fixture | Encoded bytes | Legacy peak | Enabled peak | Enabled retained |
| --- | ---: | ---: | ---: | ---: |
| Invalid discriminator, three JSON nodes | 4,193,920 | 12,582,115 | 64 | 0 |
| Invalid location line discarded by fallback | 4,188,112 | 3,034,896 | 747 | 459 |

Both paths return to zero measured outstanding bytes after the result drops.
The second enabled result retains its valid tool-call fields; the invalid line
does not become an owned diagnostic. These measurements exclude allocator/OS
overhead and RSS. The first run rejected an inadequate discriminator fixture:
Serde formats unknown variant names directly, so escaped wire length did not
establish a large decoded diagnostic. The final near-limit plain-text fixture
passes the unchanged legacy and enabled allocation assertions.

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

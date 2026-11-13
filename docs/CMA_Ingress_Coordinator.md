# Shared ingress reservation coordinator

`CHRONO_INGRESS_COORDINATOR_ENABLED` is a strict boolean, disabled by default.
It connects existing admitted storage owners to one process-wide 128 MiB
reservation pool while preserving their individual component limits:

| Covered owner | Component limit | Reservation lifetime |
| --- | ---: | --- |
| Copied retained raw payload and queue node | 16 MiB | Through startup/capture/delivery or final queue teardown |
| Parsed JSON and RPC reply trees | 32 MiB | Through the last immutable tree/result owner |
| Parser and preflight scratch | 13 MiB | Before preflight through the end of owned parsing |
| Permission table and retained replay entries | 8 MiB | Table teardown and final in-flight replay owner |
| Retained live fragments | 32 MiB | Last subscriber, deferred delivery or retained source clone |
| Sign-in notice strings and immutable envelope | 8 MiB | Through early bootstrap delivery and the final effect owner |

Enabling it requires `CHRONO_RUNTIME_RAW_OUTPUT_BOUNDS_ENABLED`,
`CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED`,
`CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED` and
`CHRONO_AGENT_LIVE_BUFFER_BUDGET_ENABLED` to be true. Invalid combinations refuse
configuration. Native and provider transports pin the setting over renderer
preferences; attachment opening independently rejects missing adapter policies
before reading Ready. Existing retained-output capability requirements remain.

Each admission acquires its component reservation and then the aggregate
reservation without waiting. Failure returns every partial reservation before
the caller sees refusal. Parsed scratch, trees and raw storage may coexist,
but no task queues for a capacity upgrade while retaining another allowance.
The original owners keep both reservations through their existing lifetimes.
Public live delivery uses the same coordinator as adapter storage. Component
refusal/error contracts, original source cursors, payload hashes, durable ACKs,
replay semantics and disabled behavior are preserved.

Durable acknowledgements do not acquire a data reservation. The socket contract
exhausts the aggregate allowance, verifies data refusal before cursor/receipt
publication, releases pressure for an exact retry, then exhausts it again while
the accepted source ACK progresses. No pressure path fabricates an ACK or terminal.

## Scope and qualification

This pool bounds reservations for the named owners. It does not establish a
complete ingress heap or process RSS limit. The existing component maxima total
117 MiB including the later sign-in notice and metadata/usage components; the
shared coordinator becomes necessary as further ownership boundaries
join the same envelope. Unwired storage must not be represented as charged.

Remaining owners under #1702 include typed domain/prepared effects, codec and
fragment buffers, desktop forwarding, durable assemblies, independently allocated
fold/serialization work, channel/table bookkeeping, allocator metadata and
operating-system buffers. Temporary live fragments already exist before retained
admission. Those producers and final-candidate RSS remain separate qualification.
The public expensive-read envelope is also separate. No conclusion about total
128 MiB ingress or RSS at most 70% follows from this foundation alone.

Contracts cover combined and component pressure, rollback of partial admission,
concurrent owners, cancellation, final clone release, parser scratch recovery,
permission replay retention, live-fragment competition, trusted transport policy
and strict configuration. Run the existing allocation fixtures after any change
to reservation layout or parser representation; requested bytes are not RSS.

## Allocation regression receipt

The reservation layout was remeasured on aarch64-apple-darwin with Rust 1.96.1
at API tree `04265438f2e2c042926b7e4a2ec373c198da9928` (source
`c09f0f0bf3d41d84160a266d5a67a0754a9e7d3a`). The seven existing parsed JSON
fixtures all fit their derived tree/scratch reservations and returned measured
frame allocations to zero after the final owner was dropped. The combined suite
passed 205 focused tests. Values below are requested allocation bytes, excluding
allocator/OS overhead; they do not qualify the still-missing ingress owners or RSS.

| Fixture | Retained requested bytes | Peak requested bytes | Tree allowance | Scratch allowance |
| --- | ---: | ---: | ---: | ---: |
| max-string | 4194438 | 4194438 | 8390656 | 12583936 |
| max-escaped | 2097287 | 4194303 | 8390656 | 12583936 |
| small-objects | 636262 | 636262 | 7034530 | 66547 |
| wide-map | 834614 | 1205174 | 8460100 | 108262 |
| wide-array | 589960 | 884736 | 8422398 | 50173 |
| nested | 18568 | 18568 | 67842 | 1411 |
| duplicates | 496 | 496 | 8527840 | 209872 |

## Rollout and rollback

Keep this policy and its dependencies off on the Thursday 13:00 train. Activation
requires a separately reviewed complete ownership inventory, useful-capacity and
control-progress evidence, runtime compatibility and a coordinated release slot.
Record source, immutable images, effective configuration on every process and
the attachment cohort. Old attachments retain their original policy, so a mixed
cohort is not aggregate enforcement. Do not force a Sandbox restart for this flag.

Rollback disables only this coordinator for new attachments and restores a
compatible configuration. Existing reservations stay attached to their owners
until actual release. Component policies retain their own rollout and rollback
contracts. Production campaigns remain through the reviewed acceptance harness
with the existing account, visitor, pacing, quiet-hour and teardown restrictions.

Part of #1472. Implementation child #1721 under #1702.

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
101 MiB; the shared coordinator becomes necessary as further ownership boundaries
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

# Retained permission state bounds

`CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED` is a strict boolean, default `false`
in code and the API ConfigMap. The native and retained-provider transports set
this policy after renderer preferences. Enabled attachments require the retained
output protocol before starting the ACP handshake.

The enabled path keeps compact permission records in a shared process-wide
8 MiB reservation pool. Each attachment admits at most 128 records across its
lifetime, including pending and answered records from earlier turns. Each record
accepts at most 64 options with identifiers at most 256 UTF-8 bytes; correlation
IDs retain the existing 256-byte encoded limit. Durable locator restoration has
a 128 KiB input ceiling. The legacy representation and limits remain when OFF.

Admission reserves space before allocating the retained table or record. The
reservation includes the fixed table capacity, record layout, compact option
array, copied identifiers/correlation and a conservative 128-byte layout allowance
per table/record. Option display names and arbitrary metadata are not retained in
this table. Immutable record ownership is shared with in-flight answer sends;
the reservation releases only when its final owner drops. Reservations describe
requested storage plus layout allowance, not measured allocator consumption or
RSS. The original request/domain update remains separately owned and unaccounted
by this component.

The first selected answer is immutable. A timeout, failed flush, cancellation,
or retry with a conflicting decision cannot remove or replace it. Replaying an
existing record does not require a new record slot. Prompt scopes prevent reuse
of a previous turn's answer when a runtime reuses its permission RPC identifier;
retries of the same prompt keep their original scope. A terminal for a different
active prompt does not rotate the permission scope. Scope changes keep the old
records reserved; they never evict uncertain state to admit more work.

New-state pressure returns a typed `-32002` runtime protocol refusal and stops the
attachment before publishing an approval or retained source receipt. The refusal
notification has a one-second deadline, including a blocked sink; notification
failure does not admit the frame. Oversized
permission shapes follow the same path. There is no synthesized source ACK,
durable terminal, or retry loop. The runtime retains the unacknowledged source
for supported recovery. An already admitted durable answer can still replay at
capacity; a new restored route must obtain a reservation before sending.

Release owners enable this policy in a separate reviewed API rollout. Existing
attachments keep their original policy. A refused attachment needs an explicitly
supported fresh attachment after capacity is available, or a compatible flag-OFF
rollout. This does not erase the runtime journal, rewrite a durable approval, or
repeat an uncertain external effect. Long-lived attachments that reach 128
records require this recovery boundary; there is no automatic eviction.

This implements #1708, Part of #1472 and #1702. It does not establish the complete
128 MiB ingress ceiling or RSS qualification. Socket/fragment buffers, original
frames, JSON decode and answer serialization/write scratch, decoded domain
updates, startup queues, durable assemblies and public projections need their
separate ownership accounting. The final aggregate equation must include this
8 MiB component rather than granting every component the whole ingress ceiling.

Validation covers shared capacity across attachments, maximal option shapes,
metadata compaction, final-owner release, retained-frame refusal without receipt
or ACK, disabled compatibility, actual transport policy override, valid locator
restoration, wrong-session refusal, a delivered answer with a reported flush
failure, cancellation at a deterministic send barrier and conflicting retries.

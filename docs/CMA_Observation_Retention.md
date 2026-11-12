# Runtime metadata and usage storage

With `CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED=true`, selected runtime
metadata and usage strings reserve storage before their independent effect is
constructed or the retained output cursor is accepted. The policy remains false
by default and OFF on Thursday's 13:00 train. This slice follows the context-goal
frame ownership change and uses the existing immutable `RuntimeOutputUpdate`.

The borrowed selection preserves metadata precedence, exact description input
and its position, source classification, working directory, native usage counters
and turn identity. Missing or invalid usage counters produce no usage effect.
Unknown source and position selectors are inspected without string copies;
input evidence with an invalid position is never copied. Disabled attachments
continue through the original observation decoder.

The reservation is the sum of selected UTF-8 string lengths and the explicit
layouts of `ConversationUpdate`, `IngressReservation` and their two Arc headers.
Selected strings are constructed once, at their exact lengths, after admission.
The immutable effect and its reservation remain inseparable through startup,
capture, routing, relay delivery and all effect clones. A source receipt does not
own the effect. Final effect release frees its strings before returning permits.
This covers requested allocation layouts, excluding allocator and OS overhead.

An independent 8 MiB process component admits at least one maximal 4 MiB frame's
selected strings plus its envelope. Observation pressure cannot consume the
separate 8 MiB sign-in component. When the common ingress coordinator is enabled,
both reservations are acquired without waiting; partial acquisition rolls back.
The shared ceiling can still refuse either component. Sign-in independence does
not imply guaranteed progress under complete shared-budget exhaustion.

Retained admission refuses before cursor advancement, effect publication or ACK.
Release permits an exact-cursor retry. Startup stores the already constructed
owner and transfers it unchanged at activation; abandoned startup releases it at
actual final bridge teardown. Retained delivery preserves ownership even with
general shared-output disabled. Foreign conversation rejection and passive
prompt scoping remain in the existing dispatch path. Metadata/usage carry no
prompt-completion evidence.

## Qualification and limits

Success/failure contracts cover decoder equivalence, malformed fields, ignored
selectors, local/shared pressure, partial rollback, exact retry, startup transfer,
two attachments sharing one component, cancellation, disabled behavior, foreign
conversation rejection and final-clone release. Allocation fixtures include
maximal 4 MiB frames and large selectors/input discarded by the legacy decoder.
Local gate results and measured values are recorded after qualification.

The coordinated component maxima now total 117 MiB: raw 16 MiB, permission 8 MiB,
live fragments 32 MiB, parsed trees 32 MiB, JSON scratch 13 MiB, sign-in 8 MiB and
metadata/usage 8 MiB. This sum neither establishes complete ingress/RSS coverage
nor proves useful attachment capacity. General typed schema and container
storage, correlation/reply owners, codec/fragment high-water storage, desktop
forwarding, durable read/fold/publication/serialization copies and accounting
infrastructure remain open under #1702. Final-candidate aggregate and RSS
acceptance remain separate work under #1472.

Rollback uses the existing JSON-retention policy for new attachments. Existing
owners keep reservations until their actual release. This source change neither
activates production policy nor runs a production acceptance campaign.

Closes implementation child #1733 when merged. Part of #1472.

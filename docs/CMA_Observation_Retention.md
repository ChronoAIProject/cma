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
Local qualification passed 672 focused tests, workspace formatting, strict
all-target/all-feature CMA/core/adapter clippy, source-size, configuration/docs,
whitespace and secret-history checks. All changed Rust files are below 500 lines.
The first gate was deliberately interrupted during dependency compilation to
correct a test's queue accessor; no failure was hidden or assertion relaxed.

Qualified source and API tree, using Rust 1.96.1 on aarch64-apple-darwin:

```text
revision c8e56915af012ee72d67f021c7ab1385a1a3c566
tree 869e8a1eb776f859863d23bf1726800aba8a772b
```

| Fixture | Encoded bytes | Reservation | Peak / retained bytes | Final bytes |
| --- | ---: | ---: | ---: | ---: |
| Maximal metadata | 4,194,304 | 4,194,625 | 4,194,625 | 0 |
| Maximal usage | 4,194,304 | 4,194,591 | 4,194,591 | 0 |

Large ignored source/input fields produce a 2,097,163-byte legacy transient peak;
the enabled selection uses only the 560-byte effect/reservation envelope. Both
return to zero tracked bytes. Local/shared refusal of a selected 1 MiB field
stays below 1 KiB and restores the reservation pools. Tests initialize pool
bookkeeping before selected-effect measurement; process infrastructure remains
a separate owner. Clones allocate no additional effect storage, and final drops
restore both component and shared capacity. These values measure requested
allocations rather than process RSS.

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

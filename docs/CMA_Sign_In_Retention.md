# Runtime sign-in notice storage

With `CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED=true`, a runtime sign-in
notice reserves its selected URL, user code and message strings before copying
them from the parsed frame. This uses an 8 MiB process component pool and joins
the common ingress coordinator when that separate flag is enabled. Both policies
remain false by default and OFF on the Thursday 13:00 train.

The allowance is the sum of the selected UTF-8 string lengths plus the explicit
layouts of `ConversationUpdate`, `IngressReservation` and the two Arc headers.
It counts the selected `userCode`, or `code` fallback, exactly once. No string
capacity growth follows construction; clones share the immutable allocation.
Allocator metadata and OS overhead remain part of separate RSS qualification.

Retained-output admission happens before cursor advancement. A refused notice
produces no effect, sign-in acceptance or durable ACK. Normal admission preserves
the source cursor and existing protocol response. Bootstrap notices still reach
the startup recorder before session activation, with the storage reservation
attached to the notice itself. The later protocol-only source receipt does not
own or release that notice. Capture, relay and cancellation retain or release the
same storage owner; only the final effect clone releases the string storage.

The wrapper is transparent to domain classification. Startup publication keeps
the original effect alive while publishing its existing durable notice copy.
Disabled attachments retain their original unwrapped notice and move behavior.
The selected fields, empty values, invalid URL types, field precedence, exact
retry fingerprint and prompt authority remain unchanged.

## Qualification and remaining scope

Local qualification covers allowed and refused admissions, local/shared rollback,
cursor-preserving retry, active and bootstrap notices, disabled and malformed
input, blocked-delivery cancellation and final-clone release. A maximal admitted
string fixture measures requested allocation against the explicit layout
allowance. Existing startup tests also exercise the transparent wrapper on
successful authentication and timeout. The combined local gate passed 650 focused
tests, workspace formatting, strict all-target/all-feature CMA/core/adapter
clippy, source-size, configuration/documentation, whitespace and history scans.
Measurements use Rust 1.96.1 on aarch64-apple-darwin. The qualified identifiers
are recorded below; subsequent documentation changes preserve the compiled tree.

```text
revision c50151a2cbb0a4e460f03240d2eb85c661957287
tree 3653d5a2f5fd671c31e9f707f20ace3c53961888
```

| Maximal notice fixture | Requested bytes |
| --- | ---: |
| Encoded outer frame | 4,193,921 |
| Reservation | 4,194,387 |
| Peak and retained effect | 4,194,387 |
| After final effect clone drops | 0 |

The refusal fixture keeps its peak below 1 KiB while refusing a 1 MiB URL,
verifies both pools recover their capacity, and returns all tracked bytes after
the actual shared pool owners drop. On macOS, the first rollback can initialize
64 bytes in a pool's wait-list mutex; that allocation follows the pool lifetime.
The initial lifetime assertion failure is preserved separately from the final
passing qualification. These measurements are requested allocations, not RSS.

This slice covers the adapter's independently owned sign-in notice strings and
their immutable envelope. It does not account general typed schema decoding,
pre-activation session updates, retained context/correlation copies, outbound
protocol responses, durable publication/read/fold/serialization copies, codec
buffers, desktop or assemblies. Those remain open under #1702. Together with
the five previously coordinated components, component maxima total 109 MiB;
that sum is not complete ingress or RSS qualification.

Rollback disables the existing JSON-retention flag for new attachments under its
compatible-runtime procedure. No production activation, Sandbox lifecycle or
acceptance campaign is performed by this source change.

Part of #1472. Implementation child #1727 under #1702.

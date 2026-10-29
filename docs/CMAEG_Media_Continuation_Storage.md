# Media continuation scheduling storage

The optional `eg_media_continuation_records` collection stores sealed active heads for a media continuation worker. It is a scheduling index: a head is never evidence that the original provider request was admitted, and it cannot authorize a new delivery. The worker must check the existing durable admission, original event, and delivery ledger before acting. This storage change does not replay or rewrite an existing provider journal.

`RecordClient::media_continuations()` selects this collection. Writes use the existing owner-immutable, revision-checked record operations. An original plan's `retained_until` supplies expiry, bounded to seven days; a later progress update must retain that exact expiry. Capability expiry and event expiry must not replace it. Once a durable terminal ledger is present, delete the scheduling head with its expected revision. Expired heads are immediately invisible to reads and scans, independently of the Mongo TTL monitor.

Scans use ascending `_id` keysets, an optional exact owner filter, a maximum of 32 records per page, explicit index hints and a two-second server query deadline. Only record heads are projected, avoiding reads of sealed payload blobs during scheduling. Workers must restart their scan after reaching the end, because an inserted UUID may sort behind a previous cursor. Existing heads and cursors survive process restarts.

## Optional migration and readiness

Base Gateway startup and base schema version 2 remain unchanged. New readers recognize the optional collection privilege without requiring it. Deploy compatible Gateway readers before adding optional privileges; older readers in isolated authority mode do not recognize the new collection and can reject those additional grants. Shared-schema readers continue using their existing effective authority, but optional readiness still checks the exact required rights and indexes.

Review `--print-media-continuations-runtime-role` and `--print-media-continuations-migration-role`. The runtime role permits only find, insert, update, remove and listIndexes on this collection; it does not allow schema changes or access to Manager collections. The migration role creates and validates the optional collection and updates only the shared Gateway migration markers. In shared-schema mode, existing credentials may already cover the required permissions; new named roles are not mandatory.

Run the ordinary migration first, then `--migrate-media-continuations` with migration authority. The optional migration installs a strict record validator, exact `_id_`, `owner_1__id_1` and full `expires_at_1` TTL indexes, and the separate `media_continuations` version-1 marker. It preserves the base, thread-fence and WhatsApp Forms markers. It refuses conflicting indexes rather than dropping or rewriting them. Rerunning a successful migration is safe.

`media_continuations_ready()` checks privileges, marker and all three exact indexes within two seconds. Missing grants, partial migration, hidden/sparse/partial indexes or disabled TTL keep this optional lane unavailable while ordinary Gateway startup remains usable. Production requires direct Gateway Mongo storage. The HTTP path `/api/v1/gateway/media-continuation-records` exists only in explicit test fixtures and must not be used as a production Manager transport.

This source change does not migrate production or enable a worker. Before activation, verify exact permissions and readiness with compatible readers and perform an isolated Custom channel restart test. Keep the existing provider journal and terminal ledger through rollback; disabling the new worker must not delete tenant history or trigger original-message replay. Rollback to an older isolated reader requires reviewing the optional role compatibility first.

## Verification

The authenticated disposable Mongo contract tests base startup without the optional role, denied optional writes, optional migration through its limited role, exact readiness, immutable expiry, owner/version fences, restart pagination, immediate expired-head invisibility, terminal CAS deletion, foreign collection and schema-mutation denial, optional marker coexistence, and absent/partial TTL rejection. Unit tests cover production HTTP refusal and malformed index layouts; CLI tests cover safe configuration diagnostics and exact role output. These tests use local disposable databases and never production credentials.

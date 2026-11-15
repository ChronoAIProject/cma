# Usage allocation interval index

Migration `2026-11-16-am-usage-interval-index` is the additive index portion of
#2140 / #2143. Agent Manager owns `sandbox_usage_events`; the release operator
owns batch 13 activation. The PR's committed tree and the operator's immutable
image receipt identify the executable. #2136 changes scope routing separately.

The existing `MongoSandboxStore::open_with_storage` startup path calls
`sandbox_usage::ensure_indexes`. After its existing indexes succeed, the new
step inventories at most 64 indexes and ensures the ordered key
`{sandbox_id:1, ended_at:1, started_at:1, revision:1}`. New creation uses the name
`sandbox_usage_events_interval`. A complete equivalent index with another name
is retained. The index is nonunique, nonsparse, visible, and has no partial
filter, TTL, or custom collation. Conflicting names, keys, options, unfinished
builds, duplicate matches, or an incomplete inventory refuse startup. No existing
index is modified or removed. The standalone migration CLI is not required.

This is an expand-only change. Usage event schema version 1, records, IDs,
revision order, billing delivery, owners, and runtime homes do not change. Old
readers and writers remain compatible; there is no new record writer, backfill,
rollup activation, or contraction step. Existing revision, owner and billing
indexes remain available to retained executables. Recovery retains the additive
index when rolling back the executable; deleting it is not part of rollback.
No data restore or destructive backup boundary is introduced.

The startup operation uses the existing Agent Manager schema client and its
collection authority. Inventory commands have a two-second server bound; index
creation has a 45-second server bound and majority write concern with a
five-second acknowledgement bound. The whole added step has a 50-second wall
bound, including inventory and postcondition readback. This does not bound the
other existing startup steps. The release must verify the allowed startup
window and index readiness before replacing a serving replica.

Concurrent processes request the identical additive specification. MongoDB
coordinates that index creation; there is no new application migration lease.
A process that loses its connection or times out may leave an index build in
progress. It does not claim success, drop anything, or rebuild a different
specification. A later startup inspects the durable state and either accepts the
complete equivalent index, requests the same missing index, or refuses drift.
An in-progress ambiguous inventory is a refusal, not permission to repair it.

Focused tests exercise the actual store-open path, repeated open, preserved old
indexes, differently named equivalent indexes, concurrent ensure after a partial
startup, and refusal of unique/sparse/hidden/partial/collated alternatives without
repair. The disposable Mongo lane supports `CHRONO_TEST_ALLOCATION_DB_01` for a
serial, authority-scoped fixture; otherwise tests create a unique disposable DB.
The ordinary readers remain unhinted: presence alone does not prove the query
planner uses this index or that complete reports meet two seconds. The integrated
hotfix qualification separately records six personal/admin window cases, complete
values, cold/warm cache state, maximum operation/connection-hold time and RSS.

Production index presence, startup execution and customer report completion are
unverified until the release operator records the exact image, deployment,
readback and UI results. Private receipts stay private; public validation links
carry hashes. No manual Mongo command or Secret access is authorized by this
record.

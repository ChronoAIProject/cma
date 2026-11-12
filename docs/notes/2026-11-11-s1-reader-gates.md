# Reader foundation executable checkpoint

Source tested: `bb6b4f80171533e079641d64859854c3ca6c25c7`, on
`tl/cma-log-segments/reader-foundation`. Part of #1348. This checkpoint supplies
the scoped capture implementation to the publication and recovery slices.
It does not enable segmented writers or claim production verification.

## Passing executable gates

- Eight focused codec and bounded-audit-batch unit tests passed.
- All 36 focused real-Mongo reader, capture, audit, schema and retention tests
  passed on the same source. The Mongo execution took about 112 seconds.
- The canonical disposable replica-set image and cache limit were retained.
  The run held the existing shared heavy lock, used two Cargo jobs and reused
  one target directory. Both disposable fixtures from the initial run and the
  corrected rerun were confirmed removed, including their volumes.

The test selection used `cargo nextest run --locked -p cma --bin cma` with the
codec/audit batch prefixes, then ignored tests under
`store::agent_log_segments::mongo_tests::`, serially, through a local adapter
of the canonical Mongo fixture runner. The adapter retained the pinned image,
resource/cache limits, replica-set startup and ownership-checked cleanup.

## Captured source contract exercised

`capture_transaction`, `points_transaction`, and
`CapturedLog::validate_transaction` use the exclusive `TransactionScope` and
its private per-transaction stamp. Tests prove both legacy and segmented
capture, original source bytes, independent generation maxima, empty active
tails, and fixed source work as sealed history grows.

Refusal cases include another session or database, a later transaction on the
same session after commit or abort, snapshot-to-transaction promotion, changed
Agent/head state in the same transaction, missing projection coverage, durable
quarantine, invalid point requests and excessive returned source bytes. Raw
`CaptureMode::Transaction` remains unavailable.

The first Mongo run exposed a real snapshot-context defect: forcing snapshot
read concern can populate the driver's timestamp on an ordinary session.
Capture now obtains the initial row without forcing that concern and requires
the server's snapshot timestamp. Tests reject an ordinary session, a primed
ordinary session, and an active transaction on the snapshot API. Existing
snapshot-coherence checks still pass. Fixture database names were also bounded
without removing their random identities or weakening any assertion.

## Bounded audit evidence

The original near-capacity source cases passed without increasing Mongo's cache
or reducing their input. The measured values below count source rows and all
observed find/getMore commands during the audit unit; writes are grouped by
transaction.

| Source case | Source rows | Read commands | Write transactions | Largest write row count | Largest encoded write commands | Audit elapsed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Many small messages | 10000 | 165 | 236 | 129 | 143353 bytes | 16651 ms |
| Large messages | 67 | 13 | 3 | 66 | 74670 bytes | 701 ms |

The whole Mongo test command reported maximum resident size of 490668032 bytes.
That includes test/fixture construction and launcher overhead; it is not a
per-request memory measurement and excludes the Mongo daemon.

Restart cases cover partial collection, interrupted promotion with no source
rescan, a committed batch whose acknowledgment stays unknown, cancellation,
stale holders, malformed durable progress and an invalid write that must roll
back evidence with progress. Movement cases cover a later active append and a
newly sealed former tail, including corruption refusal. Installation requires
an exact complete Agent/head capture and leaves original event bytes intact.

## Remaining integration and release gates

This is an executable source checkpoint, not a review-ready PR. Strict Cma
Clippy remains blocked by unwired production entry points; warnings were not
suppressed. The remaining integration work includes migration/authority/backup
registration, supported audit maintenance entry points, and the point/range/
predecessor store delegations. The integration branch has published Mongo CI
selection at `d6368d023`; that registration is not yet part of this branch.

The predecessor executable qualification subsequently passed using the harness
at `77df688f3` and original production source
`0932e58cb6c821c4cdc69f2cb5d82fe538b8c2b6`. It proved empty legacy initialization,
exact append/read success, v2 read/append refusal and unchanged original source
bytes after refusal. Only the test module was injected; source, documentation
and manifests came from the same historical revision. Its disposable replica
set and volume were confirmed absent after completion. The actual permitted
rollback artifact still requires the release compatibility gate. Required integrated consumer/writer tests and
release-owner verification remain open. No production calls or mutations were
performed by this qualification.

## Follow-up source checkpoint

At `40af6d461`, thirteen focused codec, audit-batch, parser and shared-output
unit tests passed. The expanded Mongo selection passed thirty-six cases and
failed four: the output rebuild exceeded the unchanged audit time bound, and
three audit assertions omitted the new output records or expected an error
instead of the durable corruption disposition.

The corrective source at `e85e5de1f` sends each fenced evidence batch as one
bounded update command, requires a complete acknowledgment without embedded
write errors, and asserts exact per-kind output evidence and durable corruption.
It preserves the original source/cache, transaction, query and time limits.
Production transaction capture uses only `TransactionScope`; the invalid raw
mode remains solely in negative qualification tests.

Checkpoint `8b81db391` normally consumes the narrow S4 keyed-publication and
bounded-read delegation at `072ab7465`. Existing scaling tests now emit aggregate
source rows/bytes, read commands and timings without changing their assertions.
Formatting, repository size, all touched Rust files below the sprint line limit,
and full candidate-history secret scanning passed at this checkpoint.

## Corrective qualification completed

At source `41ef81cabe437c72cacaa02f7a7bba73c9b14dc1`, all 14 selected
codec/audit/parser/output unit tests and all 40 real-Mongo reader/audit/capture/
schema/retention tests passed. The four earlier failures are resolved without
changing the original input, Mongo cache, query or audit time limits. The Mongo
test execution took 122.574 seconds; all owned containers and volumes were
verified absent afterward.

| Source case | Source rows | Read commands | Write transactions | Largest write row count | Largest encoded write commands | Audit elapsed |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Many small messages | 10000 | 184 | 392 | 129 | 149614 bytes | 12686 ms |
| Large messages | 67 | 14 | 5 | 129 | 124191 bytes | 667 ms |

At 1/10/100/1000 sealed segments, the fixed sealed-page request read two source
rows and 619 bytes, using 7/8/8/8 read commands. Captured point requests used two
source reads, five rows and 1465 bytes at each size. These are the existing fixed
small-page cases, not certification of every public page size or lifetime
consumer. The whole Mongo command maximum resident size was 424460288 bytes;
that includes test/fixture and launcher overhead and excludes the Mongo daemon.

Strict CMA Clippy at that source failed with 66 production-target diagnostics
and nine test-target diagnostics, all unused code/imports and no type errors.
That result preceded the operator/migration dependency below.

## Recovery schema and canonical migration qualification

Source `8ba9d55215b1eeb9dd56f73e27817dc2f967266a` normally merges the narrow
operator/migration pin `b11107979231ec0da5683d8719722c8b4dfea15e` and recovery
schema/index pin `daf605ab410fc4737633aa3997e67192d4c4112b`. It registers the
format-2 recovery carrier in the canonical collection inventory, loads both exact
JSON definitions, and checksums those sources together with the schema children
and ordered inventory. Existing unique indexes retain their exact contract;
each of the three recovery indexes must remain deliberately nonunique with its
specified key order, options and simple collation.

All 22 selected codec/audit/parser/output/operator/migration unit tests passed at
that source. At `8b9ec3170606c4dc2cac6f0d8a66826c8d0d9ab1`, all 12 focused
real-Mongo schema/retention/canonical-migration tests passed, including:

- Recovery index drift refuses before any additive effect, including uniqueness,
  key order/name, sparse/hidden/partial/collation drift. Existing unique segment
  indexes cannot become nonunique.
- Unregistered format-1 and unknown recovery carriers refuse migration without
  relabeling rows or creating other collections.
- Canonical migration inspection is read-only; partial DDL resumes under the
  shared lease, stale holders refuse, and completed-schema drift refuses.
- Startup performs no sidecar DDL and requires complete schema for v2 Agents.
- A damaged recovery carrier retains its exact legacy Agent lineage, refuses
  purge without mutation, and does not prevent unrelated legacy retirement.

All five recorded owned Mongo fixtures and volumes were verified absent. Each
heavy gate held one existing shared lock and used two Cargo jobs and one reusable
target. The schema/retention tests use the original bounded fixture resources.

The normal develop sync at `f205616a4e1b14f096067017758a04f7ab17351a`
includes develop `4411f9b699f7dd632c0868ca92d206a0c9af6da5`; only Layer 4 and its
documentation changed. The tested CMA and dependency subtrees/manifests were
verified unchanged by that sync. Strict CMA Clippy on this source failed with nine production diagnostics and
one test-target diagnostic, all unused code and no type errors. Remaining
locations are capture/latest-entry and transaction-scope consumer methods,
request digests, and writer reconciliation. They require real integrated
production callers; no suppression or fake call was added.

## Frozen storage-shape and discovery qualification

At `853b6cca7f4cd2f986bdadc3041eb9ef1952c35c`, all four focused recovery
schema/index Mongo cases passed. They accept all four synthetic phase shapes
and reject twenty malformed variants, preserving the original stored row after
each refusal. These hand-authored storage fixtures are explicitly not actual
codec output or recovery authority.

The first discovery test exposed a real query-shape problem: the factored
cursor predicate matching S5 `discovery::filter` at `2ff67f6cb` examined 64 keys
and documents to return its second 32-row page. The corrected predicate repeats
format/phase equality and the time cutoff inside both `$or` arms. No scan,
byte, row or time bound was relaxed. The original production filter still needs
this change from its owner; a passing index test is not a production-consumer
qualification.

Across 1/100/1000 terminal/future-history rows, the corrected covered discovery
pages and Agent-history page examined exactly their returned keys (zero, one or
32) and zero documents. Timestamp ties, crossing into a later timestamp, empty
final pages, both discovery streams and exclusions are exercised. A separate
pinned-Mongo diagnostic at 65/1024/8192 eligible rows also confirmed bounded
index seeks for the distributed predicate. These measurements demonstrate the
index/query contract, not an unbounded-worst-case claim about the old planner.

All nine recorded owned fixtures and volumes were verified absent at this
checkpoint. The subsequent normal merge of develop `4c65d80e8` preserves both
independent Mongo-store fields and initializers for segmentation and the
new default-off global admission limit. A resulting-head qualification follows.

## Open pre-review and release gates

Pre-review P1-2 remains open. Schema/index registration and the tests above are
S1's portion, not proof of working isolated runtime authority. Required follow-up:
actual codec-bound four-phase carrier fixtures; adoption and qualification of
the corrected S5 production discovery predicate; S2's runtime grants/coherent backup inventory
and actual isolated-role success/missing/foreign-grant tests; final schema and
codec agreement with resumable attachment activation. Any changed persisted
activation fields must update the frozen definition and checksum before release.

The actual permitted rollback artifact remains unqualified; the historical
predecessor executable result above is not a substitute. The combined PR must
also qualify all ordinary writer and bounded response/transcript consumers.
Writer and recovery activation remain closed until every P1 is fixed and tested.
No production calls or mutations ran, and no independent S1 PR was opened. The
binding integration plan uses one combined default-off PR.

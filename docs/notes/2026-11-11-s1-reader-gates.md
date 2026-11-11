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

The corrective executable gates have not run: two shared-lock attempts timed
out before any child process started. A later idle waiter was explicitly stopped
before execution. The earlier passing counts do not certify the later changes.
No review-ready or activation claim is made. Strict production lint, narrow
operator/migration integration, the format-2 recovery carrier's frozen discovery
contract and matching schema remain outstanding. The develop merge freeze is
respected; these checkpoints are published only to the slice branch.

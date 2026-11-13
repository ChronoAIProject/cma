# Transcript, title and usage matrix disposition

Part of #1349. Source pin: `4e49697771e06db6539191162e83431d204cfcfa` on
`tl/cma-log-segments/transcript-consumers`, based on integration
`ce18603caf0561b9e0eb48bc814679e03a764534`. Later evidence-only commits do not
change the qualified source. This is the narrow S3 follow-up for combined review.

## Consumer behavior

Segmented transcript pages seek numeric turn evidence directly, then validate
at most 32 requested original events and four header points. Capture validates
one physical active segment, capped at 10,128 entries / 65 MiB. Each requested
point set and emitted turn page has a 4 MiB limit. Neither a late cursor nor a
long turn causes replay from sequence zero. Continuations retain the source
high-water mark and refuse a changed source. Empty message pages still advance.

Title reconstruction reads four certified originals: start, owner, last answer
and terminal. It preserves first-task eligibility, setup exclusion, queued and
steered input semantics and the existing title fold's truncation. Usage reads
the projection from the same validated capture. Segmented proof failure never
opens a raw-history fallback.

The compatibility transcript response retains its JSON shape within a fixed
200-source-event / 4 MiB budget. Larger segmented histories return typed
`transcript_pagination_required`; TypeScript and Rust SDKs hydrate all pages
after that refusal. Explicit page methods are also available. Browser REST uses
the TypeScript method. Hydration rejects source drift, repeated cursors, message
reordering and changing turn scalars, and never returns partial data after an
authority refusal. See [the public contract](../CMA_Segmented_Transcript_Pages.md).

Ordinary suggestion reads, claims and completions propagate proof refusal in
both snapshot and transaction modes, including an empty tail. Enqueue validates
the exact contiguous append identity and already-published originals inside the
transaction. Unpublished, foreign and altered batches refuse. `snapshot=false`
grants no pending-publication authority; existing writer paths publish first.

## Executable matrix

Mongo cases are actual disposable replica-set tests using canonical migration,
codecs, writer projection planning and capture. They do not substitute a memory
projection for the Mongo reader. The new source cases are in
`store/agent_log_segment_reads/transcript/tests{.rs,/parity.rs,/refusal.rs}` and
`store/suggestions/context/tests.rs`.

| Requirement | Executed assertion |
| --- | --- |
| Fixed work at 1/10/100/1000 seals | Late page crosses the final seal; source rows, BSON and command count bounded; profiler rejects sort/COLLSCAN/write. The 1000-seal source exceeds 10,128 rows and 4 MiB. |
| Long first turn | Twenty seals; limits 1/20/32; complete hydration equals serialized legacy fold; compatibility budget refuses. |
| Serialized transcript/title/usage parity | Completed, failed, interrupted, gapped, steered and active turns; metadata, title truncation and multiple runtime generations. |
| Numeric and queued turns | Original queued owner appears once; numeric evidence order is preserved. |
| Title eligibility | Setup skipped; an ineligible first task never borrows a later answer. |
| Invalid proof | Missing, stale, foreign, unsupported and corrupt active proof refuse without repair or raw fallback. |
| Snapshot authority | Held snapshot remains coherent; fresh read refuses corrupted evidence or original source. |
| Continuations | Invalid bounds, unknown turns, uncovered cursors and changed high-water marks refuse. |
| Suggestion publication | Snapshot/transaction and empty/nonempty reads refuse invalid proof; published suffix accepted; unpublished/foreign/nonconsecutive/altered batch rejected. |
| Final access authority | Owner policy change and shared revocation reject buffered transcript pages; complete transcript and usage retain the owner fence. |
| Legacy compatibility | Serialized legacy page fold and shared redaction, capability precheck, usage generation peaks/retirement/saturation/overrides. |
| Public/client contract | Typed pagination instruction, query validation, next cursor, old complete response, hydration, cancellation/revocation and browser mapping. |

The bounds fixture measured 35 original rows and 19,883 BSON bytes at every
tested seal count. Total profiled commands were 53 at one seal and 56 at
10/100/1000 seals. These are fixture measurements, not production latency claims.

## Qualification

The final locked driver exited successfully on the source pin above.

| Gate | Result |
| --- | --- |
| API/core/public selected unit and contract tests | 199 passed, zero failures; final run 17.127 seconds. |
| Disposable Mongo 8.0.13 replica-set matrix | 34 passed, zero failures; final run 157.776 seconds. |
| Strict API/core/public Clippy, all targets/features | Passed with warnings denied, pinned Rust 1.96.1. |
| Rust SDK strict Clippy and REST suite | Passed; 52 tests, zero failures. |
| TypeScript SDK | Typecheck, strict lint, build, docs (53 examples / 11 documents) and format passed; 4,360 tests passed with one existing skip. |
| Browser | Typecheck, strict lint and production build passed; 78 selected hydration/loading/navigation/transport tests passed across six files. |
| Format, size, configuration, whitespace and history secrets | Passed; canonical size checked 6,342 files. Configuration matches 147/185/12 keys. |

All 41 changed source/test files are strictly below 500 lines. Rust commands use
two jobs, one target and an existing shared lock with `lockf -k`. The runner and
source hash manifests remain unchanged during execution. Both owned Mongo
fixtures and volumes were removed after their runs. Build outputs are removed
after the last gate; source evidence is retained for review.

The broader public selection has three known integration failures for the
unchanged `GET /api/v2/responses/{response_id}/output` route: documented operation
inventory (280 versus 279), tenant-isolation inventory and mounted-route
classification. They require executable response contracts in the combined
branch. The selected S3 gate excludes exactly those three checks; it cannot
certify them. No inventory gate is waived and no response-owned file is edited.

The initial Mongo selection also included the unrelated opt-in real-model
suggestion sample. All 34 storage/consumer cases passed; that sample failed
before database creation because its separately configured local model proxy
was absent. The failed run is retained. The corrected selection excluded only
that sample and passed all 34 actual Mongo cases on a fresh replica set.

This pin proposes source/executable closure of the S3 P1-4 and P2 findings.
Combined review, integration gates and an explicit merge relay remain required.
Writer/recovery activation and production deployment are separate decisions.

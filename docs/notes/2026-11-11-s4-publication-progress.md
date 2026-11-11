# S4 publication progress — 2026-11-11

Part of #1350 and transactional #1349. Branch:
`tl/cma-log-segments/writer-publication`.

## Published dependency and implementation checkpoints

- Original frozen signatures: `453b417426e015459d24cc4e95d9f13586949a03`,
  delivered at 18:53 SGT within the requested 20-minute window after normally
  merging S1 `5ed7eaae1`.
- The approved transaction-scope amendment and supporting registrations at
  `29f8789f6dd7c11d2914159cff56fa9942ff4bb0` are normally merged in `0e7b3e264`.
  This resolves the missing fold exports, sidecar handles, writer declarations,
  receipt append-time schema and transaction-context API.
- `223edd276` adopts the amended `writer_commit::{prepare,persist}` signatures:
  parameter two is `&mut TransactionScope<'_>`. Preparation validates the opaque
  capture before assembly. Prepared plans bind the private transaction stamp and
  database; persistence rejects another session, a later transaction on the same
  session, or another database before writes. The public helper never starts,
  commits or retries the publication transaction.
- `c6419fa83` implements `writer_bootstrap::stage(store, current, operation,
  admission, permit) -> Result<BootstrapPlan, TransactionLogError>`. It reads S1's
  exact bounded legacy snapshot and point proofs, folds original source, and
  stages a private generation with at most 64 evidence rows or 1 MiB per write
  transaction. Individual evidence records are capped at 256 KiB. Every batch
  checks the original durable admission and renews the exact live permit in the
  same transaction. Unknown commit returns no plan. Final publication rechecks
  exact source/aggregate identity and consumes the live fence. Original BSON
  remains untouched. Staged facts start at segmented head revision one,
  independently of the original Agent revision.
- `e323461e9` normally merges current develop after the freeze lift and is pushed.
  The source branch includes develop's merged response foundation; it does not
  import the mixed segmentation integration tip.
- `8a9fbba2d` requires an original point-proved queued owner input and unconsumed
  indexed evidence before publishing its turn. The semantic fold rejects invented
  or consumed queued references and requires complete aggregate queue coverage.
  Native generation maxima now advance in one fold location.

Existing S4 implementation includes deterministic default-off gate selection,
separate recovery admission, bounded preparation slots, exact stable-time
publication receipts, whole-batch capacity planning, independent native-generation
maxima, bounded projection deltas and touched-evidence CAS. Persistence performs
one Agent CAS plus original BSON, optional seal, Head, projection/evidence and
receipt writes in the caller transaction. Any helper error requires aborting all
caller writes. Storage-only recovery preserves conversation/worktree identity
and cannot produce Ready. S5 owns eligibility and the durable recovery carrier.

## Executable status and tests

The first `cargo check --locked --offline -p cma --tests` actually ran under
`lockf` on heavy-0 with two Cargo jobs. It failed with 14 diagnostics. S4 repaired
its test-result handling and BSON comparison. The remaining diagnostics were
relayed to S1 through the lead: retention.rs holds a cursor-stream session borrow
across a sidecar query; capture/snapshot tests require Debug on TransactionLogError;
and schema_tests.rs needs an explicit String for IndexOptions::name.

Five core projection tests passed, including a 30,000-event long turn, original
model/native facts, late usage, generation maxima, exact queued input and refusal
cases. Core clippy with warnings denied passed after fixing its guarded-match
finding. Exact commands and limits are in `2026-11-11-s4-writer-gates.md`. All current S4 Rust sources and
tests are below 500 lines; formatting and whitespace checks pass. Staged secret
checks pass. The earlier legacy assessment's 65 unit / 6 Mongo tests do not
certify this candidate.

Mongo test sources now cover committed rollover; abort and late receipt conflict;
domain/result/session fencing; later-transaction and database rejection; staged
multi-batch source preservation/invisibility; and replaced permits/changed original
requests. These new Mongo cases remain unexecuted. They are not substitutes for
end-to-end caller, unknown-commit failpoint, near-capacity or old-binary evidence.

S1 transaction capture and captured-context validation still return typed
Unavailable, so public preparation remains closed through that proof boundary.
There is no executable production adoption/rollover success claim or review-ready
PR. The staging producer is source-complete but awaits its Mongo gate.

## Outstanding integration and implementation

- S1: publish the small compile fixes; complete transaction/segmented capture and
  exact point certification. Gate/admission schema and migration/inventory/grants
  wiring remain integration requirements.
- S2: register writer persistence/bootstrap Mongo tests in the canonical runner
  using `test(store::agent_log_segments::writer_commit::persistence::tests::)`.
  Publish the exact protocol-neutral output-index types/fold signature requested
  in #1350 comment-6794296472. S4 will maintain those facts atomically; no second
  response-paging schema is being invented in the writer.
- S4: complete the nine caller families, output-index publication/bootstrap,
  crash/retry/lease-race/old-new tests, clippy and canonical Mongo gates. Close the
  source-proof integration only when S1 provides executable certification.
- One planned S4 PR follows green local gates and exact-head review. Merge requires
  the lead relay. The develop merge freeze was explicitly lifted during this
  work; no S4 PR has been opened or merged, and no activation occurred.

## Resource discipline

Heavy commands invoked `lockf` on assigned heavy-0 with CARGO_BUILD_JOBS=2.
The earlier runner incorrectly omitted `-k`, so macOS could remove the shared
lock file on exit. This was reported immediately after checking the platform
manual. Future gates must use `lockf -k -n` to preserve the existing file and
refuse to create a missing one. The gate note records this resource-rule failure;
no active S4 build or lock waiter remains.
Only one target, `api/target`, was used; no per-gate copies or Mongo containers/
volumes were created. The first cleanup removed 5,604 bytes of completed comment
drafts: `CLEANED 0.00 GB`. After gates finished, the target was removed, freeing
1.32 GB (1.23 GiB): `CLEANED 1.32 GB`. No owned build process remains. Requested
local notes are retained and mirrored here. Temporary comment files are removed
after posting; the dedicated worktree remains for review.

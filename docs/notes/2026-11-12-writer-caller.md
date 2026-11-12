# Natural segmented writer adoption

Part of #1350. Branch: `tl/cma-log-segments/writer-caller`, created from
`f3b022ba6d7142004d644317b52484f805765d52`. Develop is merged normally before
publication. This pin adds the missing production adoption caller; activation
still requires the reviewed gate, reader, snapshot and image receipts.

## Behavior

`AgentConversationStore::commit_agent_conversation` and
`commit_agent_message_update` prepare cohort admission for their original
natural mutation. An absent gate, disabled gate or unselected Agent uses the
unchanged legacy publication implementation. Reservation alone does not adopt.
The existing #1591 trusted-summary and bounded-append path is preserved.

The selected v1 path validates its source, domain authority and checkpoint in a
snapshot transaction, positively aborts those preflight fences, and reuses the
existing immutable admission, preparation slot and bounded bootstrap helpers.
The original caller transaction then revalidates all authority and exact source,
and atomically publishes the natural event batch, v2 aggregate, projections,
original publication receipt and any command/checkpoint updates. No extra event
or detached domain transaction is used to initiate adoption.

The original mutation supplies the stable publication key, payload fingerprint,
timestamp and result digest. Final commit uncertainty uses the existing exact
publication/domain reconciliation. Admission uncertainty returns unavailable
and retains the original identity. A moved source refuses rather than replans.
Gate closure admits no new activation, while one exact previously admitted
operation can finish. Already-v2 publication keeps its existing behavior.

## Regression inventory

Nine new real-Mongo tests exercise production roots:

- Raw BSON parity with the original legacy transaction for both roots, with an
  absent gate, a disabled gate that names the Agent, and an enabled other cohort.
- Natural conversation output adopts v1, retains existing bytes, appends only
  its original event and remains readable after reopening the store.
- Natural checkpoint output adopts v1 and replays the exact checkpoint receipt.
- Foreign mutation/checkpoint refusal before any durable admission.
- Aborted publication preserves original source and sidecars; the original
  accepted operation can finish after gate closure.
- A competing legacy writer prevents stale prepared adoption or replanning.
- Actual reservation/claim, natural command admission, runtime output and command
  settlement; reservation stays v1, publication adopts, settlement clears custody.
- Unknown admission commit retains the immutable original request and retries once.
- Unknown final publication reconciles its exact committed receipt or refuses
  an uncommitted outcome, without changing identity or replaying output.

Fault injection is app-name scoped on one disposable local replica set. Final
publication faults arm on the actual v1-to-v2 Agent CAS, after preparation, so
they cannot accidentally test only an earlier admission transaction.

## Qualification

Caller implementation: `a6ce080117c1b1164b2290394067f8819001c0ef`.
Final merged executable source: `e2569674483553ad63f7e59162e4d0e37cd52ad6`,
including develop `b0adf6dd7` / Mongo diagnostics #1764 by normal merge.
The two merge conflicts retain the combined Mongo inventory and preserve
segmented recovery snapshot dispatch inside the diagnostic attribution scope.
The writer caller itself is unchanged by the develop merge.

Before the diagnostics merge:

- Focused units: **16/16 passed** (six gate-policy, nine writer-plan and one
  checkpoint-decoder test).
- Real Mongo: **37/37 passed**, including **9/9 new caller tests**, on a local
  Mongo 8.0.13 replica set. Three slow tests deliberately exercise uncertain
  commits. The batch took 335 seconds; no failed or unexecuted case is counted.
- Strict CMA Clippy passed with all targets/features and `-D warnings`.

Final merged-source qualification:

- Strict CMA Clippy passes with all targets/features, `--no-deps` and
  `-D warnings`; no warning allowances added.
- Focused units: **21/21 passed**, retaining all 16 original tests and adding five
  diagnostic opt-in, privacy, sampling and overhead tests.
- Real Mongo: **42/42 passed**, including **9/9 new caller tests**, the original
  publication/checkpoint/integrity regressions, four retained-recovery cases and
  real diagnostic monitoring. The final batch took 435 seconds (two slow tests).
- Mongo runner-contract suite: **31/31 passed**, covering identical source/archive
  selection, authenticated-suite exclusion, failure preservation and cleanup.
- Workspace formatting, whitespace and canonical file-size check pass:
  **6,448 source files** after the merge. Each changed Rust file is below
  500 lines, including tests.
- Full-history secret scan passed over 4,238 commits with no findings; the
  final documentation commit is scanned again before push.

Executed Rust commands use toolchain 1.96.1, `CARGO_BUILD_JOBS=2`, one target,
no incremental compilation and no debug symbols. Heavy work holds the existing
`.heavy-release.lock` via `lockf -k -t 3600`, with a monitor pausing below 10%
available memory. The fixture is one 1 GiB container; container and owned volume
were verified removed after each batch. No production operation was performed.

```bash
cargo clippy --manifest-path api/Cargo.toml --locked --offline -p cma \
  --all-targets --all-features --no-deps -- -D warnings
cargo nextest run --manifest-path api/Cargo.toml -p cma --locked --offline \
  --no-tests fail --test-threads 2 \
  -E 'test(::agent_conversation_store::) | test(::writer_plan::tests::) | test(::writer_gate::tests::) | test(::mongo_diagnostics::tests::) | test(::diagnostic_request_tests::)'
# In api, with both test Mongo variables supplied by the disposable fixture:
cargo nextest run -p cma --locked --offline --run-ignored only --no-tests fail \
  --test-threads 1 --no-fail-fast --partition count:1/1 \
  -E 'test(::ordinary_publication::) | test(::agent_conversation_store::checkpoint_tests::) | test(::agent_transaction_tests::conversation_tests::) | test(::log_integrity_tests::) | test(::writer_commit::persistence::tests::original_callers::) | test(::sandbox_unpublished_recovery::mongo_integration_tests::retained::) | test(::mongo_diagnostics::mongo_tests::)'
node --test --test-concurrency=1 .github/scripts/mongo-contract-tests.test.mjs
cargo fmt --manifest-path api/Cargo.toml --all -- --check
bash .github/scripts/check-file-size.sh
bash .github/scripts/scan-secret-history.sh /opt/homebrew/bin/gitleaks .
```

Local commands, full test inventories, gate logs and fixture cleanup receipt:
`notes/2026-11-12-writer-caller-evidence/` in the writer worktree. The only build
target and disposable execution scripts were removed (2.819 GiB freed);
retained evidence is intentional. The integration owner takes the reviewed pin
through the combined PR.

The selected v1 path incurs bounded legacy capture/bootstrap reads once during
adoption; absent/closed gates do not enter that path. Measure preparation work
alongside subsequent read savings during activation. This source evidence does
not establish production image/reader/snapshot acceptance or natural rollover.

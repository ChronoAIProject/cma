# P0 usage-report follow-up validation

Validation uses the dedicated `tl/cma-usage-fix2/integration` worktree based on
`origin/develop` revision `6750dac2a`. Production was read-only. Heavy steps use
the shared lockf lanes with four Cargo build jobs and debug information disabled.

## Production-shaped regression

A disposable loopback MongoDB 8.2.4 replica set seeds 400 Sandbox allocation
states, including 40 active and 360 paused, over a 24-hour period. The test creates
an administrator report, caches it, changes the status filter to active, polls the
pending report repeatedly and reads the previous report's table. It then occupies
the reporting pool's four connections with bounded 700 ms commands and queues
one successful checkout. The control connection uses a separate Mongo client.

The same report-worker finalization path produces a new ready report with all
40 active Sandboxes and complete allocated usage. Repeated report polls, filtered
pages and the previous 400-Sandbox inventory must all return 200. Measured values
remain unavailable. A deliberately opened circuit still yields 429 for new work
and cached pages. Terminal cache byte accounting remains in force.

The fixture accepts both BSON integer widths for Mongo's failpoint entry counter;
an initial run exposed that test-helper assumption before reaching pressure
injection. The corrected fixture passes. CI retains its existing pinned Mongo
8.0.13 image and automatically selects the new test through the reporting module.

Unit regressions cover a 252 ms successful spike, recovery on a healthy checkout,
expired streaks, sustained slow checkouts, hard checkout/connection-hold stalls,
and failed checkouts. No configuration, schema, durable enum or public API model
changes. No web files changed.


## Baseline proof

Temporarily restoring only `usage_reports/budget.rs` from production revision
`8d2c8c109` makes the regression fail with the exact observed pair:
`(new report poll, previous report page) = (503, 429)`, with generation error
`reporting budget exhausted; retry with a narrower range`. The file is restored
to the fixed implementation immediately after this intentional baseline run.
The new implementation expects `(200, 200)` under the same bounded contention.

## Final local gates

| Gate | Result |
| --- | --- |
| API workspace formatting | Passed |
| All-target, all-feature workspace Clippy with warnings denied | Passed |
| Affected suites: cma, layer1-public, layer2-core | 3,064 passed; includes OpenAPI and public contract replay |
| Mongo report and allocation-ledger suite | 23 passed, including the new 400-Sandbox case and existing 20,001-Sandbox measured report |
| Affected doctest command | Passed; these packages expose no runnable doctests |
| Source-size | Passed; changed source files contain at most 419 lines |
| Configuration keys and documentation | Passed; no new configuration |
| Whitespace and secret scanning | Passed |

Final gates use the fixed code, after the intentional baseline reproduction:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo nextest run --locked -p cma -p layer1-public -p layer2-core --test-threads 4 --no-fail-fast
cargo nextest run --locked -p cma --run-ignored only -E 'test(store::usage_reports::tests::) | test(store::sandbox_mongo_integration_tests::sandbox_usage_tests::)' --test-threads 1 --no-fail-fast
cargo test --locked --doc -p cma -p layer1-public -p layer2-core
```

The Mongo URI identifies only the task-owned disposable loopback replica set.
Frontend/SDK code and public contracts are unchanged. Broader Linux/image CI
remains with the PR; these local results do not claim CI completion. The lead
owns merge and deployment. Temporary build outputs, Mongo data and logs are
removed after recording these results; the worktree stays for lead review.

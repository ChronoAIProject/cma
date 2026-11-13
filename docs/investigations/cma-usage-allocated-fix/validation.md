# Allocated reporting fix validation

Validated on 6 November 2026 in the dedicated
`tl/cma-usage-allocated-fix/integration` worktree, based on production API
`b31a88603`. Heavy commands used the shared `lockf` lanes, four Cargo build jobs,
and disabled development/test debug information. Production access was read-only.

## Regression evidence

The disposable local Mongo replica set ran native MongoDB 8.2.4 with test
failpoints enabled, loopback-only binding and a 0.25 GiB WiredTiger cache. CI
retains its existing MongoDB 8.0.13 fixture and all new tests are included in its
existing reporting-test selection. No production database was accessed.

- Both public owner and admin report routes: POST 202, generated report GET 200,
  member page 200, complete allocated capacity/resource-hours, null measured
  values even with retained measurements, correct owner cohort, cross-owner 404
  and missing admin capability 403.
- Collection enabled: retained measured means remain unchanged. Corrupt measured
  records fail that path, while collection-off reports still succeed. Corrupt
  allocation evidence fails both modes; failed report polling returns 503
  `usage_unavailable`.
- Twelve alternating owner/admin reports complete without exhausting two maximum
  reservations. Both successful and failed reports retain their exact stored
  cache bytes; a reduced real quota still refuses admission, and stale workers
  cannot append after finalization.
- Three 750 ms database reads succeed individually but exceed two seconds in
  aggregate. Temporarily restoring only the deployed `budget.rs` makes this test
  fail with `MetricsError::Busy`; restoring the fix makes it pass. The baseline
  reproduction was intentional, and the deployed source was restored only in
  this local test before returning the file to the fixed implementation.
- Slow individual connection holds, slow checkouts and checkout failures still
  open the circuit. Consecutive bounded reads do not.

One new test initially attempted to append a second generation to an already
written lease cache and failed correctly on duplicate cache identities. The test
was corrected to use distinct admitted reports. All final regression runs pass.

## Local gates

| Gate | Result |
| --- | --- |
| Locked API workspace build | Passed |
| API workspace formatting and all-target/all-feature Clippy, warnings denied | Passed on final code |
| Affected backend suites (`cma`, `layer1-public`, `layer2-core`) | 3,063 passed; includes OpenAPI and shared public contract replay |
| Mongo reporting and allocation-ledger contracts | 22 passed: 13 reporting and 9 lifecycle/allocation tests |
| Existing 20,001-Sandbox measured report, indexed pages and authorization | Passed within the existing 60-second generation/RSS budgets |
| Affected backend doctest command | Passed; these packages expose no runnable doctests |
| TypeScript SDK format, lint, typecheck, tests, build and documentation examples | Passed; 4,131 tests, one pre-existing skipped case; 53 documentation examples |
| TypeScript SDK audit at high severity | Passed; existing moderate advisories remain outside that threshold |
| Rust SDK format, all-target/all-feature Clippy, tests and doctests | Passed; 255 tests and 37 doctests |
| API dependency advisories, licenses, bans and sources | Passed |
| Source-size, configuration keys/documentation and protocol documentation checks | Passed; every changed source file is below 500 lines |
| Secret scan and whitespace checks | Passed |

The public report/row models, reporting collection schema, OpenAPI and SDK source
are unchanged from `b31a88603`. New reports reuse the existing schema version,
fields, nullable quantities and enum values. Strict existing report decoding is
exercised after finalization; no new durable reader requirement is introduced.

No web files changed. Frontend presentation and polling behavior remain outside
this PR; the data fix serves both existing interfaces. The lead owns merge and
production deployment. CI includes broader Linux/image checks that cannot be
claimed complete from these focused local gates.

Core commands (from `api/`, with the resource-lane environment described above):

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo build --workspace --locked
cargo nextest run --locked -p cma -p layer1-public -p layer2-core --test-threads 4 --no-fail-fast
cargo nextest run --locked -p cma --run-ignored only -E 'test(store::usage_reports::tests::) | test(store::sandbox_mongo_integration_tests::sandbox_usage_tests::)' --test-threads 1 --no-fail-fast
cargo test --locked --doc -p cma -p layer1-public -p layer2-core
cargo deny --locked --all-features check advisories licenses bans sources
```

The Mongo command receives only the task-owned loopback fixture URI. Temporary
build outputs, fixture data and logs are removed after recording these results;
the dedicated worktree stays for the lead's review and later removal.

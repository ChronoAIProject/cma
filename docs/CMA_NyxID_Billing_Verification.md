# NyxID prepaid Sandbox adapter verification

This records local verification for [CMA #1312](https://github.com/ChronoAIProject/cma/issues/1312).
The producer contract is [NyxID #2204](https://github.com/ChronoAIProject/NyxID/pull/2204),
reviewed at `dc5d1cd9193c8a5b41c80315e067208505e87581`. Configuration and activation
requirements are in [Sandbox usage](CMA_Sandbox_Usage.md#nyxid-prepaid-lifecycle-adapter).
Both billing flags remain disabled in the checked-in deployment.

## Focused scenarios

The focused run passed all 42 tests. It exercises the provider-neutral adapter,
an actual HTTP fake producer, Kubernetes HTTP fixtures, lifecycle workers, and
an isolated MongoDB replica set. Cases include:

- Durable admission before external effects, duplicate reservation, renewal in
  the final 30 seconds, and full quote/window receipt preservation.
- A server committing a reservation and losing the response, transport timeout,
  recovery after adapter restart, and replay with the original request identity.
- Insufficient funds, dependency outage reported as HTTP 402, unavailable
  credentials, malformed/oversized receipts, Retry-After, expired coverage,
  owner/allocation mismatch, and stale replicas losing a Mongo compare-and-swap.
- Whole-window evidence, partial delivery, stable frozen payloads, durable cursors,
  straddling revisions, historical enrollment, and refusal to invent missing use.
- Disk-only renewal while paused, explicit observed zero tails after teardown,
  retirement despite funding failure, and retry after settlement persistence fails.
- Native deadlines fenced by runtime identity, UID and resource version, retained
  homes, refusal to extend an expired generation, and unconfirmed patch rejection.
- Default-off behavior, unchanged historical provider routing, and a delayed
  funding pause refusing to mutate a newer Sandbox revision.

## Coverage

LLVM source coverage from the focused run, excluding test code:

| Production module | Covered executable lines | Line coverage |
| --- | ---: | ---: |
| CMA billing configuration | 44 / 44 | 100.00% |
| Funding coordinator | 254 / 271 | 93.73% |
| Retained evidence | 133 / 138 | 96.38% |
| Funding models and receipt validation | 81 / 81 | 100.00% |
| Mongo/lifecycle funding ports | 22 / 23 | 95.65% |
| NyxID HTTP transport | 142 / 149 | 95.30% |
| Mongo funding persistence | 97 / 102 | 95.10% |
| Native deadline enforcement | 63 / 63 | 100.00% |
| **New modules combined** | **836 / 871** | **95.98%** |

Including added executable production lines in existing lifecycle, routing,
composition and metering files gives **963 / 1,119 (86.06%)**. This calculation
intersects added lines from `git diff --unified=0 origin/develop` with LLVM `DA`
entries, combines repeated entries by source location, and excludes test files
and inline test modules. Both measures exceed the requested 80% threshold.

Reproduce the focused run from `api/` with an isolated replica set supplied by
`CHRONO_TEST_MONGODB_URI`:

```sh
export RUST_MIN_STACK=33554432
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
cargo llvm-cov nextest --no-report -p cma -p layer3-adapter -p layer2-core \
  --run-ignored all --test-threads 4 \
  -E 'test(sandbox::funding::) | test(funding_) | test(config::sandbox_billing::) | test(store::sandbox_billing::) | test(funded_zero_tail) | test(historical_unattached_home_preserves_local_request_when_native_becomes_default)'
cargo llvm-cov report --lcov --output-path /tmp/cma-funding-coverage.lcov
```

## Repository checks

Local checks use the repository's locked dependencies and CI command selections.

| Check | Result |
| --- | --- |
| API workspace format, all-target/all-feature Clippy, locked build and doctests | Passed |
| Full API nextest suite | 7,155 passed; 668 skipped by the standard selection |
| Isolated API media-memory and turn-metrics budgets | 2 passed |
| Mandatory Mongo contract selection on an isolated replica set | 454 passed |
| Rust SDK format, all-target/all-feature Clippy, build and tests | Passed; 250 tests and 37 doctests |
| Web format, lint, typecheck, harness typecheck, tests and build | Passed; 3,279 tests across 369 files |
| TypeScript SDK format, lint, typecheck, coverage, build and package/docs checks | Passed; 4,011 tests, 1 skipped; 96.64% line coverage |
| Configuration keys and operations documentation | Passed; key validator's 11 regressions also passed |
| Source file size gate | Passed; 5,085 source files checked |
| Mongo runner inventory/cleanup regressions | Passed; 7 tests |
| Rust dependency policy and branch secret scan | Passed |
| Web and TypeScript SDK dependency audit | Passed at the CI high-severity threshold; existing moderate development-tool advisories remain |

The full API run exited successfully and emitted one nextest leak warning in the
existing `readiness_cannot_survive_an_intervening_claim_and_release_revision` test.
The Python native recovery suite completed 40 tests with one skip, and the managed
transport proof suite passed all seven tests. CI remains responsible for Linux-only
process-boundary tests, image builds/scans, and deployed browser/provider campaigns.

## Environment and limits

Local verification ran on macOS with task-owned MongoDB 8.2.4 replica sets;
CI provisions its pinned MongoDB 8.0.13 image. The funding Mongo test is included
in the mandatory `.github/scripts/mongo-contract-tests.sh` selection, whose
inventory and fixture cleanup regression suite also passes. Mongo failpoint
tests and the coverage fixture use separate replica sets.

Native deadline semantics were checked against agent-sandbox v1.0.2 at
`9a85153590e54cb980f3241f9e7a9228449412c9`: core Sandbox `shutdownTime` with
`shutdownPolicy: Retain` removes the Pod/Service while retaining the Sandbox/PVC.
The tests verify desired-state requests and responses. They do not certify
physical termination during node/control-plane partitions, live NyxID/Lago
invoice outcomes, or end-user price consent. Those remain controlled activation
requirements; this change does not enable paid admission.

## CI authority-catalog repair — 5 November 2026

CI run `41865437880` exposed a missing Agent Manager grant: full store startup
creates `sandbox_billing_state` even with funding disabled. The collection now
uses the same constant in storage and the exact authority catalog, so generated
runtime grants and startup credential validation include it. The authenticated
bootstrap regression checks `sandbox_billing_due` on `{retry_at: 1, _id: 1}` after
both initial startup and reopen. Grant regressions require the collection and
reject `dropIndex` authority on it. The grant fix does not modify funding or
lifecycle implementations.

The PR adds only this collection and explicit index; `sandbox_usage` and
`sandbox_usage_events` already have catalog entries. Agent Manager's existing
runtime role owns startup DDL and has no separate migrator role. Exact actions
and the existing-role update procedure are documented in
[service isolation](CMA_Service_Isolation.md#sandbox-funding-grants) and linked
from deployment readiness. Both billing flags remain false.

After merging `origin/develop` through `c21108356` (#1317 and #1318) without
conflicts, final local verification used Rust 1.96.1, `CARGO_BUILD_JOBS=4`,
disabled incremental/debug output and
`RUST_MIN_STACK=33554432`:

| Repair verification | Result |
| --- | --- |
| Authenticated Agent Manager isolation checks | 4 passed, including full production assembly startup twice and foreign collection denial |
| Standard API workspace suite | 7,166 passed; 669 standard-selection skips |
| Isolated media-memory and socket-latency budgets | 2 passed |
| Mandatory Mongo contracts | 455 passed across 12 disposable replica sets |
| Format and all-target/all-feature workspace Clippy | Passed |
| Locked workspace build, doctest gate and dependency policy | Passed; no workspace doctest examples are defined |
| Compiled runtime-role output | Exact billing actions and service collection boundaries verified |
| Native recovery and retained transport proofs | 40 tests with 1 skip, and 7 tests passed |
| Source size and its validator regressions | 5,091 files checked; 11 regressions passed; isolation module is 489 lines |
| Root/SDK/infra/docs structure and layer boundaries | Passed |
| Deployment configuration and documentation inventory | Passed |
| Mongo runner and authenticated-readiness regressions | 7 and 5 passed |

Both Mongo runs used the repository's pinned Docker MongoDB 8.0.13 image.
The isolated credential run reused the authenticated fixture setup from
`.github/scripts/image-manager-mongo-contract.sh`, then executed:

```sh
cargo nextest run --workspace --locked --run-ignored all --test-threads 1 \
  -E 'package(=cma) and test(~store::isolation::tests::)' --success-output final
```

The full Mongo run used `.github/scripts/mongo-contract-tests.sh` unchanged,
including its two-fixture concurrency bound and owned-container/volume cleanup.
The first runner-regression attempt hit fixture timing bounds on the shared
machine; the unchanged rerun passed all seven checks. The final workspace suite
exited successfully with one nextest lingering-process warning in the existing
`lost_create_response_recovers_same_uid_and_changed_spec_is_refused` HTTP fixture;
its exact isolated rerun passed without the warning. These are local results;
production role provisioning, rollout and activation remain the lead's work.

# CMA Mongo performance verification

Issue: https://github.com/ChronoAIProject/cma/issues/1320.
See [the investigation report](report.md) for production observations, interpretation and the deployment verification plan.

## Environment and reproduction

Local verification ran on macOS / Apple silicon on November 5, 2026. Cargo ran from `api/` or `sdk/rust/` with the repository-pinned Rust 1.96.1, `CARGO_BUILD_JOBS=4`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_INCREMENTAL=0` and `RUST_MIN_STACK=33554432`. Workspace tests used four test processes; timing budgets and focused Mongo measurements used one. JavaScript checks used Node 25.5.0; CI checks the declared Node versions on Linux.

The focused database reproduction used an owned loopback Mongo replica set. The required Mongo contract script separately creates bounded, labelled, disposable Mongo 8.0.13 containers and deletes their containers and volumes. No test connects to production Mongo or writes to production services.

```sh
# In api/, with both test Mongo URI variables pointing at a disposable replica set:
cargo nextest run --workspace --locked --run-ignored only --test-threads 1 \
  -E 'package(=cma) and test(task_status_batch)' --success-output final
```

The fixture has 20 ready Agents, each with 1,286 retained events and 4 KiB messages. Ten have a completed turn; ten contain a conversation gap. The baseline drops only the added task-status index and runs the original sequential status method; the final path recreates the index and uses the batch method. Command counts and examined-document counts are assertions; elapsed times are observations without a machine-specific pass threshold.

| Measurement | Baseline | Final |
| --- | ---: | ---: |
| Status commands for the 20 Agents | 60 | 1 |
| Documents examined by the no-gap range lookup | 1,284 | 0 |
| Initial local elapsed time | 32.752 ms | 6.016 ms |
| Final pinned-toolchain elapsed time, shared machine | 69.442 ms | 9.085 ms |
| Unchanged maintenance admissions: 15 candidates, 300 one-second sweeps | 4,500 | 150 |

The comparison asserts identical statuses and ten results of each expected kind. Separate tests exercise snapshot isolation during concurrent appends, large-record fallback, high-water disagreement, corrupt evidence, non-ready lifecycle status, and actual lost-connection recovery during the optional maintenance cooldown. CMAP tests cover bounded tracking, separate server identities, hold timing across summary windows, closed connections and payload privacy. The new ignored Mongo tests are explicitly included in the CI contract selector, whose seven regression tests pass.

An early exploratory command invoked Cargo from the repository root and therefore selected the machine's Rust 1.95.0; the final gates run from the pinned-toolchain directories. Another overly broad exploratory test filter selected unrelated maintenance tests without their required failpoint/stack setup. The corrected focused selection passed. These setup corrections did not require changing test assertions or weakening gates. The authenticated isolation gate initially reached its final browser journey and failed because another local process owned the default port 4359. The complete gate passed on rerun with the existing `CMA_BOT_BROWSER_PORT=48339` override and an owned temporary directory; no other process was stopped.

## Local gates

These are local results for the implementation plus the merged `develop` probe-alias change. The Gateway package was linted again after that merge.

| Gate | Result |
| --- | --- |
| Backend format, all-target/all-feature Clippy, workspace build | Passed |
| Backend ordinary nextest suite | 7,122 passed, 669 skipped by the ordinary-suite selection; 538.887 seconds |
| Image normalization and socket timing budgets | Two passed with one test process |
| Backend doctests and dependency audit | Passed; advisories, licenses, bans and sources accepted |
| Native recovery and retained-transport Python contracts | 40 recovery cases (one skipped) and seven transport cases passed |
| Required disposable Mongo contract suite | 455 passed across 12 owned fixture partitions; zero failures |
| Authenticated Image Manager / Gateway / CMA isolation contracts | Passed, including the compiled Gateway and real dashboard browser journey |
| Gateway latency budget | Both ignored budget tests passed on the first run |
| Frontend format, lint, types, build, CSS, computed styles, harness types | Passed |
| Frontend Vitest | 3,279 passed |
| Browser layout / harness mechanics | 10 / 5 passed |
| TypeScript SDK format, lint, types, build, docs and package checks | Passed |
| TypeScript SDK coverage suite | 4,011 passed, one skipped |
| Frontend and TypeScript SDK dependency audits | Passed at high severity; existing moderate findings remain |
| Rust SDK format, lint, build, tests, docs, dependency audit and coverage | Passed; 250 tests, two skipped; 93.16% line coverage |
| Standalone Rust and packaged Node SDK consumers | Passed |
| Source-size, configuration, API docs, release/provider and harness contracts | Passed (5,075 source files checked); every changed Rust source and test file is below 500 lines (largest: 489) |
| Supplemental release / deployment structure contracts | Passed, including eight real Skopeo transport contracts |
| Kubernetes manifests | 52 valid; zero errors or skipped resources; kube-linter passed |
| Final whitespace and secret scan | Passed; pinned gitleaks 8.30.1, full candidate history, no leaks |

Core backend commands are the repository CI commands: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo build --workspace --locked`, ordinary `cargo nextest run --workspace --locked --no-fail-fast` with the two dedicated budget binaries excluded, those binaries separately with one test process, `cargo test --doc --workspace --locked`, and `cargo deny --locked --all-features check advisories licenses bans sources`. Database gates use `.github/scripts/mongo-contract-tests.sh` and `.github/scripts/image-manager-mongo-contract.sh` unchanged except for adding this PR's tests to the former's selection.

Linux-only Workspace writer boundaries, the five Linux image builds, image vulnerability scans and built-image contracts remain CI gates. They are not represented as locally passed on macOS. The lead owns the green-CI merge and batched deployment; no production improvement is claimed before that verification.

## Review and cleanup

No patch edits the files owned by #1316 or #1318, the probe-alias implementation in #1317, `sandbox/funding.rs`, or `store/sandbox_billing.rs`. Upstream changes are incorporated by normal merges into `tl/cma-db-perf/integration`. The PR uses `Refs #1320`; the issue remains open for production verification.

Temporary local tools, databases, logs, dependencies and build outputs are removed after publishing the PR. The committed Markdown evidence and worktree remain for the lead. No teammate worktree, branch, cache, container or production resource is removed.

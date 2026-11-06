# Backend CI archive

`Backend` remains the required check name. It always runs after lint, the test
archive producer, all three ordinary-suite shards, timing budgets, disposable
Mongo contracts and authenticated Image Manager Mongo isolation. Its one step
rejects every dependency result other than `success` and prints the failing
job names. A failed, cancelled or skipped upstream job cannot turn the required
check into a passing skip. `CMAEG latency budget` depends on the archive producer
and remains outside this join.

Hosted run `42170757820` at `ca3bd3cec` completed the first `Backend` join in
30.1 minutes on cold caches, within the 25–33 minute target. That attempt failed
on an existing Mongo scale-test timing sensitivity; rerunning only Mongo and the
join passed using the original archive. `Images (api)` took 47 minutes and was
the longest job, so image work bounds merge time for PRs that change API crates.
See [hosted Actions evidence](Backend_CI_Archive_Verification.md#hosted-actions-evidence)
for durations, artifact identity and the successful rerun.

## Build and runner boundaries

All jobs use standard `ubuntu-latest` runners. Lint retains the pinned dependency
auditor, format check, both Python process/recovery proofs, all-target/all-feature
Clippy, the locked workspace build, and doctests. Doctests cannot be archived.
The independent producer runs `cargo nextest archive --workspace --locked`.
Producers and consumers install exactly `cargo-nextest@0.9.151`.

The artifact is `backend-tests`, with one-day retention, no upload compression,
`if-no-files-found: error` and `overwrite: true`. Its name does not contain
`run_attempt`: rerunning only failed consumers reuses the successful producer's
artifact; rerunning the producer replaces it. Retention is one day: rerunning
only consumers more than a day after the producer requires rerunning the whole
workflow to create a fresh archive. The archive lives in `RUNNER_TEMP`,
outside the Rust cache. It never uses `actions/cache`.

Each consumer checks out the same revision at the default runner path and runs,
from `api/`, exactly once:

```sh
cargo nextest list --archive-file "$RUNNER_TEMP/backend-tests.tar.zst" --extract-to "$PWD" --list-type binaries-only
```

This extracts into `api/target/`; it does not execute tests. Every subsequent
invocation uses `target/nextest/binaries-metadata.json` and
`target/nextest/cargo-metadata.json`. There is no per-partition extraction,
Cargo compilation or Rust cache in a consumer. The wrapper rejects metadata
whose recorded workspace or target path differs from the current checkout.
Every test-running job has `RUST_MIN_STACK=33554432`; both compiling jobs retain
`CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_PROFILE_TEST_DEBUG=0`.

The ordinary suite uses `count:1/3`, `count:2/3` and `count:3/3`, always with
`--no-fail-fast`, excluding only `media_image_peak` and `turn_metrics` as before.
The matrix has `fail-fast: false`. The two excluded binaries run consecutively
with `--test-threads 1 --success-output final` on their own runner, followed by
the exact ignored Linux writer-boundary test. No other test process competes
with either timing budget there. The separate CMAEG latency job retains its one
retry and `api/target/cmaeg-latency.txt` upload.

Job timeouts are 45 minutes for lint/build/shards, 35 for timing, 90 for Mongo,
30 for Image Manager, 15 for CMAEG latency and 5 for the join. These exceed twice
the supplied worst measured durations. Mongo retains its 75-minute step limit;
Image Manager retains its 10-minute contract step limit.

## Artifact dependency audit

The Rust source at parent `8d2c8c109` contains 20 uses of
`env!("CARGO_BIN_EXE_cmaeg")`, one each for `cmaeg-media-probe` and
`layer4-runtime`, 19 `CARGO_MANIFEST_DIR` uses in 18 files, and one
`CARGO_TARGET_TMPDIR` use. The gateway fixture derives `target/debug/cmaeg` from
its test process's `current_exe()` unless `CMAEG_TEST_BINARY` is supplied.
Other current-executable lookups launch the test binary itself or inspect its
file descriptor; they need no additional workspace executable. External tools
such as Git, curl and the browser remain runner/setup dependencies.

Consumers retain the original absolute workspace path
`/home/runner/work/cma/cma` and target path. Checkout supplies manifest-relative
source and fixture files, and the consumer-readers test creates its own
`CARGO_TARGET_TMPDIR` subdirectory. Path remapping alone would not repair these
compile-time `env!` values. Extraction explicitly requires all three child
executables to exist and be executable.

The Linux proof built the archive without a preceding `cargo build`. It contained
98 test binaries and four non-test binaries: `cmaeg`, `cmaeg-media-probe`,
`layer4-runtime` and `cma-image-manager`. After removing the entire compiled
target, extraction alone supplied the child binaries and passed both the
compile-time executable-path test and the gateway fixture lookup. No
`archive.include` configuration is needed. This is why the separate locked
workspace build remains a required lint-job gate without blocking archive
consumers. Exact commands and inventories are recorded in
[archive verification](Backend_CI_Archive_Verification.md).

## Local scripts and selection semantics

Without `CMA_NEXTEST_REUSE`, both Mongo scripts still build from source for local
development, consistently using the workspace feature graph. With
`CMA_NEXTEST_REUSE=1`, they require the extracted metadata and never fall back to
compilation. An empty/unknown mode, missing/malformed metadata, relocated
checkout, or CI source mode fails before creating a fixture. The Python Mongo
coordinator inherits the same mode for its listing and every child partition;
it neither extracts nor compiles on its own.

Both modes discard caller Mongo URIs, create their own pinned disposable
fixtures, and retain their ownership checks and cleanup scope. Mongo still uses
at most two independent fixtures, with at most 40 tests per fixture and one test
process per fixture. Image Manager uses one authenticated replica set and runs
all its invocation groups sequentially.

Converted `cargo test` filters map as follows:

| Original | Nextest |
| --- | --- |
| `-p NAME` | `package(=NAME)` |
| `--lib` | `kind(lib)` |
| `--test NAME` | `kind(test) and binary(=NAME)` |
| positional substring | `test(~substring)` |
| `--exact` | `test(=full_test_name)` |
| `--ignored` | `--run-ignored only` |
| `--include-ignored` | `--run-ignored all` |
| `--test-threads=1` | `--test-threads 1` |

Every run explicitly uses `--no-tests fail`. Converted libtest groups also use
`--no-fail-fast` so a failed case does not suppress the remaining cases in that
invocation. For the two-target Image Manager invocation this also attempts the
second target if the first fails. The shell still stops before the next group
on failure. Existing nextest invocations retain their failure policy.

Libtest previously ran several cases inside one process; nextest starts a fresh
process per case. These converted tests create their fixture state inside each
test, use unique database names, and do not rely on a previous case initializing
process globals. One-process-at-a-time execution preserves their shared Mongo
fixture exclusion. The archive deliberately uses the workspace dependency
feature graph throughout instead of switching to package-only builds between
invocations.

## Cache pressure

Only lint and the archive producer use `rust-cache`. They use its default job-ID
keys, with no `shared-key` or other key override:

| Job | Cache namespace |
| --- | --- |
| Backend test build | `v0-rust-backend-test-build-*` |
| Backend lint | `v0-rust-backend-lint-*` |

The purpose is deterministic ownership: each job restores caches populated by
that same job, so lint cannot decide which build artifacts the producer restores.
Both save only on pushes to `refs/heads/develop`. PR jobs restore readable develop
caches without creating merge-ref entries. Compiler/dependency and environment
hash changes can still create new entries; consumers have no Rust cache and
archives use Actions artifacts.

The earlier warm-cache transition claim, including the rationale in commit
`c1a3bc974`, was incorrect. Keeping `shared-key: backend` preserved only a prefix.
The producer's toolchain component set changed its environment hash, and the old
entry had already been evicted. Both compilation jobs were cold in the hosted
run. Default job-ID keys make the separate writers explicit without suggesting
that an old entry will be reused.

There are two develop-only cache entries per compiler/dependency/environment
key, one more than the former Backend job. New per-PR Backend entries and
`v0-rust-cmaeg-latency-*` writes are eliminated. Old Backend, PR and latency
entries remain until Actions evicts them; this workflow performs no remote cache
deletion. The hosted PR run created no Backend merge-ref caches. The first
develop-push save remains to be verified after merge using the
[caches API procedure](Backend_CI_Archive_Verification.md#hosted-actions-evidence).
See the [rust-cache key implementation](https://github.com/Swatinem/rust-cache/blob/v2/src/config.ts).

## Reproduce selection equality

Export the two baseline scripts from `8d2c8c109` into a temporary directory.
After extracting an archive, run from `api/`:

```sh
CMA_NEXTEST_REUSE=1 python3 ../.github/scripts/backend-selection-proof.py BASELINE_SCRIPTS OUTPUT
```

The proof compares the old main selection to the disjoint union of three
partitions, both isolated budgets, the exact Linux case, Mongo, every Image
Manager invocation, and CMAEG latency. It enumerates original `cargo test`
filters through the archived binaries' own libtest `--list` interface, then
compares them to nextest lists. It emits sorted `(binary, test)` inventories and
counts, and fails on an empty set, any difference, or a duplicate shard member.
This proves selection semantics on the archive's compiled platform; it does
not substitute an ARM64 inventory for the x86_64 Actions run.

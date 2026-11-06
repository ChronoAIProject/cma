# Backend archive verification

Verified on 2026-11-06 against parent `8d2c8c109`, with Rust 1.96.1 and
cargo-nextest 0.9.151 (`6b3f994577a98874a2e0ea5bef77fbbab3434044`).
Use an Ubuntu 24.04 ARM64 Linux container with a disposable repository copy
mounted at `/home/runner/work/cma/cma`. Install the pinned Rust and nextest
versions, Ruby 3.2, Python 3, Git, curl, OpenSSL, zstd and Docker CLI, with access
to a Docker daemon for owned Mongo fixtures. Keep proof files in `/proof`, outside
the mounted checkout. No Rust source or test was changed by the CI split.

## Archive execution

From the container's `api/` directory:

```sh
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 \
  CARGO_BUILD_JOBS=1 RUST_MIN_STACK=33554432 \
  cargo nextest archive --workspace --locked --archive-file /proof/backend-tests.tar.zst
```

The cold build took 14m23s; archiving took 4.82s. These are validation timings, not an Actions
performance measurement. The archive contains 133 files, 98 test
binaries and four non-test executables. Its compressed size is 724,027,291 bytes (about 690 MiB);
the tar stream is 2,858,985,472 bytes. Its non-test metadata contains:

```text
target/debug/cmaeg
target/debug/cmaeg-media-probe
target/debug/layer4-runtime
target/debug/cma-image-manager
```

Archive SHA-256:
`6002175732d1cbd39b5fee73688cb5309dcc481f2491266d7bbaa0cdd26f2ab8`.

Use a fresh consumer container with the same repository revision mounted at the
same absolute path and an absent `api/target`, or remove the producer's disposable
`api/target` after archiving. Copy the archive into `/proof` and use the exact CI
extraction command:

```sh
cargo nextest list --archive-file /proof/backend-tests.tar.zst --extract-to "$PWD" --list-type binaries-only
test -x target/debug/cmaeg
test -x target/debug/cmaeg-media-probe
test -x target/debug/layer4-runtime
export CMA_NEXTEST_REUSE=1 RUST_MIN_STACK=33554432
```

Extraction succeeded in 7.65s. All subsequent commands reused that extraction.
The archive also passed `zstd -t /proof/backend-tests.tar.zst`.

| Archive invocation | Result |
| --- | --- |
| `bash ../.github/scripts/backend-nextest.sh run -E 'package(=cmaeg) and binary(=entrypoint) and test(=unknown_arguments_print_the_usage_line_and_exit_two)' --success-output final` | 1 passed; executes compile-time `CARGO_BIN_EXE_cmaeg`. |
| `bash ../.github/scripts/backend-nextest.sh run -E 'binary(=media_image_peak)' --test-threads 1 --success-output final` | 1 passed in 28.676s. |
| `bash ../.github/scripts/backend-nextest.sh run -E 'binary(=turn_metrics)' --test-threads 1 --success-output final` | 1 passed in 0.010s. |
| `bash ../.github/scripts/backend-nextest.sh run -E 'package(=layer4-runtime) and test(=supervisor::mutation_tests::workspace_reconciliation_tests::real_linux_writer_boundary_reconciles_target_and_resumes_live_sibling)' --run-ignored only --test-threads 1` | 1 passed in 0.152s. |
| `bash ../.github/scripts/backend-nextest.sh run -E 'package(=cmaeg) and binary(=consumer_readers)' --success-output final` | 3 passed; covers manifest-relative source and `CARGO_TARGET_TMPDIR`. |
| `bash ../.github/scripts/backend-nextest.sh run --run-ignored only -E 'package(=cmaeg) and test(~latency_budget)' --success-output final` | 2 passed on the first attempt in 40.841s; produced nonempty `api/target/cmaeg-latency.txt`. |
| `bash ../.github/scripts/backend-nextest.sh run -E 'package(=cmaeg) and test(=__cma_archive_missing_selection__)'` | Exit 4; empty exact selections fail. |

To exercise the gateway's default child lookup without the browser campaign,
reuse the authenticated fixture setup and cleanup from the Image Manager runner.
From `api/`, generate a disposable proof script that replaces only its test tail:

```sh
python3 - <<'PY'
from pathlib import Path
import shlex

scripts = Path.cwd().parent / ".github/scripts"
source = (scripts / "image-manager-mongo-contract.sh").read_text()
setup = source.split('cd "$repo_root/api"\n', 1)[0]
directory = 'script_directory=$(dirname -- "${BASH_SOURCE[0]}")'
assert setup.count(directory) == 1
setup = setup.replace(directory, "script_directory=" + shlex.quote(str(scripts)))
command = '''cd "$repo_root/api"
nextest_build run --run-ignored only --test-threads 1 --success-output final \\
  -E 'package(=cma) and test(=gateway::tests::key_expiry::mint_accepts_provider_fractional_zero_expiry_and_returns_canonical_time)'
'''
Path("/proof/gateway-proof.sh").write_text(setup + command)
PY
env -u CMAEG_TEST_BINARY bash /proof/gateway-proof.sh
```

With `CMAEG_TEST_BINARY` unset, it passed in 0.339s against the owned pinned Mongo
replica set and cleaned that fixture. This executes the gateway fixture's
`current_exe().parent().parent().join("cmaeg")` path. The two timing probes and
Linux writer-boundary test ran sequentially without other local test work.

## Selection inventories

From a checkout containing the parent revision, export the baseline scripts:

```sh
mkdir -p /proof/baseline
git show 8d2c8c109:.github/scripts/image-manager-mongo-contract.sh > /proof/baseline/image-manager-mongo-contract.sh
git show 8d2c8c109:.github/scripts/mongo-contract-tests.sh > /proof/baseline/mongo-contract-tests.sh
```

Then, from `api/` in the consumer container:

```sh
CMA_NEXTEST_REUSE=1 python3 ../.github/scripts/backend-selection-proof.py /proof/baseline /proof/selections
```

Every old/new pair was equal and nonempty. These counts describe the compiled
ARM64 inventory. Original `cargo test` filters were enumerated through libtest's
own `--list`; original nextest filters were enumerated through nextest. The proof emits
sorted `(binary, test)` inventories and `/proof/selections/summary.json`.

| Selection | Old | New |
| --- | ---: | ---: |
| Main suite / union of shards | 7,327 | 7,327 |
| Image normalization budget | 1 | 1 |
| Gateway socket budget | 1 | 1 |
| Exact ignored Linux writer boundary | 1 | 1 |
| Disposable Mongo contracts | 499 | 499 |
| Image Manager `mongo_isolation` + `browser_session` | 3 | 3 |
| CMAEG lib `optional_forms_storage` | 1 | 1 |
| CMAEG lib `optional_media_continuation_storage` | 1 | 1 |
| Exact worker-history Mongo contract | 1 | 1 |
| CMAEG lib `whatsapp_retention_on_real_mongo` | 1 | 1 |
| CMAEG `mongo_isolation` | 3 | 3 |
| CMAEG `management`, `github_integrations::` | 24 | 24 |
| CMAEG `github_source` | 19 | 19 |
| CMAEG lib `tools::threads::ledger::mongo_tests::` | 1 | 1 |
| CMA `store::isolation::tests::real_` | 1 | 1 |
| CMAEG `management`, `binary::compiled_binary_`, including ignored | 4 | 4 |
| CMAEG `relay_ingress`, `runner_binary::` | 5 | 5 |
| CMA `gateway::tests::`, ignored only | 8 | 8 |
| CMAEG latency, ignored only | 2 | 2 |

The main shard sizes were 2,468, 2,442 and 2,417. Every selected test occurred
exactly once; duplicate count was zero.

## Workflow and script checks

The four Ruby commands pass on Ruby 2.6.10 and on Ruby 3.2.3 in Ubuntu 24.04,
matching the [published runner manifest](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Readme.md).
The contract helper uses an ordinary optional hash so string-keyed attributes
have the same calling convention on both Ruby versions:

```sh
ruby .github/scripts/ci-contract.rb provider
ruby .github/scripts/ci-contract.rb release
ruby .github/scripts/ci-contract-tests.rb provider
ruby .github/scripts/ci-contract-tests.rb release
```

Each implementation commit was checked from its staged snapshot. The dual-mode
runner commit retained the original workflow and passed 31 Node tests with
`CI=true` source fixtures, all four Ruby commands on both versions, actionlint
and ShellCheck. The workflow migration added the CI archive guard and its
rejection test together, passing 32 Node tests and the same checks. Its design
docs landed with the behavior; the proof script and verification record landed
in the following commit.

Both validators passed. Provider regressions rejected 477 mutations and executed
the real aggregate command against success, failure, cancelled, skipped and
missing dependency results. Release regressions rejected 108 mutations.

```sh
node --test .github/scripts/mongo-contract-tests.test.mjs .github/scripts/image-manager-mongo-contract.test.mjs
node --test .github/scripts/authenticated-mongo-readiness.test.mjs .github/scripts/check-api-docs.test.mjs .github/scripts/check-config-docs.test.mjs
.github/scripts/check-file-size.sh
.github/scripts/check-file-size-tests.sh
.github/scripts/check-config-keys.sh
.github/scripts/check-config-keys-tests.sh
node .github/scripts/check-config-docs.mjs
node .github/scripts/check-api-docs.mjs api/crates/cmaeg/openapi/v1.json docs/CMAEG_Protocol.md
git diff --check
```

Results: 32 script tests passed, exercising every existing Mongo mock scenario in
both source and archive modes, every Image Manager selection, and owned/foreign/
failing cleanup cases; 14 readiness/docs tests passed; 5,281 source files passed size checks; both shell
validator suites passed 11 regressions each. Configuration keys matched
111/111, 184/184 and 12/12; docs covered all quoted config keys and 52 OpenAPI
paths. Whitespace checks passed.

Run actionlint from the repository root:

```sh
docker run --rm -v "$PWD:/repo" -w /repo rhysd/actionlint:latest .github/workflows/ci.yml
```

Actionlint 1.7.12 passed with no findings. ShellCheck also passed for all three
runner scripts with source following enabled.

`gitleaks git --redact --no-banner --log-opts=8d2c8c109..HEAD .` scanned the three
commits and found no leaks. `git diff 8d2c8c109 --check` passed as well.

## Evidence boundary

A real GitHub Actions run must still verify x86_64-only tests, the complete job
matrix and browser/Mongo campaigns on hosted runners, artifact upload/download
and failed-job reruns, cache hits/saves, and the proposed 25–33 minute wall-clock
target. Local inventory equality and representative archive execution do not
claim those remote results. No branch protection or remote GitHub state changed.

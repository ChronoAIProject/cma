# frozen_string_literal: true

# The required Backend check is a fail-closed join. Keep its complete execution
# graph reviewable here: extra steps can compete with timing probes, override
# environment, or bypass the extracted build just as missing steps can weaken it.
module CIContract
  BACKEND_NEEDS = %w[backend-lint backend-test-build backend-tests backend-timing backend-mongo backend-authenticated-mongo].freeze
  NEXTEST = "bash ../.github/scripts/backend-nextest.sh"
  BACKEND_RESULTS = <<~'SH'
    python3 - <<'PYTHON'
    import json
    import os
    import sys
    results = json.loads(os.environ["NEEDS"])
    expected = ["backend-lint", "backend-test-build", "backend-tests", "backend-timing", "backend-mongo", "backend-authenticated-mongo"]
    failed = {job: results.get(job, {}).get("result", "missing") for job in expected if results.get(job, {}).get("result") != "success"}
    for job, result in failed.items():
        print(f"::error::{job}: {result}")
    sys.exit(bool(failed))
    PYTHON
  SH

  def self.backend_step(name, run, attributes = {})
    { "name" => name, "run" => run }.merge(attributes.transform_keys(&:to_s))
  end

  def self.backend_jobs(ci)
    checkout = { "uses" => "actions/checkout@v5" }
    capacity = backend_step("Prepare Backend capacity",
      'sudo --preserve-env=GITHUB_ACTIONS,RUNNER_ENVIRONMENT,RUNNER_OS python3 .github/scripts/backend-capacity.py || echo "::warning::Backend capacity preparation unavailable"',
      "working-directory" => ".")
    toolchain = { "uses" => "dtolnay/rust-toolchain@stable" }
    install = { "name" => "Install pinned nextest", "uses" => "taiki-e/install-action@v2",
      "with" => { "tool" => "cargo-nextest@0.9.151", "fallback" => "none" } }
    # No key overrides: rust-cache includes the distinct job ID by default.
    cache = { "uses" => "Swatinem/rust-cache@v2", "with" => {
      "workspaces" => "api",
      "save-if" => "${{ github.event_name == 'push' && github.ref == 'refs/heads/develop' }}",
    } }
    stack = { "RUST_MIN_STACK" => 33554432 }
    compile = stack.merge("CARGO_PROFILE_DEV_DEBUG" => "0", "CARGO_PROFILE_TEST_DEBUG" => "0")
    defaults = { "run" => { "working-directory" => "api" } }
    download = { "name" => "Download Backend test archive", "uses" => "actions/download-artifact@v4",
      "with" => { "name" => "backend-tests", "path" => "${{ runner.temp }}" } }
    extract = backend_step("Extract Backend test archive once", <<~'SH')
      cargo nextest list --archive-file "$RUNNER_TEMP/backend-tests.tar.zst" --extract-to "$PWD" --list-type binaries-only
      test -x target/debug/cmaeg
      test -x target/debug/cmaeg-media-probe
      test -x target/debug/layer4-runtime
    SH
    setup = [checkout, toolchain, install]
    consumers = setup + [download, extract]
    expected = {
      "backend-lint" => {
        "name" => "Backend lint", "timeout-minutes" => 45, "env" => compile,
        "steps" => [checkout, capacity, toolchain.merge("with" => { "components" => "rustfmt, clippy" }),
          { "name" => "Install pinned dependency auditor", "uses" => "taiki-e/install-action@v2",
            "with" => { "tool" => "cargo-deny@0.20.2", "fallback" => "none" } },
          backend_step("Dependency audit", "cargo deny --locked --all-features check advisories licenses bans sources"), cache,
          backend_step("Format", "cargo fmt --all --check"),
          backend_step("Desktop browser launch and ownership regressions", "python3 crates/layer4-runtime/src/desktop-browser-test.py"),
          backend_step("Native recovery persistence and process fences", "python3 crates/layer3-adapter/src/codex/recovery_test.py"),
          backend_step("Retained transport native goal read-only proof", "python3 crates/layer3-adapter/src/hosted/managed_context/transport_goal_test.py"),
          backend_step("Lint", "cargo clippy --all-targets --all-features --locked -- -D warnings"),
          backend_step("Build", "cargo build --workspace --locked"),
          backend_step("Doctests", "cargo test --doc --workspace --locked")],
      },
      "backend-test-build" => {
        "name" => "Backend test build", "timeout-minutes" => 45, "env" => compile,
        "steps" => [checkout, capacity, toolchain, cache, install,
          backend_step("Archive workspace tests", 'cargo nextest archive --workspace --locked --archive-file "$RUNNER_TEMP/backend-tests.tar.zst"'),
          { "name" => "Upload Backend test archive", "uses" => "actions/upload-artifact@v4", "with" => {
            "name" => "backend-tests", "path" => "${{ runner.temp }}/backend-tests.tar.zst",
            "overwrite" => true, "retention-days" => 1, "compression-level" => 0, "if-no-files-found" => "error",
          } }],
      },
      "backend-tests" => {
        "name" => "Backend tests (${{ matrix.shard }}/3)", "timeout-minutes" => 45,
        "strategy" => { "fail-fast" => false, "matrix" => { "shard" => [1, 2, 3] } },
        "steps" => consumers + [backend_step("Tests",
          "#{NEXTEST} run --no-fail-fast -E 'not (binary(=media_image_peak) or binary(=turn_metrics))' --partition \"count:$SHARD/3\"",
          env: { "SHARD" => "${{ matrix.shard }}" })],
      },
      "backend-timing" => {
        "name" => "Backend timing budgets", "timeout-minutes" => 35,
        "steps" => consumers + [
          backend_step("Image normalization budgets", "#{NEXTEST} run -E 'binary(=media_image_peak)' --test-threads 1 --success-output final"),
          backend_step("Gateway socket latency budget", "#{NEXTEST} run -E 'binary(=turn_metrics)' --test-threads 1 --success-output final"),
          backend_step("Isolated Linux Workspace removal writer boundary", "#{NEXTEST} run -E 'package(=layer4-runtime) and test(=supervisor::mutation_tests::workspace_reconciliation_tests::real_linux_writer_boundary_reconciles_target_and_resumes_live_sibling)' --run-ignored only --test-threads 1")],
      },
      "backend-mongo" => {
        "name" => "Backend Mongo contracts", "timeout-minutes" => 90,
        "steps" => consumers + [backend_step("Disposable real Mongo persistence contracts",
          "bash ../.github/scripts/mongo-contract-tests.sh", "timeout-minutes" => 75)],
      },
      "backend-authenticated-mongo" => {
        "name" => "Backend authenticated Mongo contracts", "timeout-minutes" => 30,
        "steps" => consumers + [{ "uses" => "actions/setup-node@v6", "with" => { "node-version" => "24.15.0" } },
          backend_step("Prepare compiled gateway browser contract", "npm ci --prefix web\nnpm run build --prefix web\n", "working-directory" => "."),
          backend_step("Install gateway contract browser", "node --test ../.github/scripts/install-contract-browser.test.mjs\nnode ../.github/scripts/install-contract-browser.mjs\n", "timeout-minutes" => 10, "working-directory" => "web"),
          backend_step("Run authenticated Mongo contracts", "bash ../.github/scripts/authenticated-mongo-contract.sh", "timeout-minutes" => 15)],
      },
      "cmaeg-latency" => {
        "name" => "CMAEG latency budget", "timeout-minutes" => 15,
        "steps" => consumers + [backend_step("Enforce the production budgets (one retry)", <<~'SH'),
          bash ../.github/scripts/backend-nextest.sh run --run-ignored only -E 'package(=cmaeg) and test(~latency_budget)' --success-output final || \
            bash ../.github/scripts/backend-nextest.sh run --run-ignored only -E 'package(=cmaeg) and test(~latency_budget)' --success-output final
        SH
          { "uses" => "actions/upload-artifact@v4", "if" => "always()", "with" => {
            "name" => "cmaeg-latency", "path" => "api/target/cmaeg-latency.txt", "if-no-files-found" => "warn",
          } }],
      },
    }
    expected.each do |id, wanted|
      wanted.merge!("runs-on" => "ubuntu-latest", "defaults" => defaults)
      unless %w[backend-lint backend-test-build].include?(id)
        wanted.merge!("needs" => "backend-test-build", "env" => stack.merge("CMA_NEXTEST_REUSE" => "1"))
      end
      if BACKEND_NEEDS.include?(id)
        wanted["env"] = wanted.fetch("env").merge("BASH_ENV" => "${{ github.workspace }}/.github/scripts/backend-resource-env.sh")
      end
      actual = job(ci, id, wanted.fetch("name"))
      require!(actual == wanted, "#{id}: Backend execution, isolation, archive, cache or bounds changed")
    end
    aggregator = job(ci, "backend", "Backend")
    require!(aggregator == {
      "name" => "Backend", "needs" => BACKEND_NEEDS, "if" => "always()",
      "runs-on" => "ubuntu-latest", "timeout-minutes" => 5,
      "steps" => [backend_step("Require every Backend job to succeed", BACKEND_RESULTS,
        env: { "NEEDS" => "${{ toJSON(needs) }}" })],
    }, "Backend must always require exactly its mandatory jobs to succeed; latency remains advisory")
  end
end

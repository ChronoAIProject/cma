# frozen_string_literal: true

require "yaml"

module CIContract
  ROOT = File.expand_path("../..", __dir__)
  class Refusal < StandardError; end

  def self.require!(condition, message)
    raise Refusal, message unless condition
  end

  def self.load_inputs
    Dir.glob(File.join(ROOT, ".github/workflows/*.{yml,yaml}")).to_h do |path|
      [File.basename(path, File.extname(path)), YAML.safe_load(File.read(path), aliases: false)]
    end
  end

  def self.job(workflow, id, name)
    found = workflow.fetch("jobs").fetch(id)
    require!(found["name"] == name, "#{id}: exact check name changed")
    require!(!found.key?("continue-on-error"), "#{id}: job cannot be optional")
    found
  end

  def self.step(job, name, run = nil, condition = nil)
    found = job.fetch("steps").select { |row| row["name"] == name }
    require!(found.length == 1, "missing or duplicate step: #{name}")
    found = found.first
    require!(!found.key?("continue-on-error") && found["if"] == condition, "optional or skipped step: #{name}")
    require!(found["run"] == run, "command changed: #{name}") if run
    found
  end

  def self.order(job, names)
    positions = names.map { |name| job.fetch("steps").index { |row| row["name"] == name } }
    require!(positions.none?(&:nil?) && positions == positions.sort && positions.uniq == positions, "execution order changed: #{names.join(', ')}")
  end

  def self.artifact(job, condition, prefix)
    upload = step(job, "Retain runtime image evidence", nil, condition)
    require!(upload["uses"] == "actions/upload-artifact@v4", "runtime evidence uploader changed")
    require!(upload["with"] == {
      "name" => "#{prefix}-${{ github.run_id }}-${{ github.run_attempt }}",
      "path" => "${{ runner.temp }}/runtime-image-evidence.json",
      "if-no-files-found" => "error", "retention-days" => 90,
    }, "runtime evidence must remain exact, required and retained")
  end

  def self.native_image_artifacts(images)
    condition = "success() && (github.event_name == 'push' || github.event.pull_request.head.repo.full_name == github.repository)"
    export = step(images, "Export tested native image", 'python3 .github/scripts/export-ci-native-image.py "$IMAGE_COMPONENT" "$GITHUB_SHA" "$RUNNER_TEMP/ci-native-image"', condition)
    require!(export["timeout-minutes"] == 10 && export["env"] == { "IMAGE_COMPONENT" => "${{ matrix.component }}" }, "native export must retain exact component and timeout")
    upload = step(images, "Retain tested native image", nil, condition)
    require!(upload["uses"] == "actions/upload-artifact@v4" && upload["timeout-minutes"] == 10, "native image upload changed")
    require!(upload["with"] == {
      "name" => "ci-native-image-${{ matrix.component }}-${{ github.sha }}-${{ github.run_id }}-${{ github.run_attempt }}",
      "path" => "${{ runner.temp }}/ci-native-image/",
      "if-no-files-found" => "error", "compression-level" => 0, "retention-days" => 1,
    }, "native images must remain exact, required and short-lived")
    order(images, ["Vulnerability scan", "Require passing runtime image evidence", "Export tested native image", "Retain tested native image"])
  end

  def self.release(inputs)
    ci = inputs.fetch("ci")
    publish = inputs.fetch("publish")
    gate = job(ci, "trigger-release-contract", "Trigger release contract")
    require!(!gate.key?("if") && !gate.key?("needs"), "release contract must execute on every PR")
    step(gate, "Release workflow and Gateway source contracts", "ruby .github/scripts/ci-contract.rb release")
    step(gate, "Release contract mutation regressions", "ruby .github/scripts/ci-contract-tests.rb release")
    step(gate, "Runtime image evidence and provenance regressions", "node --test .github/scripts/runtime-image-evidence.test.mjs .github/scripts/release-runtime-input.test.mjs")
    images = job(ci, "images", "Images")
    require!(!images.key?("if") && images.dig("strategy", "matrix", "component") == %w[api web runtime cmaeg], "all image gates must execute")
    step(images, "Execute built runtime image contract", 'node .github/scripts/runtime-image-evidence.mjs run cma-runtime:ci "$GITHUB_SHA" "$RUNNER_TEMP/runtime-image-evidence.json"', "matrix.component == 'runtime'")
    step(images, "Require passing runtime image evidence", 'node .github/scripts/runtime-image-evidence.mjs verify cma-runtime:ci "$GITHUB_SHA" "$RUNNER_TEMP/runtime-image-evidence.json"', "always() && matrix.component == 'runtime'")
    artifact(images, "always() && matrix.component == 'runtime'", "runtime-image-evidence")
    order(images, ["Load native image for scanning", "Vulnerability scan", "Execute built runtime image contract", "Retain runtime image evidence", "Require passing runtime image evidence"])
    native_image_artifacts(images)
    runtime = job(publish, "runtime-image-gate", "Runtime image evidence")
    require!(runtime["needs"] == %w[plan images image-exports] && !runtime.key?("if"), "release runtime dependency authority changed")
    require!(runtime["permissions"] == { "contents" => "read", "packages" => "read" }, "runtime evidence cannot publish")
    step(runtime, "Verify exact runtime artifact bytes and source", 'node .github/scripts/release-runtime-input.mjs "$RUNNER_TEMP/runtime-export" "$GITHUB_SHA" "$RELEASE_VERSION"')
    step(runtime, "Pull verified immutable runtime", 'docker pull "$RUNTIME_REFERENCE"')
    step(runtime, "Execute published runtime image contract", 'node .github/scripts/runtime-image-evidence.mjs run "$RUNTIME_REFERENCE" "$GITHUB_SHA" "$RUNNER_TEMP/runtime-image-evidence.json"')
    step(runtime, "Require passing runtime image evidence", 'node .github/scripts/runtime-image-evidence.mjs verify "$RUNTIME_REFERENCE" "$GITHUB_SHA" "$RUNNER_TEMP/runtime-image-evidence.json"', "always()")
    %w[Pull\ verified\ immutable\ runtime Execute\ published\ runtime\ image\ contract Require\ passing\ runtime\ image\ evidence].each do |name|
      require!(step(runtime, name, nil, name.start_with?("Require") ? "always()" : nil).dig("env", "RUNTIME_REFERENCE") == "${{ steps.runtime.outputs.reference }}", "runtime must use verified immutable reference")
    end
    artifact(runtime, "always()", "release-runtime-image-evidence")
    order(runtime, ["Verify exact runtime artifact bytes and source", "Pull verified immutable runtime", "Execute published runtime image contract", "Retain runtime image evidence", "Require passing runtime image evidence"])
    release = job(publish, "release", "Tag and release")
    require!(release["needs"] == %w[plan images image-exports runtime-image-gate] && !release.key?("if"), "tag/release must require passing runtime evidence")
    structure = job(ci, "structure", "Repository structure")
    %w[release-notes plan-release reconcile-release-image assert-release-image-tags-absent reconcile-github-release export-release-image import-release-images skopeo-transport].each do |script|
      require!(structure.fetch("steps").any? { |row| row["run"].to_s.match?(%r{\A(?:bash )?\.github/#{script}-tests\.sh\z}) && !row.key?("if") && !row.key?("continue-on-error") }, "release helper coverage removed: #{script}")
    end
    writers = publish.fetch("jobs").select { |_id, row| row.dig("permissions", "packages") == "write" }.keys
    require!(writers == ["images"] && publish.dig("jobs", "images", "permissions") == { "contents" => "read", "packages" => "write" }, "exclusive image writer changed")
    grants = []
    inputs.each do |name, workflow|
      scopes = { "workflow" => workflow["permissions"] }.merge(workflow.fetch("jobs").to_h { |id, row| [id, row["permissions"]] })
      scopes.each do |id, permissions|
        grants << "#{name}:#{id}" if permissions == "write-all" || (permissions.is_a?(Hash) && permissions["packages"] == "write")
      end
    end
    require!(grants == ["publish:images"], "all workflows must retain one exclusive package writer")
    require!(publish.dig("jobs", "images", "steps").map { |row| row["uses"] }.compact == [
      "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09",
      "docker/login-action@c94ce9fb468520275223c153574b00df6fe4bcc9",
      "docker/setup-qemu-action@c7c53464625b32c7a7e944ae62b3e17d2b600130",
      "docker/setup-buildx-action@8d2750c68a42422c14e847fe6c8ac0403b4cbd6f",
      "docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8",
      "aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25",
      "docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8",
    ], "every action with package write authority must remain pinned")
    # Live Gateway adoption evidence is unresolved. Release automation must not
    # gain a deployment/mutation path as a consequence of passing these checks.
    publish.fetch("jobs").each_value do |row|
      require!(!row.key?("environment"), "release cannot acquire a deployment environment")
      row.fetch("steps").each do |entry|
        require!(!entry["run"].to_s.match?(/\b(kubectl|gcloud|terraform|helm)\b/), "release must not mutate live routing or infrastructure")
      end
    end
  end

  def self.provider(inputs)
    ci = inputs.fetch("ci")
    workflow = inputs.fetch("nightly-staging")
    gate = job(ci, "trigger-provider-smoke-contract", "Trigger provider smoke contract")
    require!(!gate.key?("if") && !gate.key?("needs"), "provider contract must execute on every PR")
    {
      "Provider workflow authority contract" => "ruby .github/scripts/ci-contract.rb provider",
      "Provider contract mutation regressions" => "ruby .github/scripts/ci-contract-tests.rb provider",
      "Staging authority and evidence regressions" => "node --test .github/scripts/staging-authority.test.mjs .github/scripts/deployment-observer.test.mjs .github/scripts/cma-bridge-continuity-observer.test.mjs .github/scripts/cma-bridge-continuity-transport.test.mjs .github/scripts/cma-bridge-continuity-watch.test.mjs .github/scripts/cma-bridge-continuity-owned.test.mjs .github/scripts/cma-bridge-continuity-recovery.test.mjs .github/scripts/cma-bridge-continuity-successor.test.mjs .github/scripts/cma-bridge-continuity-active.test.mjs .github/scripts/cma-bridge-continuity-browser.test.mjs .github/scripts/mongo-contract-tests.test.mjs",
      "Install continuity fixture tooling" => "python3 -m pip install -r .github/tools/cma-bridge-continuity/requirements.txt",
      "Continuity fixture ownership and isolation regressions" => "python3 -m unittest discover -s .github/tools/cma-bridge-continuity -p 'test_*.py' -v",
      "Typecheck every harness entry point" => "npm run e2e:typecheck",
      "Exact remote campaign inventory remains mandatory" => "node .github/scripts/staging-authority.mjs inventory",
      "Fail-closed fixtures and cleanup mechanics" => "npm run test -- e2e/environment.test.ts e2e/owner-mechanics.test.ts",
      "Install pinned local browser" => "npx --no-install playwright install --with-deps chromium",
      "Local browser guard mechanics" => "npm run e2e:mechanics",
    }.each { |name, command| step(gate, name, command) }
    backend = job(ci, "backend", "Backend")
    mongo = step(backend, "Disposable real Mongo persistence contracts", "bash ../.github/scripts/mongo-contract-tests.sh")
    require!(mongo["timeout-minutes"] == 75, "Mongo proof must retain its 75-minute bound")
    sandbox_mongo = step(backend, "Sandbox Manager authenticated Mongo isolation", "bash ../.github/scripts/sandbox-manager-mongo-contract.sh")
    require!(sandbox_mongo["timeout-minutes"] == 10, "Sandbox Manager isolation proof must remain bounded")
    require!(workflow["permissions"] == { "contents" => "read" }, "remote workflow permissions broadened")
    require!(workflow.dig("concurrency", "cancel-in-progress") == false, "remote writers cannot be cancelled by a newer run")
    triggers = workflow["on"] || workflow[true]
    require!(triggers.keys.sort == %w[schedule workflow_dispatch], "remote campaign cannot execute on PR code")
    %w[smoke journeys].each do |id|
      row = workflow.fetch("jobs").fetch(id)
      require!(!row.fetch("env").key?("CMA_E2E_OUTPUT_DIR"), "runner evidence path must be initialized at step runtime")
      command = %Q(printf '%s\\n' "CMA_E2E_OUTPUT_DIR=$RUNNER_TEMP/cma-e2e-#{id}-$GITHUB_RUN_ID-$GITHUB_RUN_ATTEMPT" >> "$GITHUB_ENV")
      initialize = step(row, "Initialize isolated evidence path", command)
      require!(initialize["working-directory"] == ".", "evidence initialization must not depend on a child directory")
      harness = id == "smoke" ? "GET-only smoke, no credentials or hooks" : "Require complete fixtures and execute J0-J7, faults and security"
      order(row, ["Initialize isolated evidence path", harness])
    end
    plan = job(workflow, "plan", "Reviewed staging authority")
    step(plan, "Bind the exact reviewed develop commit and observer authority", "node .github/scripts/staging-authority.mjs plan")
    remote = job(workflow, "journeys", "Explicitly scoped staging journeys")
    require!(remote["needs"] == "plan" && remote["if"] == "github.event_name == 'workflow_dispatch' && inputs.mutation_target != ''", "remote mutation needs explicit approved dispatch")
    require!(remote["environment"] == "staging-e2e-mutation" && remote["runs-on"] == %w[self-hosted linux x64 cma-staging-e2e], "remote writer isolation changed")
    require!(!remote.key?("strategy"), "remote writer must not fan out")
    checkout = remote.fetch("steps").find { |row| row["uses"].to_s.start_with?("actions/checkout@") }
    require!(checkout.dig("with", "ref") == "${{ needs.plan.outputs.sha }}", "remote checkout must use the approved exact SHA")
    require!(remote.dig("env", "VERIFIER_SHA256") == "${{ needs.plan.outputs.verifier }}" && remote.dig("env", "CONFIG_SHA256") == "${{ needs.plan.outputs.config }}", "observer authority must match approval")
    before = "Verify approval policy and independently observed deployment before fixture access"
    after = "Recheck independent deployment identity after the campaign"
    audit = "Require complete cleanup and independently bound campaign evidence"
    upload = "Upload credential-checked screenshots, redacted results and cleanup journal"
    step(remote, before, 'node .github/scripts/staging-authority.mjs attest "$RUNNER_TEMP/staging-before.json"')
    step(remote, "Require complete fixtures and execute J0-J7, faults and security", "npm run e2e")
    step(remote, after, 'node .github/scripts/staging-authority.mjs attest "$RUNNER_TEMP/staging-after.json"', "always() && steps.attestation.outcome == 'success'")
    step(remote, audit, 'node .github/scripts/staging-authority.mjs audit "$CMA_E2E_OUTPUT_DIR" "$RUNNER_TEMP/staging-before.json" "$RUNNER_TEMP/staging-after.json"', "always() && steps.harness.outcome == 'success' && steps.final_attestation.outcome == 'success'")
    transfer = step(remote, upload, nil, "always() && steps.audit.outputs.publishable == 'true'")
    require!(transfer.dig("with", "if-no-files-found") == "error", "missing remote evidence cannot pass")
    require!(transfer.dig("with", "path").lines.map(&:strip).sort == %w[attestation.json cleanup.ndjson provenance.json results.json *.png].map { |name| "${{ env.CMA_E2E_OUTPUT_DIR }}/#{name}" }.sort, "remote artifacts must remain curated")
    order(remote, [before, "Require complete fixtures and execute J0-J7, faults and security", after, audit, upload])
    before_index = remote.fetch("steps").index { |row| row["name"] == before }
    require!(remote.fetch("steps").take(before_index).none? { |row| row["run"].to_s.match?(/npm|playwright|fixture|e2e/) }, "dependencies or fixtures cannot execute before independent attestation")
    [plan, remote].each do |row|
      row.fetch("steps").each do |entry|
        require!(entry["uses"].match?(/@[0-9a-f]{40}\z/), "protected workflow actions must be immutable") if entry["uses"]
      end
    end
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    mode = ARGV.fetch(0)
    CIContract.require!(%w[release provider].include?(mode) && ARGV.length == 1, "expected release or provider")
    CIContract.public_send(mode, CIContract.load_inputs)
    require_relative "gateway-source-contract" if mode == "release"
    puts "#{mode} executable workflow contract passed"
  rescue StandardError => error
    warn "CI contract: #{error.message}"
    exit 1
  end
end

# frozen_string_literal: true

require_relative "workflow-yaml"
require_relative "backend-ci-contract"
require_relative "ledger-ci-contract"

module CIContract
  ROOT = File.expand_path("../..", __dir__)
  class Refusal < StandardError; end

  def self.require!(condition, message)
    raise Refusal, message unless condition
  end

  def self.load_inputs
    Dir.glob(File.join(ROOT, ".github/workflows/*.{yml,yaml}")).to_h do |path|
      [File.basename(path, File.extname(path)), WorkflowYAML.load_file(path)]
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
    expected = prefix.start_with?("release-") ? "actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9" : "actions/upload-artifact@v4"
    require!(upload["uses"] == expected, "runtime evidence uploader changed")
    require!(upload["with"] == {
      "name" => "#{prefix}-${{ github.run_id }}-${{ github.run_attempt }}",
      "path" => "${{ runner.temp }}/runtime-image-evidence.json",
      "if-no-files-found" => "error", "retention-days" => 90,
    }, "runtime evidence must remain exact, required and retained")
  end

  def self.native_image_artifacts(images)
    condition = "success() && (github.event_name == 'push' || github.event_name == 'merge_group' || github.event.pull_request.head.repo.full_name == github.repository)"
    export = step(images, "Export tested native image", 'python3 .github/scripts/export-ci-native-image.py "$IMAGE_COMPONENT" "$GITHUB_SHA" "$RUNNER_TEMP/ci-native-image"', condition)
    require!(export["timeout-minutes"] == 10 && export["env"] == { "IMAGE_COMPONENT" => "${{ matrix.component }}" }, "native export must retain exact component and timeout")
    upload = step(images, "Retain tested native image", nil, condition)
    require!(upload["uses"] == "actions/upload-artifact@v4" && upload["timeout-minutes"] == 10, "native image upload changed")
    require!(upload["with"] == {
      "name" => "ci-native-image-${{ matrix.component }}-${{ github.sha }}-${{ github.run_id }}-${{ github.run_attempt }}",
      "path" => "${{ runner.temp }}/ci-native-image/",
      "if-no-files-found" => "error", "compression-level" => 0, "retention-days" => 1,
    }, "native images must remain exact, required and short-lived")
    # The exported CI image gets the staging publication secret scan before it
    # is retained, so a blocking finding fails the pull request or merge group.
    scan_if = "success() && (github.event_name == 'merge_group' || (github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository))"
    install = step(images, "Install pinned image secret scanner",
      "bash .github/scripts/staging_cd_transport.sh\nbash .github/scripts/install-image-gitleaks.sh\n", scan_if)
    require!(install["timeout-minutes"] == 5 && install["env"] == {
      "CD_COMPONENT" => "${{ matrix.component == 'api' && 'am' || matrix.component == 'cmaeg' && 'eg' || matrix.component }}",
    }, "image scanner install must keep its component and bound")
    scan = step(images, "Scan tested image as staging publication does",
      'python3 -B .github/scripts/ci_image_secret_scan.py "$IMAGE_COMPONENT" "$RUNNER_TEMP/ci-native-image" "$GITHUB_SHA"', scan_if)
    require!(scan["timeout-minutes"] == 30 && scan["env"] == { "IMAGE_COMPONENT" => "${{ matrix.component }}" },
      "CI image secret scan must keep its component and bound")
    order(images, ["Vulnerability scan", "Require passing runtime image evidence", "Export tested native image",
      "Install pinned image secret scanner", "Scan tested image as staging publication does", "Retain tested native image"])
  end

  def self.api_cargo_cache(ci, images)
    base = "matrix.component == 'api' && steps.api-cargo-cache.outcome == 'success'"
    prepare = step(images, "Prepare API Cargo cache identity", "python3 .github/scripts/api-cargo-cache.py prepare",
      "matrix.component == 'api' && ((github.event_name == 'push' && github.ref == 'refs/heads/develop') || (github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository) || github.event_name == 'merge_group')")
    require!(prepare["id"] == "api-cargo-cache", "API cache identity changed")
    inject = step(images, "Inject API Cargo cache mounts", "python3 .github/scripts/api-cargo-cache.py inject", base)
    extract = step(images, "Extract API Cargo cache before pruning", "python3 .github/scripts/api-cargo-cache.py extract",
      "github.event_name == 'push' && github.ref == 'refs/heads/develop' && #{base} && steps.api-cargo-cache-restore.outputs.cache-hit != 'true'")
    require!(extract["id"] == "api-cargo-cache-export", "API cache extraction output changed")
    require!([inject, extract].all? { |row| row["timeout-minutes"] == 3 }, "API cache transfers must be bounded")
    revision = "caa296126883cff596d87d8935842f9db880ef25"
    shared = { "path" => "${{ runner.temp }}/cma-api-cargo-cache/data",
      "key" => "${{ steps.api-cargo-cache.outputs.key }}", "enableCrossOsArchive" => false }
    expected = [
      { "name" => "Restore API Cargo cache archive", "if" => base, "id" => "api-cargo-cache-restore",
        "uses" => "actions/cache/restore@#{revision}", "timeout-minutes" => 3, "continue-on-error" => true,
        "with" => shared.merge("restore-keys" => "${{ steps.api-cargo-cache.outputs.prefix }}", "fail-on-cache-miss" => false) },
      { "name" => "Save accepted API Cargo cache archive",
        "if" => "success() && github.event_name == 'push' && github.ref == 'refs/heads/develop' && matrix.component == 'api' && steps.api-cargo-cache-export.outputs.ready == 'true'",
        "uses" => "actions/cache/save@#{revision}", "timeout-minutes" => 3, "continue-on-error" => true, "with" => shared },
    ]
    expected.each do |wanted|
      rows = images.fetch("steps").select { |row| row["name"] == wanted["name"] }
      require!(rows == [wanted], "API cache action authority, isolation or bounds changed: #{wanted['name']}")
    end
    step(job(ci, "trigger-release-contract", "Trigger release contract"), "API Cargo cache bridge regressions",
      "python3 .github/scripts/api-cargo-cache-tests.py")
    %w[Build\ image\ for\ all\ target\ platforms Load\ native\ image\ for\ scanning Vulnerability\ scan].each do |name|
      step(images, name) # A cache hit must never skip the build or vulnerability gate.
    end
    prune = step(images, "Release build cache before image checks")
    require!(prune["env"] == { "IMAGE_COMPONENT" => "${{ matrix.component }}" } && prune["run"] == <<~'SH', "mandatory image-preserving prune changed")
      native_image_id=$(docker image inspect "cma-${IMAGE_COMPONENT}:ci" --format '{{.Id}}')
      docker buildx prune --all --force
      test "$(docker image inspect "cma-${IMAGE_COMPONENT}:ci" --format '{{.Id}}')" = "$native_image_id"
      df -h .
    SH
    order(images, ["Prepare API Cargo cache identity", "Restore API Cargo cache archive", "Inject API Cargo cache mounts",
      "Build image for all target platforms", "Load native image for scanning", "Extract API Cargo cache before pruning",
      "Release build cache before image checks", "Vulnerability scan", "Export tested native image",
      "Retain tested native image", "Save accepted API Cargo cache archive"])
  end

  def self.release(inputs)
    ledger_images(inputs)
    ci = inputs.fetch("ci")
    publish = inputs.fetch("publish")
    gate = job(ci, "trigger-release-contract", "Trigger release contract")
    require!(!gate.key?("if") && !gate.key?("needs"), "release contract must execute on every PR")
    step(gate, "Release workflow and Gateway source contracts", "ruby .github/scripts/ci-contract.rb release")
    step(gate, "Release contract mutation regressions", "ruby .github/scripts/ci-contract-tests.rb release")
    step(gate, "Runtime image evidence and provenance regressions", "node --test .github/scripts/runtime-image-evidence.test.mjs .github/scripts/release-runtime-input.test.mjs")
    step(gate, "Migration matrix evidence regressions", "node --test .github/scripts/migration-matrix.test.mjs")
    step(gate, "Debian desktop fixture regressions", <<~SH)
      python3 .github/scripts/desktop-browser-input-tests.py
      node --test .github/scripts/desktop-browser-page.test.cjs
    SH
    images = job(ci, "images", "Images")
    require!(!images.key?("if") && images.dig("strategy", "matrix", "component") == %w[api web runtime runtime-desktop cmaeg image-manager], "all image gates must execute")
    %w[images image-exports].each do |id|
      require!(publish.dig("jobs", id, "strategy", "matrix", "component") == %w[api web runtime cmaeg image-manager], "release #{id} must include every service image")
    end
    step(images, "Execute built runtime image contract", 'node .github/scripts/runtime-image-evidence.mjs run cma-runtime:ci "$GITHUB_SHA" "$RUNNER_TEMP/runtime-image-evidence.json"', "matrix.component == 'runtime'")
    step(images, "Require passing runtime image evidence", 'node .github/scripts/runtime-image-evidence.mjs verify cma-runtime:ci "$GITHUB_SHA" "$RUNNER_TEMP/runtime-image-evidence.json"', "always() && matrix.component == 'runtime'")
    artifact(images, "always() && matrix.component == 'runtime'", "runtime-image-evidence")
    order(images, ["Load native image for scanning", "Vulnerability scan", "Execute built runtime image contract", "Retain runtime image evidence", "Require passing runtime image evidence"])
    step(images, "Execute desktop image contract", 'node .github/scripts/desktop-runtime-evidence.mjs cma-runtime-desktop:ci "$GITHUB_SHA" "$RUNNER_TEMP/desktop-runtime-evidence.json"', "matrix.component == 'runtime-desktop'")
    desktop = step(images, "Retain desktop image evidence", nil, "always() && matrix.component == 'runtime-desktop'")
    require!(desktop["uses"] == "actions/upload-artifact@v4" && desktop.dig("with", "if-no-files-found") == "error", "desktop execution evidence must be retained")
    order(images, ["Load native image for scanning", "Vulnerability scan", "Execute desktop image contract", "Retain desktop image evidence", "Export tested native image"])
    build = step(images, "Build image for all target platforms")
    require!(build.dig("with", "cache-from") == "type=gha,scope=${{ matrix.component }}" &&
      build.dig("with", "cache-to") == "${{ github.event_name == 'push' && github.ref == 'refs/heads/develop' && format('type=gha,mode=max,scope={0}', matrix.component) || '' }}",
      "image caches must restore per component and export only on develop pushes")
    native_image_artifacts(images)
    api_cargo_cache(ci, images)
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
    staging_package_authority(inputs)
    require!(publish.dig("jobs", "images", "steps").map { |row| row["uses"] }.compact == [
      "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09",
      "docker/login-action@c94ce9fb468520275223c153574b00df6fe4bcc9",
      "docker/setup-qemu-action@c7c53464625b32c7a7e944ae62b3e17d2b600130",
      "docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e",
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

  def self.staging_package_authority(inputs)
    grants = []
    inputs.each do |name, workflow|
      scopes = { "workflow" => workflow["permissions"] }.merge(workflow.fetch("jobs").to_h { |id, row| [id, row["permissions"]] })
      scopes.each do |id, permissions|
        grants << "#{name}:#{id}" if permissions == "write-all" || (permissions.is_a?(Hash) && permissions["packages"] == "write")
      end
    end
    publications = %w[web am eg runtime runtime_omarchy].map { |component| "release_#{component}" }
    allowed = %w[publish:images staging-release:publish omarchy-package-bundle:bundle] + publications.map { |id| "staging-cd:#{id}" }
    require!(grants.sort == allowed.sort,
      "only the reviewed release entry points may grant package write")
    omarchy_package_bundle_authority(inputs.fetch("omarchy-package-bundle"))
    staging = inputs.fetch("staging-cd")
    staging_events = staging["on"] || staging[true]
    require!(staging_events == { "push" => { "branches" => ["develop"] } },
      "staging publication is develop-push only")
    publications.each do |id|
      require!(staging.dig("jobs", id, "uses") == "./.github/workflows/staging-release.yml",
        "staging package authority must delegate to the reviewed publisher")
    end
    writer = inputs.fetch("staging-release").fetch("jobs").fetch("publish")
    require!(writer["permissions"] == { "contents" => "read", "pull-requests" => "read", "actions" => "read", "packages" => "write" } &&
      writer["if"] == "github.event_name == 'push' && github.ref == 'refs/heads/develop' && github.repository == 'ChronoAIProject/cma'",
      "staging publisher authority changed")
  end

  BUNDLE_ABSENT = 'python3 -B .github/scripts/omarchy-package-bundle.py absent --repository "$REPOSITORY" --tag "$TAG"'
  BUNDLE_PUSH = 'skopeo copy --preserve-digests --dest-authfile /auth/config.json "oci:/output/layout:$TAG" "docker://ghcr.io/$REPOSITORY:$TAG"'

  # The package bundle writes one new tag of one package, on an explicit
  # dispatch from develop only, with exactly the reviewed pinned actions.
  # Every step runs and fails the run on failure. The tag is proved absent
  # before the build and again on the line before the only push, which goes
  # to the job-level repository.
  def self.omarchy_package_bundle_authority(workflow)
    events = workflow["on"] || workflow[true]
    require!(events.is_a?(Hash) && events.keys == ["workflow_dispatch"] &&
      events.dig("workflow_dispatch", "inputs").keys == ["snapshot"], "package bundle runs only on dispatch")
    require!(workflow["permissions"] == {} && workflow.fetch("jobs").keys == ["bundle"], "package bundle authority changed")
    bundle = workflow.dig("jobs", "bundle")
    require!(bundle["permissions"] == { "contents" => "read", "packages" => "write" } &&
      bundle["if"] == "github.repository == 'ChronoAIProject/cma' && github.ref == 'refs/heads/develop'" &&
      bundle["runs-on"] == "ubuntu-24.04" && bundle.dig("env", "REPOSITORY") == "chronoaiproject/cma-omarchy-packages",
      "package bundle publisher authority changed")
    require!(bundle.fetch("steps").map { |row| row["uses"] }.compact == [
      "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09",
      "docker/login-action@c94ce9fb468520275223c153574b00df6fe4bcc9",
    ], "every action with package write authority must remain pinned")
    steps = bundle.fetch("steps")
    require!(!bundle.key?("continue-on-error") && steps.none? { |row| row.key?("continue-on-error") || row.key?("if") },
      "every package bundle step must run, and any failure must fail the run")
    redirected = steps.any? do |row|
      env = row.fetch("env", {})
      !env.is_a?(Hash) || env.key?("REPOSITORY") || row["run"].to_s.match?(/\bREPOSITORY\s*=/)
    end
    require!(!redirected, "package bundle publishes only to its job-level repository")
    refuse, publish = "Refuse an existing tag before building", "Publish the exact verified layout"
    order(bundle, ["Resolve the bundle tag from the committed lock", refuse, "Download, verify and lay out the bundle", publish])
    require!(step(bundle, refuse)["run"].to_s.strip == BUNDLE_ABSENT, "package bundle must prove its tag absent before building")
    commands = step(bundle, publish)["run"].to_s.gsub(/[ \t]*\\\n[ \t]*/, " ").lines.map(&:strip)
    script = steps.map { |row| row["run"].to_s }.join("\n")
    require!(commands.count(BUNDLE_PUSH) == 1 && script.scan(/\bskopeo copy\b/).size == 1 &&
      script.scan(%r{docker://[^\s"']*}).uniq == ["docker://ghcr.io/$REPOSITORY:$TAG"],
      "package bundle must push once, and only to ghcr.io/$REPOSITORY:$TAG")
    push = commands.index(BUNDLE_PUSH)
    require!(commands.first == "set -euo pipefail" && commands.count(BUNDLE_ABSENT) == 1 && push.positive? &&
      commands[push - 1] == BUNDLE_ABSENT && script.scan(/omarchy-package-bundle\.py absent /).size == 2,
      "package bundle must prove its tag absent immediately before pushing")
  end

  KEY_EXPIRY_STEPS = [
    { "uses" => "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09", "with" => { "persist-credentials" => false } },
    { "name" => "Check Arch packager key expiry",
      "run" => "python3 -B .github/scripts/omarchy_key_expiry.py --verify-record --fail-days 30" },
  ].freeze

  # The weekly key expiry check fails by name once a key that signs a pinned
  # Omarchy package expires within 30 days, so the failed-run notification
  # reaches someone (#3040), and fails when the committed signing-key record
  # no longer matches the one derived from the locked URLs. It only reads the
  # repository and public package servers, runs on its schedule or an explicit
  # dispatch, and every step runs so that its failure fails the run.
  def self.omarchy_key_expiry_authority(inputs)
    workflow = inputs.fetch("omarchy-key-expiry")
    events = workflow["on"] || workflow[true]
    schedule = events.is_a?(Hash) ? events["schedule"] : nil
    require!(events.is_a?(Hash) && events.keys.sort == %w[schedule workflow_dispatch] && events["workflow_dispatch"] == {} &&
      schedule.is_a?(Array) && schedule.length == 1 && schedule.first.keys == ["cron"] &&
      schedule.first["cron"].to_s.match?(/\A[0-5]?[0-9] (?:1?[0-9]|2[0-3]) \* \* [0-6]\z/),
      "key expiry check runs only weekly or on dispatch")
    require!(workflow["permissions"] == { "contents" => "read" } && workflow.fetch("jobs").keys == ["expiry"],
      "key expiry check authority changed")
    check = job(workflow, "expiry", "Arch packager key expiry")
    require!(check.keys.sort == %w[if name permissions runs-on steps timeout-minutes] &&
      check["if"] == "github.repository == 'ChronoAIProject/cma'" && check["permissions"] == { "contents" => "read" } &&
      check["runs-on"] == "ubuntu-24.04" && check["timeout-minutes"].is_a?(Integer) && check["timeout-minutes"].between?(1, 30),
      "key expiry check must stay read-only, bounded and on a hosted runner")
    require!(check.fetch("steps") == KEY_EXPIRY_STEPS,
      "key expiry check must run exactly the pinned checkout and the unconditional check")
  end

  def self.provider(inputs)
    ci = inputs.fetch("ci")
    require_relative "omarchy-ci-contract"
    OmarchyCIContract.validate(ci)
    workflow = inputs.fetch("nightly-staging")
    gate = job(ci, "trigger-provider-smoke-contract", "Trigger provider smoke contract")
    require!(!gate.key?("if") && !gate.key?("needs"), "provider contract must execute on every PR")
    {
      "Provider workflow authority contract" => "ruby .github/scripts/ci-contract.rb provider",
      "Provider contract mutation regressions" => "ruby .github/scripts/ci-contract-tests.rb provider",
      "Backend capacity preparation regressions" => "python3 .github/scripts/backend-capacity-tests.py",
      "Staging authority and evidence regressions" => "node --test .github/scripts/staging-authority.test.mjs .github/scripts/deployment-observer.test.mjs .github/scripts/mongo-contract-tests.test.mjs",
      "Authenticated Mongo startup regressions" => "node --test .github/scripts/authenticated-mongo-readiness.test.mjs .github/scripts/authenticated-mongo-contract.test.mjs",
      "Typecheck every harness entry point" => "npm run e2e:typecheck",
      "Exact remote campaign inventory remains mandatory" => "node .github/scripts/staging-authority.mjs inventory",
      "Fail-closed fixtures and cleanup mechanics" => "npm run test -- e2e/environment.test.ts e2e/owner-mechanics.test.ts",
      "Install pinned local browser" => "node --test ../.github/scripts/install-contract-browser.test.mjs\nnode ../.github/scripts/install-contract-browser.mjs\n",
      "Local browser guard mechanics" => "npm run e2e:mechanics",
    }.each { |name, command| step(gate, name, command) }
    backend_jobs(ci)
    step(gate, "Backend resource sampling regressions", "python3 .github/scripts/backend-resource-sampler-tests.py")
    require!(workflow["permissions"] == { "contents" => "read" }, "remote workflow permissions broadened")
    require!(workflow.dig("concurrency", "cancel-in-progress") == false, "remote writers cannot be cancelled by a newer run")
    triggers = workflow["on"] || workflow[true]
    require!(triggers.keys.sort == %w[schedule workflow_dispatch], "remote campaign cannot execute on PR code")
    require!(workflow.dig("jobs", "smoke", "env", "CMA_E2E_PUBLIC_ID") ==
      "${{ vars.CMA_E2E_PUBLIC_ID == secrets.CMA_E2E_PUBLIC_ID && secrets.CMA_E2E_PUBLIC_ID || '' }}",
      "smoke public ID must match its referenced redaction mirror")
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
    puts "#{mode} executable workflow contract passed"
  rescue StandardError => error
    warn "CI contract: #{error.message}"
    exit 1
  end
end

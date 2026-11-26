# frozen_string_literal: true

# Keep Omarchy independent of the Backend graph, including its archive split.
module OmarchyCIContract
  EXPORT_IF = "success() && (github.event_name == 'push' || github.event_name == 'merge_group' || github.event.pull_request.head.repo.full_name == github.repository)"
  SCAN_IF = "success() && (github.event_name == 'merge_group' || (github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository))"
  CACHE_MISS = "steps.omarchy-cache-restore.outcome != 'success' || steps.omarchy-cache-restore.outputs.cache-hit != 'true'"
  DEVELOP_PUSH = "github.event_name == 'push' && github.ref == 'refs/heads/develop'"
  CACHE_WRITE = "#{DEVELOP_PUSH} && (#{CACHE_MISS})"
  CACHE_PATH = "${{ runner.temp }}/omarchy-layer-cache"
  CACHE_KEY = "omarchy-layer-archive-v1-${{ steps.cache.outputs.scope }}"

  def self.validate(ci)
    contract = CIContract
    triggers = ci["on"] || ci[true]
    contract.require!(triggers == {
      "pull_request" => { "branches" => %w[develop main] },
      "push" => { "branches" => ["develop", "tl/release/**"] },
    }, "Omarchy must run on develop, isolated release pushes and ordinary PRs")
    contract.require!(ci["concurrency"] == {
      "group" => "ci-${{ github.workflow }}-${{ github.ref }}",
      "cancel-in-progress" => "${{ !(github.event_name == 'push' && github.ref == 'refs/heads/develop') }}",
    }, "CI cancellation must remain isolated by the exact workflow and ref, and spare develop cache writers")
    row = contract.job(ci, "omarchy", "Images (runtime-omarchy)")
    contract.require!(row.keys.sort == %w[name permissions runs-on steps timeout-minutes].sort,
      "Omarchy cannot acquire conditional, dependency, matrix or deployment authority")
    contract.require!(row["runs-on"] == "ubuntu-24.04" && row["timeout-minutes"] == 150 &&
      row["permissions"] == { "contents" => "read" }, "Omarchy requires bounded native amd64 read-only CI")
    expected = [
      { "uses" => "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09" },
      { "name" => "Reserve space on the disposable Omarchy runner", "run" => <<~'SH' },
        test "$GITHUB_ACTIONS" = true
        sudo rm -rf /usr/local/lib/android /usr/share/dotnet /opt/ghc
        df -h /
      SH
      { "uses" => "actions/setup-node@v6", "with" => { "node-version" => 26, "package-manager-cache" => false } },
      { "name" => "Omarchy desktop fixture regressions", "run" => <<~'SH' },
        python3 -B .github/scripts/omarchy-cache-key-tests.py
        python3 -B .github/scripts/runtime_image_layers_tests.py
        python3 -B .github/scripts/runtime_image_metadata_tests.py
        python3 -B .github/scripts/runtime_image_private_keyrings_tests.py
        python3 -B .github/scripts/runtime_image_machine_id_tests.py
        python3 .github/scripts/omarchy-close-browser-tests.py
        python3 .github/scripts/omarchy-browser-input-tests.py
        node --test .github/scripts/omarchy-browser-page.test.cjs
      SH
      { "uses" => "docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e",
        "with" => { "cache-binary" => "${{ #{DEVELOP_PUSH} }}" } },
      { "name" => "Set immutable Omarchy image version", "id" => "identity",
        "run" => 'echo "version=git-${GITHUB_SHA:0:12}" >> "$GITHUB_OUTPUT"' },
      { "name" => "Identify pinned Omarchy dependency cache", "id" => "cache",
        "run" => 'python3 .github/scripts/omarchy-cache-key.py "$RUNNER_TEMP/omarchy-evidence/cache-inputs.json" >> "$GITHUB_OUTPUT"' },
      { "name" => "Restore complete Omarchy layer cache", "id" => "omarchy-cache-restore",
        "uses" => "actions/cache/restore@caa296126883cff596d87d8935842f9db880ef25", "timeout-minutes" => 5,
        "continue-on-error" => true, "with" => { "path" => CACHE_PATH, "key" => CACHE_KEY, "fail-on-cache-miss" => false } },
      { "name" => "Discard incomplete Omarchy cache restore", "if" => CACHE_MISS,
        "run" => 'rm -rf "$RUNNER_TEMP/omarchy-layer-cache"' },
      { "name" => "Resolve OS security refresh key", "id" => "os-refresh",
        "run" => 'python3 .github/scripts/os_refresh_key.py runtime-omarchy api/crates/layer4-runtime/Dockerfile "${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}" >> "$GITHUB_OUTPUT"' },
      { "name" => "Build current-source Omarchy image", "id" => "build",
        "uses" => "docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8",
        "with" => { "context" => ".", "file" => "api/crates/layer4-runtime/Dockerfile", "target" => "omarchy",
          "platforms" => "linux/amd64", "push" => false, "load" => true, "outputs" => "type=docker,rewrite-timestamp=true", "tags" => "cma-runtime-omarchy:ci",
          "cache-to" => "${{ #{CACHE_WRITE} && format('type=local,mode=min,dest={0}/omarchy-layer-cache', runner.temp) || '' }}",
          "build-args" => <<~'ARGS', "cache-from" => <<~'CACHE' } },
            OMARCHY_CMA_BASE=runtime-tools
            RUNTIME_DESKTOP=1
            CHRONO_BUILD=${{ steps.identity.outputs.version }}
            CHRONO_BUILD_SHA=${{ github.sha }}
            OS_SECURITY_REFRESH=${{ steps.os-refresh.outputs.value }}
            SOURCE_DATE_EPOCH=0
          ARGS
            ${{ steps.omarchy-cache-restore.outcome == 'success' && steps.omarchy-cache-restore.outputs.cache-hit == 'true' && format('type=local,src={0}/omarchy-layer-cache', runner.temp) || '' }}
          CACHE
      { "name" => "Release Omarchy build cache before image checks", "run" => <<~'SH' },
        image_id=$(docker image inspect cma-runtime-omarchy:ci --format '{{.Id}}')
        docker buildx prune --all --force
        test "$(docker image inspect cma-runtime-omarchy:ci --format '{{.Id}}')" = "$image_id"
        df -h /
      SH
      { "name" => "Verify Omarchy runtime contract",
        "run" => 'node .github/scripts/omarchy-contract.mjs "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/runtime-contract.json"' },
      { "name" => "Verify Omarchy staging qualification profile",
        "run" => 'CMA_RUNTIME_EVIDENCE_PROFILE=omarchy node .github/scripts/runtime-image-evidence.mjs run cma-runtime-omarchy:ci "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/staging-qualification.json"' },
      { "name" => "Qualify exact Omarchy ledger storage",
        "run" => 'node .github/scripts/runtime-ledger-evidence.mjs cma-runtime-omarchy:ci "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/ledger-storage.json"' },
      { "name" => "Prepare native Chromium namespaces", "run" => "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0" },
      { "name" => "Verify device-free Omarchy desktop",
        "run" => 'node .github/scripts/omarchy-runtime-evidence.mjs "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/device-free"' },
      { "name" => "Verify Omarchy virtual graphics desktop", "env" => { "CMA_OMARCHY_TEST_DRM" => "1" }, "run" => <<~'SH' },
        sudo modprobe vkms
        node .github/scripts/omarchy-runtime-evidence.mjs "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/virtual-drm"
      SH
      { "name" => "Verify Omarchy production temporary directory", "env" => { "CMA_OMARCHY_TEST_IMAGE_TMP" => "1" },
        "run" => 'node .github/scripts/omarchy-runtime-evidence.mjs "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/image-tmp"' },
      { "name" => "Restore Trivy databases", "if" => "github.event_name != 'push' || github.ref != 'refs/heads/develop'",
        "uses" => "actions/cache/restore@caa296126883cff596d87d8935842f9db880ef25",
        "with" => { "path" => "${{ github.workspace }}/.cache/trivy", "key" => "cache-trivy-readonly", "restore-keys" => "cache-trivy-" } },
      { "name" => "Scan tested Omarchy image", "uses" => "aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25",
        "with" => { "cache" => "${{ #{DEVELOP_PUSH} }}", "image-ref" => "cma-runtime-omarchy:ci", "format" => "json",
          "output" => "${{ runner.temp }}/omarchy-evidence/vulnerabilities.json", "severity" => "HIGH,CRITICAL",
          "exit-code" => "0", "ignore-unfixed" => true } },
      { "name" => "Omarchy vulnerability gate", "timeout-minutes" => 5,
        "env" => { "BASE_SHA" => "${{ github.event_name == 'pull_request' && github.event.pull_request.base.sha || github.event_name == 'merge_group' && github.event.merge_group.base_sha || github.event.before }}",
          "GH_TOKEN" => "${{ github.token }}" },
        "run" => 'python3 -B .github/scripts/image_vulnerability_gate.py --component runtime-omarchy --report "$RUNNER_TEMP/omarchy-evidence/vulnerabilities.json" --base "$BASE_SHA"' },
      { "name" => "Retain Omarchy image evidence", "if" => "always() && steps.build.outcome == 'success'",
        "uses" => "actions/upload-artifact@v4", "with" => {
          "name" => "omarchy-image-evidence-${{ github.run_id }}-${{ github.run_attempt }}",
          "path" => "${{ runner.temp }}/omarchy-evidence/", "if-no-files-found" => "error", "retention-days" => 90 } },
      { "name" => "Export tested Omarchy image", "if" => EXPORT_IF, "timeout-minutes" => 10,
        "run" => 'python3 .github/scripts/export-ci-native-image.py runtime-omarchy "$GITHUB_SHA" "$RUNNER_TEMP/ci-native-image"' },
      { "name" => "Install pinned image secret scanner", "if" => SCAN_IF, "timeout-minutes" => 5,
        "env" => { "CD_COMPONENT" => "runtime-omarchy" },
        "run" => "bash .github/scripts/staging_cd_transport.sh\nbash .github/scripts/install-image-gitleaks.sh\n" },
      { "name" => "Scan tested Omarchy image as staging publication does", "if" => SCAN_IF, "timeout-minutes" => 30,
        "run" => 'python3 -B .github/scripts/ci_image_secret_scan.py runtime-omarchy "$RUNNER_TEMP/ci-native-image" "$GITHUB_SHA"' },
      { "name" => "Retain tested Omarchy image", "if" => EXPORT_IF, "timeout-minutes" => 10,
        "uses" => "actions/upload-artifact@v4", "with" => {
          "name" => "ci-native-image-runtime-omarchy-${{ github.sha }}-${{ github.run_id }}-${{ github.run_attempt }}",
          "path" => "${{ runner.temp }}/ci-native-image/", "if-no-files-found" => "error",
          "compression-level" => 0, "retention-days" => 1 } },
      { "name" => "Save qualified Omarchy layer cache", "if" => "success() && #{CACHE_WRITE}",
        "uses" => "actions/cache/save@caa296126883cff596d87d8935842f9db880ef25", "timeout-minutes" => 5,
        "continue-on-error" => true, "with" => { "path" => CACHE_PATH, "key" => CACHE_KEY } },
    ]
    contract.require!(row["steps"] == expected,
      "Omarchy build, native gates, evidence and success-only export must remain exact and ordered")
  end
end

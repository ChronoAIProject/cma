# frozen_string_literal: true

# Keep Omarchy independent of the Backend graph, including its archive split.
module OmarchyCIContract
  EXPORT_IF = "success() && (github.event_name == 'push' || github.event.pull_request.head.repo.full_name == github.repository)"

  def self.validate(ci)
    contract = CIContract
    triggers = ci["on"] || ci[true]
    contract.require!(triggers == {
      "pull_request" => { "branches" => %w[develop main] },
      "push" => { "branches" => ["develop"] },
    }, "Omarchy must run on every develop push and ordinary PR")
    row = contract.job(ci, "omarchy", "Images (runtime-omarchy)")
    contract.require!(row.keys.sort == %w[name permissions runs-on steps timeout-minutes].sort,
      "Omarchy cannot acquire conditional, dependency, matrix or deployment authority")
    contract.require!(row["runs-on"] == "ubuntu-24.04" && row["timeout-minutes"] == 90 &&
      row["permissions"] == { "contents" => "read" }, "Omarchy requires bounded native amd64 read-only CI")
    expected = [
      { "uses" => "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09" },
      { "name" => "Omarchy desktop fixture regressions",
        "run" => "python3 .github/scripts/omarchy-close-browser-tests.py" },
      { "name" => "Reserve space on the disposable Omarchy runner", "run" => <<~'SH' },
        test "$GITHUB_ACTIONS" = true
        sudo rm -rf /usr/local/lib/android /usr/share/dotnet /opt/ghc
        df -h /
      SH
      { "uses" => "actions/setup-node@v6", "with" => { "node-version" => 26 } },
      { "uses" => "docker/setup-buildx-action@8d2750c68a42422c14e847fe6c8ac0403b4cbd6f" },
      { "name" => "Set immutable Omarchy image version", "id" => "identity",
        "run" => 'echo "version=git-${GITHUB_SHA:0:12}" >> "$GITHUB_OUTPUT"' },
      { "name" => "Build current-source Omarchy image", "id" => "build",
        "uses" => "docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8",
        "with" => { "context" => ".", "file" => "infra/runtime/Dockerfile", "target" => "omarchy",
          "platforms" => "linux/amd64", "push" => false, "load" => true, "tags" => "cma-runtime-omarchy:ci",
          "build-args" => <<~'ARGS', "cache-from" => "type=gha,scope=runtime-omarchy", "cache-to" => "type=gha,mode=max,scope=runtime-omarchy" } },
            OMARCHY_CMA_BASE=runtime
            RUNTIME_DESKTOP=1
            CHRONO_BUILD=${{ steps.identity.outputs.version }}
            CHRONO_BUILD_SHA=${{ github.sha }}
            OS_SECURITY_REFRESH=${{ github.run_id }}-${{ github.run_attempt }}
          ARGS
      { "name" => "Release Omarchy build cache before image checks", "run" => <<~'SH' },
        image_id=$(docker image inspect cma-runtime-omarchy:ci --format '{{.Id}}')
        docker buildx prune --all --force
        test "$(docker image inspect cma-runtime-omarchy:ci --format '{{.Id}}')" = "$image_id"
        df -h /
      SH
      { "name" => "Verify Omarchy runtime contract",
        "run" => 'node .github/scripts/omarchy-contract.mjs "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/runtime-contract.json"' },
      { "name" => "Prepare native Chromium namespaces", "run" => "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0" },
      { "name" => "Verify device-free Omarchy desktop",
        "run" => 'node .github/scripts/omarchy-runtime-evidence.mjs "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/device-free"' },
      { "name" => "Verify Omarchy virtual graphics desktop", "env" => { "CMA_OMARCHY_TEST_DRM" => "1" }, "run" => <<~'SH' },
        sudo modprobe vkms
        node .github/scripts/omarchy-runtime-evidence.mjs "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/virtual-drm"
      SH
      { "name" => "Verify Omarchy production temporary directory", "env" => { "CMA_OMARCHY_TEST_IMAGE_TMP" => "1" },
        "run" => 'node .github/scripts/omarchy-runtime-evidence.mjs "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-evidence/image-tmp"' },
      { "name" => "Scan tested Omarchy image", "uses" => "aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25",
        "with" => { "image-ref" => "cma-runtime-omarchy:ci", "format" => "json",
          "output" => "${{ runner.temp }}/omarchy-evidence/vulnerabilities.json", "severity" => "HIGH,CRITICAL",
          "exit-code" => "1", "ignore-unfixed" => true } },
      { "name" => "Retain Omarchy image evidence", "if" => "always() && steps.build.outcome == 'success'",
        "uses" => "actions/upload-artifact@v4", "with" => {
          "name" => "omarchy-image-evidence-${{ github.run_id }}-${{ github.run_attempt }}",
          "path" => "${{ runner.temp }}/omarchy-evidence/", "if-no-files-found" => "error", "retention-days" => 90 } },
      { "name" => "Export tested Omarchy image", "if" => EXPORT_IF, "timeout-minutes" => 10,
        "run" => 'python3 .github/scripts/export-ci-native-image.py runtime-omarchy "$GITHUB_SHA" "$RUNNER_TEMP/ci-native-image"' },
      { "name" => "Retain tested Omarchy image", "if" => EXPORT_IF, "timeout-minutes" => 10,
        "uses" => "actions/upload-artifact@v4", "with" => {
          "name" => "ci-native-image-runtime-omarchy-${{ github.sha }}-${{ github.run_id }}-${{ github.run_attempt }}",
          "path" => "${{ runner.temp }}/ci-native-image/", "if-no-files-found" => "error",
          "compression-level" => 0, "retention-days" => 1 } },
    ]
    contract.require!(row["steps"] == expected,
      "Omarchy build, native gates, evidence and success-only export must remain exact and ordered")
  end
end

# frozen_string_literal: true

module StagingDesktopFixtureContract
  BRANCH = "github.repository == 'ChronoAIProject/cma' && github.ref == 'refs/heads/develop'"

  def self.check(workflow, guard)
    events = workflow["on"] || workflow[true]
    inputs = events.dig("workflow_dispatch", "inputs")
    guard.call(events.is_a?(Hash) && events.keys == ["workflow_dispatch"] &&
      inputs.is_a?(Hash) && inputs.keys.sort == %w[source variant] &&
      inputs.dig("source", "required") == true && inputs.dig("source", "type") == "string" &&
      inputs.dig("variant", "required") == true && inputs.dig("variant", "type") == "choice" &&
      inputs.dig("variant", "options") == %w[debian omarchy] &&
      workflow["permissions"] == {} && workflow.fetch("jobs").keys == ["publish"] &&
      workflow["concurrency"] == { "group" => "staging-desktop-fixture-publication", "cancel-in-progress" => false },
      "Desktop fixture publication must be a serialized manual staging operation")
    job = workflow.fetch("jobs").fetch("publish")
    guard.call(job.keys.sort == %w[environment if permissions runs-on steps timeout-minutes] &&
      job["if"] == BRANCH && job["environment"] == "chrono-ai-staging" && job["runs-on"] == "ubuntu-24.04" &&
      job["timeout-minutes"] == 240 && job["permissions"] == { "contents" => "read", "packages" => "write" },
      "Desktop fixture writes require the protected staging environment and bounded native publisher")
    guard.call(job.fetch("steps") == reviewed_steps, "Desktop fixture package write steps changed")
  end

  def self.reviewed_steps
    [
      { "uses" => "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09",
        "with" => { "fetch-depth" => 0, "persist-credentials" => false } },
      { "uses" => "actions/setup-node@249970729cb0ef3589644e2896645e5dc5ba9c38",
        "with" => { "node-version" => 26, "package-manager-cache" => false } },
      { "uses" => "docker/setup-buildx-action@37fe631027851001ddb9b187196cc803df7f5f0e" },
      { "name" => "Prepare native image qualification tools", "env" => { "CD_COMPONENT" => "runtime" },
        "run" => "bash .github/scripts/staging_cd_transport.sh\n" \
                 "bash .github/scripts/install-image-gitleaks.sh\n" \
                 "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0\n" },
      { "name" => "Qualify and publish the same OCI archive",
        "env" => { "DESKTOP_VARIANT" => "${{ inputs.variant }}", "DESKTOP_SOURCE" => "${{ inputs.source }}",
                   "REGISTRY_TOKEN" => "${{ github.token }}" },
        "run" => "python3 -B .github/scripts/staging_cd_desktop_fixture.py \\\n" \
                 "  --variant \"$DESKTOP_VARIANT\" --source \"$DESKTOP_SOURCE\" \\\n" \
                 "  --receipt \"$RUNNER_TEMP/desktop-publication.json\"\n" },
      { "name" => "Retain the immutable publication receipt",
        "uses" => "actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9",
        "with" => { "name" => "staging-desktop-publication", "path" => "${{ runner.temp }}/desktop-publication.json",
                    "if-no-files-found" => "error" } },
    ]
  end
end

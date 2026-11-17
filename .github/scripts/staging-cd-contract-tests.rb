# frozen_string_literal: true

module StagingCDContractTests
  def self.run(reject)
    {
      "environment" => "production", "if" => "true", "continue-on-error" => true,
      "permissions" => { "contents" => "write", "packages" => "write" },
      "runs-on" => "self-hosted", "timeout-minutes" => 360,
    }.each do |field, value|
      reject.call("staging #{field} boundary") { |candidate| candidate["staging-cd"]["jobs"]["deploy"][field] = value }
    end
    reject.call("staging PR trigger") do |candidate|
      workflow = candidate["staging-cd"]
      (workflow["on"] || workflow[true])["pull_request"] = {}
    end
    reject.call("staging CI branch") do |candidate|
      workflow = candidate["staging-cd"]
      (workflow["on"] || workflow[true])["workflow_run"]["branches"] = ["main"]
    end
    reject.call("staging cancellation") { |candidate| candidate["staging-cd"]["concurrency"]["cancel-in-progress"] = true }
    reject.call("staging checkout artifact source") do |candidate|
      candidate["staging-cd"]["jobs"]["deploy"]["steps"][0]["with"]["ref"] = "${{ inputs.sha }}"
    end
    reject.call("staging manual mutation default") do |candidate|
      workflow = candidate["staging-cd"]
      (workflow["on"] || workflow[true])["workflow_dispatch"]["inputs"]["dry_run"]["default"] = false
    end
    reject.call("staging arbitrary execution") { |candidate| candidate["staging-cd"]["jobs"]["deploy"]["steps"] << { "run" => "true" } }
    reject.call("staging omit regressions in CI") do |candidate|
      candidate["ci"]["jobs"]["trigger-release-contract"]["steps"].delete_if { |step| step["name"] == "Staging CD promotion regressions" }
    end
    reject.call("staging force mutation") do |candidate|
      candidate["staging-cd"]["jobs"]["deploy"]["steps"][2]["env"]["DRY_RUN"] = "false"
    end
    reject.call("extra package writer") do |candidate|
      candidate["staging-cd"]["jobs"]["extra"] = { "permissions" => { "packages" => "write" }, "steps" => [] }
    end
  end
end

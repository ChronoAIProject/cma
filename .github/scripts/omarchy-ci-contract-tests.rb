# frozen_string_literal: true

module OmarchyCIContractTests
  def self.run(reject)
    reject.call("remove Omarchy gate") { |value| value["ci"]["jobs"].delete("omarchy") }
    {
      "if" => "false", "continue-on-error" => true, "needs" => "backend",
      "permissions" => { "contents" => "read", "packages" => "write" },
      "runs-on" => "ubuntu-24.04-arm", "timeout-minutes" => 360,
      "environment" => "production", "env" => { "CMA_RUNTIME_TOKEN" => "fixture" },
    }.each do |key, changed|
      reject.call("Omarchy job #{key}") { |value| value["ci"]["jobs"]["omarchy"][key] = changed }
    end
    reject.call("Omarchy missing timeout") { |value| value["ci"]["jobs"]["omarchy"].delete("timeout-minutes") }
    reject.call("Omarchy insufficient build and qualification budget") do |value|
      value["ci"]["jobs"]["omarchy"]["timeout-minutes"] = 90
    end
    reject.call("Omarchy no develop push") do |value|
      workflow = value["ci"]
      (workflow["on"] || workflow[true])["push"]["branches"] = ["prototype"]
    end
    names = CIContract.load_inputs["ci"]["jobs"]["omarchy"]["steps"].map { |step| step["name"] }.compact
    names.each do |name|
      %w[removed skipped optional fake-success].each do |mutation|
        reject.call("Omarchy #{name}: #{mutation}") do |value|
          steps = value["ci"]["jobs"]["omarchy"]["steps"]
          entry = steps.find { |step| step["name"] == name }
          case mutation
          when "removed" then steps.delete(entry)
          when "skipped" then entry["if"] = "false"
          when "optional" then entry["continue-on-error"] = true
          when "fake-success" then entry["run"] = "true"
          end
        end
      end
    end
    ["python3 .github/scripts/omarchy-close-browser-tests.py",
     "python3 .github/scripts/omarchy-browser-input-tests.py",
     "node --test .github/scripts/omarchy-browser-page.test.cjs"].each do |command|
      reject.call("Omarchy omits regression #{command}") do |value|
        step = value["ci"]["jobs"]["omarchy"]["steps"].find { |entry| entry["name"] == "Omarchy desktop fixture regressions" }
        step["run"] = step["run"].sub("#{command}\n", "")
      end
    end
    {
      "target" => "default-runtime", "platforms" => "linux/arm64", "push" => true,
      "context" => "https://example.invalid/old-source.git", "load" => false,
      "build-args" => "OMARCHY_CMA_BASE=old-donor\nRUNTIME_DESKTOP=0\n",
    }.each do |key, changed|
      reject.call("Omarchy build #{key}") do |value|
        value["ci"]["jobs"]["omarchy"]["steps"].find { |step| step["id"] == "build" }["with"][key] = changed
      end
    end
    reject.call("Omarchy exports before scan") do |value|
      steps = value["ci"]["jobs"]["omarchy"]["steps"]
      export = steps.find { |step| step["name"] == "Export tested Omarchy image" }
      steps.delete(export)
      steps.insert(steps.index { |step| step["name"] == "Scan tested Omarchy image" }, export)
    end
    reject.call("Omarchy failed image exports") do |value|
      value["ci"]["jobs"]["omarchy"]["steps"].find { |step| step["name"] == "Export tested Omarchy image" }["if"] = "always()"
    end
    reject.call("Omarchy unreviewed extra command") do |value|
      value["ci"]["jobs"]["omarchy"]["steps"] << { "run" => "docker push cma-runtime-omarchy:ci" }
    end
    unless system("python3", File.join(__dir__, "omarchy-download-tests.py"))
      raise "Omarchy pinned download failure contracts failed"
    end
  end
end

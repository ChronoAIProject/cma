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
      %w[removed skipped changed-failure-policy fake-success].each do |mutation|
        reject.call("Omarchy #{name}: #{mutation}") do |value|
          steps = value["ci"]["jobs"]["omarchy"]["steps"]
          entry = steps.find { |step| step["name"] == name }
          case mutation
          when "removed" then steps.delete(entry)
          when "skipped" then entry["if"] = "false"
          when "changed-failure-policy" then entry["continue-on-error"] = entry["continue-on-error"] != true
          when "fake-success" then entry["run"] = "true"
          end
        end
      end
    end
    ["python3 -B .github/scripts/omarchy-cache-key-tests.py",
     "python3 .github/scripts/omarchy-close-browser-tests.py",
     "python3 .github/scripts/omarchy-browser-input-tests.py",
     "node --test .github/scripts/omarchy-browser-page.test.cjs"].each do |command|
      reject.call("Omarchy omits regression #{command}") do |value|
        step = value["ci"]["jobs"]["omarchy"]["steps"].find { |entry| entry["name"] == "Omarchy desktop fixture regressions" }
        step["run"] = step["run"].sub("#{command}\n", "")
      end
    end
    {
      "cache-to" => "type=gha,mode=max,scope=runtime-omarchy",
      "cache-from" => "type=gha,scope=other",
      "target" => "default-runtime", "platforms" => "linux/arm64", "push" => true,
      "context" => "https://example.invalid/old-source.git", "load" => false,
      "build-args" => "OMARCHY_CMA_BASE=old-donor\nRUNTIME_DESKTOP=0\n",
    }.each do |key, changed|
      reject.call("Omarchy build #{key}") do |value|
        value["ci"]["jobs"]["omarchy"]["steps"].find { |step| step["id"] == "build" }["with"][key] = changed
      end
    end
    [nil, "", "type=gha,mode=min,scope=${{ steps.cache.outputs.scope }}",
     "type=local,mode=max,dest=${{ runner.temp }}/omarchy-layer-cache"].each do |changed|
      reject.call("Omarchy omits archive retention or changes cache storage") do |value|
        row = value["ci"]["jobs"]["omarchy"]["steps"].find { |entry| entry["id"] == "build" }
        row["with"]["cache-to"] = changed
      end
    end
    ["Restore complete Omarchy layer cache", "Save qualified Omarchy layer cache"].each do |name|
      ["path", "key"].each do |key|
        reject.call("Omarchy archive #{name} changes #{key}") do |value|
          entry = value["ci"]["jobs"]["omarchy"]["steps"].find { |step| step["name"] == name }
          entry["with"][key] = "unqualified-cache"
        end
      end
    end
    reject.call("Omarchy archive restores a different dependency identity") do |value|
      entry = value["ci"]["jobs"]["omarchy"]["steps"].find { |step| step["id"] == "omarchy-cache-restore" }
      entry["with"]["restore-keys"] = "omarchy-layer-archive-v1-"
    end
    reject.call("Omarchy cache becomes visible before qualification") do |value|
      steps = value["ci"]["jobs"]["omarchy"]["steps"]
      save = steps.find { |step| step["name"] == "Save qualified Omarchy layer cache" }
      steps.delete(save)
      steps.insert(steps.index { |step| step["name"] == "Scan tested Omarchy image" }, save)
    end
    reject.call("Omarchy dependency export requires a second solve") do |value|
      steps = value["ci"]["jobs"]["omarchy"]["steps"]
      build = steps.find { |entry| entry["id"] == "build" }
      export = { "uses" => build["uses"], "with" => {
        "context" => ".", "file" => "infra/runtime/Dockerfile", "target" => "omarchy-dependencies",
        "platforms" => "linux/amd64", "push" => false, "load" => false,
        "cache-to" => build["with"].delete("cache-to"),
      } }
      steps.insert(steps.index(build) + 1, export)
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

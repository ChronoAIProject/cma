# frozen_string_literal: true

require_relative "ci-contract"
require_relative "gateway-source-contract"

mode = ARGV.fetch(0)
raise "expected release or provider" unless %w[release provider].include?(mode)
base = CIContract.load_inputs
CIContract.public_send(mode, base)
tests = 0
reject = lambda do |label, &change|
  candidate = Marshal.load(Marshal.dump(base))
  change.call(candidate)
  begin
    CIContract.public_send(mode, candidate)
  rescue StandardError
    tests += 1
    next
  end
  raise "accepted unsafe workflow mutation: #{label}"
end

job_ids = mode == "release" ? { "ci" => %w[trigger-release-contract images], "publish" => %w[runtime-image-gate release] } : { "ci" => %w[trigger-provider-smoke-contract], "nightly-staging" => %w[journeys] }
job_ids.each do |workflow, ids|
  ids.each do |id|
    reject.call("remove #{id}") { |candidate| candidate[workflow]["jobs"].delete(id) }
    reject.call("optional #{id}") { |candidate| candidate[workflow]["jobs"][id]["continue-on-error"] = true }
    reject.call("skip #{id}") { |candidate| candidate[workflow]["jobs"][id]["if"] = "false" }
  end
end

guarded = mode == "release" ? [
  ["ci", "trigger-release-contract", "Debian desktop fixture regressions"],
  ["ci", "images", "Execute desktop image contract"],
  ["ci", "images", "Execute built runtime image contract"],
  ["ci", "images", "Require passing runtime image evidence"],
  ["ci", "images", "Export tested native image"],
  ["publish", "runtime-image-gate", "Verify exact runtime artifact bytes and source"],
  ["publish", "runtime-image-gate", "Pull verified immutable runtime"],
  ["publish", "runtime-image-gate", "Execute published runtime image contract"],
] : [
  ["ci", "trigger-provider-smoke-contract", "Staging authority and evidence regressions"],
  ["ci", "trigger-provider-smoke-contract", "Backend capacity preparation regressions"],
  ["ci", "trigger-provider-smoke-contract", "Typecheck every harness entry point"],
  ["ci", "trigger-provider-smoke-contract", "Local browser guard mechanics"],
  ["ci", "trigger-provider-smoke-contract", "Backend resource sampling regressions"],
  ["ci", "backend", "Image normalization budgets"],
  ["ci", "backend", "Prepare Backend capacity"],
  ["ci", "backend", "Tests"],
  ["ci", "backend", "Gateway socket latency budget"],
  ["ci", "backend", "Disposable real Mongo persistence contracts"],
  ["ci", "backend", "Image Manager authenticated Mongo isolation"],
  ["nightly-staging", "smoke", "Initialize isolated evidence path"],
  ["nightly-staging", "journeys", "Initialize isolated evidence path"],
  ["nightly-staging", "journeys", "Verify approval policy and independently observed deployment before fixture access"],
  ["nightly-staging", "journeys", "Require complete cleanup and independently bound campaign evidence"],
]
guarded.each do |workflow, id, name|
  %w[removed skipped optional fake-success].each do |mutation|
    reject.call("#{name}: #{mutation}") do |candidate|
      steps = candidate[workflow]["jobs"][id]["steps"]
      entry = steps.find { |row| row["name"] == name }
      case mutation
      when "removed" then steps.delete(entry)
      when "skipped" then entry["if"] = "false"
      when "optional" then entry["continue-on-error"] = true
      when "fake-success" then entry["run"] = "true"
      end
    end
  end
end

if mode == "provider"
  require_relative "omarchy-ci-contract-tests"
  OmarchyCIContractTests.run(reject)
  [nil, 10, 30].each do |timeout|
    reject.call("Image Manager loses its measured finite margin: #{timeout.inspect}") do |value|
      row = value["ci"]["jobs"]["backend"]["steps"].find { |step| step["name"] == "Image Manager authenticated Mongo isolation" }
      timeout.nil? ? row.delete("timeout-minutes") : row["timeout-minutes"] = timeout
    end
  end
  reject.call("capacity runs after compilation") do |value|
    rows = value["ci"]["jobs"]["backend"]["steps"]
    rows << rows.delete_at(1)
  end
  reject.call("capacity uses a different runner size") do |value|
    value["ci"]["jobs"]["backend"]["runs-on"] = "ubuntu-latest-4-cores"
  end
  reject.call("capacity uses a caller-controlled environment") do |value|
    value["ci"]["jobs"]["backend"]["steps"][1]["env"] = { "RUNNER_ENVIRONMENT" => "github-hosted" }
  end
  reject.call("capacity helper has the wrong working directory") do |value|
    value["ci"]["jobs"]["backend"]["steps"][1]["working-directory"] = "api"
  end
end

if mode == "release"
  cache_changes = {
    "fork cache access" => ["Prepare API Cargo cache identity", "if", "matrix.component == 'api'"],
    "unbounded cache transfer" => ["Inject API Cargo cache mounts", "timeout-minutes", 60],
    "mutable cache action" => ["Restore API Cargo cache archive", "uses", "actions/cache/restore@v4"],
    "cache other paths" => ["Restore API Cargo cache archive", "with", { "path" => "~/.cargo" }],
    "save failed image cache" => ["Save accepted API Cargo cache archive", "if", "always()"],
    "skip build on cache hit" => ["Build image for all target platforms", "if", "steps.api-cargo-cache-restore.outputs.cache-hit != 'true'"],
    "skip scan on cache hit" => ["Vulnerability scan", "if", "steps.api-cargo-cache-restore.outputs.cache-hit != 'true'"],
    "omit prune" => ["Release build cache before image checks", "run", "true"],
  }
  cache_changes.each do |label, (name, field, changed)|
    reject.call(label) do |value|
      value["ci"]["jobs"]["images"]["steps"].find { |row| row["name"] == name }[field] = changed
    end
  end
  reject.call("extract after prune") do |value|
    rows = value["ci"]["jobs"]["images"]["steps"]
    extraction = rows.find { |row| row["name"] == "Extract API Cargo cache before pruning" }
    rows.delete(extraction)
    rows << extraction
  end
  reject.call("cache fallback crosses compatibility boundary") do |value|
    value["ci"]["jobs"]["images"]["steps"].find { |row| row["name"] == "Restore API Cargo cache archive" }["with"]["restore-keys"] = "cma-api-"
  end
  reject.call("cache regressions no longer execute") do |value|
    value["ci"]["jobs"]["trigger-release-contract"]["steps"].delete_if { |row| row["name"] == "API Cargo cache bridge regressions" }
  end
  %w[api web runtime cmaeg image-manager].each do |component|
    reject.call("CI omits #{component} image") do |value|
      value["ci"]["jobs"]["images"]["strategy"]["matrix"]["component"].delete(component)
    end
    %w[images image-exports].each do |id|
      reject.call("release #{id} omits #{component}") do |value|
        value["publish"]["jobs"][id]["strategy"]["matrix"]["component"].delete(component)
      end
    end
  end
  reject.call("release omits runtime dependency") { |value| value["publish"]["jobs"]["release"]["needs"].delete("runtime-image-gate") }
  reject.call("mutable runtime reference") do |value|
    value["publish"]["jobs"]["runtime-image-gate"]["steps"].find { |row| row["name"] == "Pull verified immutable runtime" }["env"]["RUNTIME_REFERENCE"] = "ghcr.io/chronoaiproject/cma-runtime:latest"
  end
  reject.call("evidence upload can be empty") do |value|
    value["ci"]["jobs"]["images"]["steps"].find { |row| row["name"] == "Retain runtime image evidence" }["with"]["if-no-files-found"] = "warn"
  end
  reject.call("native artifacts uploaded after failed checks") do |value|
    value["ci"]["jobs"]["images"]["steps"].find { |row| row["name"] == "Retain tested native image" }["if"] = "always()"
  end
  reject.call("native export precedes scan") do |value|
    steps = value["ci"]["jobs"]["images"]["steps"]
    export = steps.find { |row| row["name"] == "Export tested native image" }
    steps.delete(export)
    steps.unshift(export)
  end
  reject.call("native artifacts retain too long") do |value|
    value["ci"]["jobs"]["images"]["steps"].find { |row| row["name"] == "Retain tested native image" }["with"]["retention-days"] = 90
  end
  reject.call("native artifacts omit source identity") do |value|
    value["ci"]["jobs"]["images"]["steps"].find { |row| row["name"] == "Retain tested native image" }["with"]["name"] = "native-image"
  end
  reject.call("publisher mutates live Gateway") { |value| value["publish"]["jobs"]["release"]["steps"] << { "run" => "kubectl apply -f infra" } }
  GatewaySourceContract.expected.each do |path, documents|
    candidate = GatewaySourceContract.expected
    candidate[path] = documents + [documents.first]
    begin
      GatewaySourceContract.validate(candidate)
    rescue StandardError
      tests += 1
      next
    end
    raise "Gateway contract accepted duplicate #{path}"
  end
  %w[api web].product([nil, 30]).each do |service, timeout|
    candidate = GatewaySourceContract.expected
    defaults = candidate["infra/#{service}/gcpbackendpolicy.yaml"][0]["spec"]["default"]
    if timeout.nil?
      defaults.delete("connectionDraining")
    else
      defaults["connectionDraining"]["drainingTimeoutSec"] = timeout
    end
    begin
      GatewaySourceContract.validate(candidate)
    rescue StandardError
      tests += 1
      next
    end
    raise "Gateway contract accepted absent or shortened #{service} connection drain"
  end
  %w[/livez /].each do |health|
    candidate = GatewaySourceContract.expected
    candidate["infra/api/healthcheckpolicy.yaml"][0]["spec"]["default"]["config"]["httpHealthCheck"]["requestPath"] = health
    begin
      GatewaySourceContract.validate(candidate)
    rescue StandardError
      tests += 1
      next
    end
    raise "Gateway contract accepted non-readiness health"
  end
  %w[direct-gateway stripped-prefix].each do |mutation|
    candidate = GatewaySourceContract.expected
    route = candidate["infra/api/httproute.yaml"].find { |value| value.dig("metadata", "name") == "cma-web" }
    bridge = route["spec"]["rules"].find { |value| value["matches"].any? { |match| match.dig("path", "value") == "/api/event-gateway" } }
    if mutation == "direct-gateway"
      bridge["backendRefs"] = GatewaySourceContract.backend("cma-event-gateway")
    else
      bridge["filters"] = GatewaySourceContract.service_route("event-gateway", "cma-agent-manager", "/")["filters"]
    end
    begin
      GatewaySourceContract.validate(candidate)
    rescue StandardError
      tests += 1
      next
    end
    raise "Gateway contract accepted a bypass of the shared login bridge: #{mutation}"
  end
  %w[portal-capture wrong-protocol-target stripped-connect removed-legacy-host wrong-openapi-target unsupported-full-path-rewrite].each do |mutation|
    candidate = GatewaySourceContract.expected
    routes = candidate.fetch("infra/cmaeg/httproute.yaml")
    public_route = routes.find { |value| value.dig("metadata", "name") == "cma-event-gateway-public" }
    case mutation
    when "portal-capture"
      public_route["spec"]["rules"][0]["matches"][0]["path"]["value"] = "/event-gateway"
    when "wrong-protocol-target"
      public_route["spec"]["rules"][0]["backendRefs"] = GatewaySourceContract.backend("cma-agent-manager")
    when "stripped-connect"
      public_route["spec"]["rules"][-1]["filters"] = GatewaySourceContract.gateway_public_route("PathPrefix", "connect", "/connect")["filters"]
    when "removed-legacy-host"
      routes.delete_at(0)
    when "wrong-openapi-target"
      api_route = candidate.fetch("infra/api/httproute.yaml").find { |value| value.dig("metadata", "name") == "cma-web" }
      discovery = api_route["spec"]["rules"].find { |value| value["matches"].any? { |match| match.dig("path", "value") == "/agent-manager/openapi.json" } }
      discovery["backendRefs"] = GatewaySourceContract.backend("cma-web")
    when "unsupported-full-path-rewrite"
      # This formerly schema-valid rule is rejected by the GKE Gateway controller.
      discovery = public_route["spec"]["rules"].find { |value| value["matches"].any? { |match| match.dig("path", "value") == "/event-gateway/openapi.json" } }
      discovery["matches"] = [GatewaySourceContract.path("Exact", "/event-gateway/openapi.json")]
      discovery["filters"] = [{ "type" => "URLRewrite", "urlRewrite" => { "path" => { "type" => "ReplaceFullPath", "replaceFullPath" => "/openapi.json" } } }]
    end
    begin
      GatewaySourceContract.validate(candidate)
    rescue StandardError
      tests += 1
      next
    end
    raise "Gateway contract accepted incompatible service routing: #{mutation}"
  end
  {
    "infra/api/httproute.yaml" => %w[cma-web-nyxid cma-nyxid-http-redirect],
    "infra/cmaeg/httproute.yaml" => %w[cma-event-gateway-public-nyxid],
  }.each do |path, names|
    names.each do |name|
      %w[removed wrong-listener wildcard-host wrong-backend].each do |mutation|
        candidate = GatewaySourceContract.expected
        routes = candidate.fetch(path)
        route = routes.find { |value| value.dig("metadata", "name") == name }
        case mutation
        when "removed" then routes.delete(route)
        when "wrong-listener" then route["spec"]["parentRefs"] = GatewaySourceContract.parent("https")
        when "wildcard-host" then route["spec"]["hostnames"] = ["*.nyxid.ai"]
        when "wrong-backend" then route["spec"]["rules"][0]["backendRefs"] = GatewaySourceContract.backend("cma-web")
        end
        begin
          GatewaySourceContract.validate(candidate)
        rescue StandardError
          tests += 1
          next
        end
        raise "Gateway contract accepted unsafe Nyxid route: #{name}: #{mutation}"
      end
    end
  end
  config = GatewaySourceContract.documents(File.expand_path("../../infra/cmaeg/configmap.yaml", __dir__)).first
  GatewaySourceContract.validate_provider_origin(config)
  %w[http://example-host-03.example-namespace.svc https://example-host-01.example-namespace.svc http://example-host-01.example-namespace.svc:8080 missing duplicate].each do |origin|
    candidate = Marshal.load(Marshal.dump(config))
    providers = JSON.parse(candidate["data"]["CMAEG_PROVIDERS"])
    cma = providers.find { |provider| provider["slug"] == "cma_codex" }
    case origin
    when "missing" then providers.delete(cma)
    when "duplicate" then providers << cma.dup
    else cma["base_url"] = origin
    end
    candidate["data"]["CMAEG_PROVIDERS"] = JSON.generate(providers)
    begin
      GatewaySourceContract.validate_provider_origin(candidate)
    rescue StandardError
      tests += 1
      next
    end
    raise "Gateway contract accepted incompatible CMA provider origin: #{origin}"
  end
else
  reject.call("remove resource hook") { |v| v["ci"]["jobs"]["backend"]["env"].delete("BASH_ENV") }
  reject.call("wrap Backend shell") { |v| v["ci"]["jobs"]["backend"]["defaults"]["run"]["shell"] = "true {0}" }
  reject.call("override step shell") { |v| v["ci"]["jobs"]["backend"]["steps"][1]["shell"] = "true {0}" }
  reject.call("override step hook") { |v| v["ci"]["jobs"]["backend"]["steps"][1]["env"] = { "BASH_ENV" => "/dev/null" } }
  %w[smoke journeys].each do |id|
    reject.call("#{id}: runner context at job scope") do |value|
      value["nightly-staging"]["jobs"][id]["env"]["CMA_E2E_OUTPUT_DIR"] = "${{ runner.temp }}/evidence"
    end
    reject.call("#{id}: evidence initialized after harness") do |value|
      steps = value["nightly-staging"]["jobs"][id]["steps"]
      initialize = steps.find { |row| row["name"] == "Initialize isolated evidence path" }
      steps.delete(initialize)
      steps.push(initialize)
    end
  end
  reject.call("unprotected mutation") { |value| value["nightly-staging"]["jobs"]["journeys"].delete("environment") }
  reject.call("mutable checkout") do |value|
    value["nightly-staging"]["jobs"]["journeys"]["steps"].find { |row| row["uses"].to_s.start_with?("actions/checkout@") }["with"]["ref"] = "develop"
  end
  reject.call("cancel external writer") { |value| value["nightly-staging"]["concurrency"]["cancel-in-progress"] = true }
  reject.call("dependency execution before attestation") { |value| value["nightly-staging"]["jobs"]["journeys"]["steps"].unshift({ "run" => "npm ci" }) }
  reject.call("publish before cleanup audit") do |value|
    value["nightly-staging"]["jobs"]["journeys"]["steps"].last["if"] = "always()"
  end
end
puts "#{mode} workflow and source contracts rejected #{tests} unsafe mutations"

# frozen_string_literal: true

require "json"
require "yaml"

# These are deployment invariants, not authorization to apply the resources.
# Application membership/lease correctness and capacity are separate stage gates.
module CmaScaleManifests
  ROOT = File.expand_path("../..", __dir__)
  ROLES = {
    "api" => { name: "cma-agent-manager", container: "api", readiness: "/ready", grace: 760, max_replicas: 6 },
    "cmaeg" => { name: "cma-event-gateway", container: "cmaeg", readiness: "/readyz", grace: 760, max_replicas: 4 }
  }.freeze

  def self.documents(role)
    ROLES.fetch(role)
    %w[deployment hpa pdb].map { |kind| YAML.load_file(File.join(ROOT, "infra", role, "#{kind}.yaml")) }
  end

  def self.require_contract(condition, message)
    raise "CMA scale: #{message}" unless condition
  end

  def self.validate(role, deployment, hpa, pdb, down_period: 120)
    contract = ROLES.fetch(role)
    require_contract(down_period.is_a?(Integer) && down_period.between?(120, 1800), "supported downscale period")
    label = { "app.kubernetes.io/name" => contract.fetch(:name) }
    [deployment, hpa, pdb].zip(%w[Deployment HorizontalPodAutoscaler PodDisruptionBudget],
      %w[apps/v1 autoscaling/v2 policy/v1]).each do |object, kind, version|
      require_contract(object["kind"] == kind && object["apiVersion"] == version &&
        object.dig("metadata", "name") == contract[:name] &&
        object.dig("metadata", "namespace") == "chrono-bot", "resource identity/version")
    end
    spec = deployment.fetch("spec")
    pod = spec.fetch("template").fetch("spec")
    containers = pod.fetch("containers")
    require_contract(containers.length == 1 && containers[0]["name"] == contract[:container],
      "CPU HPA denominator must be reviewed if sidecars are introduced")
    container = containers[0]
    require_contract(spec["replicas"] == 2 && spec["minReadySeconds"].to_i >= 30, "ready replica floor")
    require_contract(spec.dig("selector", "matchLabels") == label &&
      label.all? { |key, value| spec.dig("template", "metadata", "labels", key) == value }, "pod selector")
    require_contract(spec.dig("strategy", "type") == "RollingUpdate" &&
      spec.dig("strategy", "rollingUpdate") == { "maxSurge" => 1, "maxUnavailable" => 0 }, "safe rollout")
    require_contract(pod["terminationGracePeriodSeconds"] == contract[:grace], "drain grace")
    readiness = container.fetch("readinessProbe")
    require_contract(readiness.dig("httpGet", "path") == contract[:readiness] &&
      readiness.dig("httpGet", "port") == "http" && readiness["failureThreshold"] == 1 &&
      readiness["periodSeconds"].to_i.between?(1, 5), "membership/drain readiness probe")
    hook = container.dig("lifecycle", "preStop")
    expected_hook = if role == "api"
      { "exec" => { "command" => ["/bin/sh", "-c",
        "/usr/local/bin/cma --agent-manager-drain; status=$?; /bin/sleep 30; exit $status"] } }
    else
      # Same-uid media jobs must not observe a credential-bearing exec process.
      { "sleep" => { "seconds" => 30 } }
    end
    require_contract(hook == expected_hook, "preStop must preserve the role's drain and credential isolation contract")
    resources = if role == "api"
      { "requests" => { "cpu" => "4", "memory" => "256Mi" }, "limits" => { "cpu" => "8", "memory" => "8Gi" } }
    else
      { "requests" => { "cpu" => "250m", "memory" => "2Gi" }, "limits" => { "cpu" => "2", "memory" => "2Gi" } }
    end
    require_contract(container["resources"] == resources, "resource/HPA/capacity budget must change together")
    spreads = pod.fetch("topologySpreadConstraints")
    { "kubernetes.io/hostname" => "DoNotSchedule", "topology.kubernetes.io/zone" => "ScheduleAnyway" }.each do |key, policy|
      matches = spreads.select { |spread| spread["topologyKey"] == key }
      require_contract(matches.length == 1 && matches[0]["maxSkew"] == 1 &&
        matches[0]["whenUnsatisfiable"] == policy && matches[0]["nodeTaintsPolicy"] == "Honor" &&
        matches[0]["matchLabelKeys"] == ["pod-template-hash"] &&
        matches[0]["labelSelector"] == { "matchLabels" => label }, "eligible-node/zone topology per rollout revision")
    end
    require_contract(pdb["spec"] == { "minAvailable" => 1, "unhealthyPodEvictionPolicy" => "AlwaysAllow",
      "selector" => { "matchLabels" => label } },
      "PDB must retain one available replica, including rollback to one")
    autoscale = hpa.fetch("spec")
    require_contract(autoscale["scaleTargetRef"] == { "apiVersion" => "apps/v1", "kind" => "Deployment",
      "name" => contract[:name] }, "HPA target")
    require_contract(autoscale["minReplicas"] == 2 && autoscale["maxReplicas"] == contract[:max_replicas], "HPA bounds")
    require_contract(autoscale["metrics"] == [{ "type" => "Resource", "resource" => {
      "name" => "cpu", "target" => { "type" => "Utilization", "averageUtilization" => (role == "api" ? 60 : 200) }
    } }], "CPU target/request budget")
    expected_up = { "stabilizationWindowSeconds" => 0, "selectPolicy" => "Max", "policies" => [
      { "type" => "Pods", "value" => 2, "periodSeconds" => 60 },
      { "type" => "Percent", "value" => 100, "periodSeconds" => 60 }
    ] }
    expected_down = { "stabilizationWindowSeconds" => 300, "selectPolicy" => "Min", "policies" => [
      { "type" => "Pods", "value" => 1, "periodSeconds" => down_period }
    ] }
    require_contract(autoscale.dig("behavior", "scaleUp") == expected_up, "fast bounded scale-up")
    require_contract(autoscale.dig("behavior", "scaleDown") == expected_down, "slow stabilized scale-down")
    true
  end

  # This calculation consumes an already-qualified measurement. It does not
  # establish sample completeness, correctness, or production permission.
  def self.downscale_period(p95_seconds)
    require_contract(p95_seconds.is_a?(Numeric) && p95_seconds.finite? && p95_seconds.positive?,
      "qualified positive finite p95 required; missing timing holds S3")
    period = p95_seconds > 90 ? (p95_seconds + 30).ceil : 120
    require_contract(period <= 1800, "required period exceeds Kubernetes range; HOLD, do not clamp")
    period
  end

  def self.hpa_candidate(role, p95_seconds)
    period = downscale_period(p95_seconds)
    deployment, hpa, pdb = documents(role)
    validate(role, deployment, hpa, pdb)
    hpa.fetch("spec").fetch("behavior").fetch("scaleDown").fetch("policies")[0]["periodSeconds"] = period
    validate(role, deployment, hpa, pdb, down_period: period)
    hpa
  end

  # Emit only this change's owned fields. A whole Deployment apply could restore
  # stale image pins or overwrite newer production configuration and annotations.
  def self.patch(role, replicas)
    require_contract([0, 1, 2].include?(replicas), "prepare at zero, activate two, compatible rollback one")
    deployment, hpa, pdb = documents(role)
    validate(role, deployment, hpa, pdb)
    spec = deployment.fetch("spec")
    pod = spec.fetch("template").fetch("spec")
    container = pod.fetch("containers")[0]
    { "spec" => { "replicas" => replicas, "minReadySeconds" => spec.fetch("minReadySeconds"),
      "strategy" => spec.fetch("strategy"), "template" => { "spec" => {
        "terminationGracePeriodSeconds" => pod.fetch("terminationGracePeriodSeconds"),
        "topologySpreadConstraints" => pod.fetch("topologySpreadConstraints"),
        "containers" => [container.select { |key, _| %w[name resources readinessProbe lifecycle].include?(key) }]
      } } } }
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    if ARGV.empty?
      CmaScaleManifests::ROLES.each_key { |role| CmaScaleManifests.validate(role, *CmaScaleManifests.documents(role)) }
      puts "CMA scale manifests: both roles pass"
    elsif ARGV.length == 3 && ARGV[0] == "--hpa"
      puts JSON.pretty_generate(CmaScaleManifests.hpa_candidate(ARGV[1], Float(ARGV[2])))
    elsif ARGV.length == 3 && ARGV[0] == "--patch"
      puts JSON.pretty_generate(CmaScaleManifests.patch(ARGV[1], Integer(ARGV[2], 10)))
    else
      raise "Usage: cma-scale-manifest-contract.rb [--patch api|cmaeg 0|1|2 | --hpa api|cmaeg QUALIFIED_P95_SECONDS]"
    end
  rescue StandardError => error
    warn error.message
    exit 1
  end
end

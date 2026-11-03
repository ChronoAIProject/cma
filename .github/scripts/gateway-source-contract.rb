# frozen_string_literal: true

require "yaml"
require "json"

module GatewaySourceContract
  def self.documents(path)
    Psych.parse_stream(File.read(path)).children.map do |document|
      stream = Psych::Nodes::Stream.new
      stream.children << document
      YAML.safe_load(stream.to_yaml, aliases: false)
    end
  end

  def self.object(kind, name, spec)
    labels = { "app.kubernetes.io/part-of" => "cma" }
    labels["app.kubernetes.io/name"] = name unless name == "cma-http-redirect"
    {
      "apiVersion" => kind == "HTTPRoute" ? "gateway.networking.k8s.io/v1" : "networking.gke.io/v1",
      "kind" => kind, "metadata" => { "name" => name, "namespace" => "chrono-bot", "labels" => labels },
      "spec" => spec,
    }
  end

  def self.parent(section)
    [{ "name" => "chronoai-gateway", "namespace" => "gateway-infra", "sectionName" => section }]
  end

  def self.path(type, value)
    { "path" => { "type" => type, "value" => value } }
  end

  def self.backend(name)
    [{ "group" => "", "kind" => "Service", "name" => name, "port" => 80, "weight" => 1 }]
  end

  def self.service_route(service, target, replacement)
    { "matches" => [path("PathPrefix", "/api/#{service}")],
      "filters" => [{ "type" => "URLRewrite", "urlRewrite" => { "path" => { "type" => "ReplacePrefixMatch", "replacePrefixMatch" => replacement } } }],
      "backendRefs" => backend(target) }
  end

  def self.gateway_public_route(type, suffix, replacement)
    replacement_type = type == "Exact" ? "ReplaceFullPath" : "ReplacePrefixMatch"
    replacement_key = type == "Exact" ? "replaceFullPath" : "replacePrefixMatch"
    { "matches" => [path(type, "/event-gateway/#{suffix}")],
      "filters" => [{ "type" => "URLRewrite", "urlRewrite" => { "path" => { "type" => replacement_type, replacement_key => replacement } } }],
      "backendRefs" => backend("cma-event-gateway") }
  end

  def self.gateway_route(name, spec)
    value = object("HTTPRoute", name, spec)
    value["metadata"]["labels"]["app.kubernetes.io/name"] = "cma-event-gateway"
    value
  end

  def self.service(name, workload)
    value = object("Service", name, {
      "type" => "ClusterIP", "selector" => { "app.kubernetes.io/name" => workload },
      "ports" => [{ "name" => "http", "port" => 80, "targetPort" => "http", "protocol" => "TCP" }],
    })
    value["apiVersion"] = "v1"
    value["metadata"]["labels"]["app.kubernetes.io/name"] = workload
    value
  end

  def self.nyxid_route(route, name, listener = "https-nyxid")
    value = Marshal.load(Marshal.dump(route))
    value["metadata"]["name"] = name
    value["spec"]["hostnames"] = ["cma.nyxid.ai"]
    value["spec"]["parentRefs"] = parent(listener)
    value
  end

  def self.expected
    all = [path("PathPrefix", "/")]
    routes = [
      object("HTTPRoute", "cma-agent-manager", {
        "parentRefs" => parent("https"), "hostnames" => ["bot-api.chrono-ai.fun"],
        "rules" => [{ "matches" => all, "backendRefs" => backend("cma-agent-manager") }],
      }),
      object("HTTPRoute", "cma-web", {
        "parentRefs" => parent("https"), "hostnames" => ["bot.chrono-ai.fun"],
        "rules" => [
          { "matches" => [path("PathPrefix", "/api/event-gateway")], "backendRefs" => backend("cma-agent-manager") },
          service_route("image-manager", "cma-image-manager", "/"),
          service_route("agent-manager", "cma-agent-manager", "/api"),
          { "matches" => [path("PathPrefix", "/api"), path("PathPrefix", "/auth"), path("Exact", "/openapi.json"), path("Exact", "/agent-manager/openapi.json"), path("Exact", "/.well-known/agent-card.json")], "backendRefs" => backend("cma-agent-manager") },
          { "matches" => [path("Exact", "/image-manager/openapi.json")], "backendRefs" => backend("cma-image-manager") },
          { "matches" => all, "backendRefs" => backend("cma-web") },
        ],
      }),
      object("HTTPRoute", "cma-http-redirect", {
        "parentRefs" => parent("http"), "hostnames" => %w[bot.chrono-ai.fun bot-api.chrono-ai.fun],
        "rules" => [{ "matches" => all, "filters" => [{ "type" => "RequestRedirect", "requestRedirect" => { "scheme" => "https", "statusCode" => 301 } }] }],
      }),
    ]
    routes << nyxid_route(routes[1], "cma-web-nyxid")
    routes << nyxid_route(routes[2], "cma-nyxid-http-redirect", "http")
    health = object("HealthCheckPolicy", "cma-agent-manager", {
      "targetRef" => { "group" => "", "kind" => "Service", "name" => "cma-agent-manager" },
      "default" => { "config" => { "type" => "HTTP", "httpHealthCheck" => { "port" => 8080, "requestPath" => "/ready" } } },
    })
    values = { "infra/api/httproute.yaml" => routes, "infra/api/healthcheckpolicy.yaml" => [health] }
    values["infra/cmaeg/httproute.yaml"] = [
      gateway_route("cma-event-gateway", {
        "parentRefs" => parent("https"), "hostnames" => ["bot-gateway.chrono-ai.fun"],
        "rules" => [{ "matches" => all, "backendRefs" => backend("cma-event-gateway") }],
      }),
      gateway_route("cma-event-gateway-http-redirect", {
        "parentRefs" => parent("http"), "hostnames" => ["bot-gateway.chrono-ai.fun"],
        "rules" => [{ "matches" => all, "filters" => [{ "type" => "RequestRedirect", "requestRedirect" => { "scheme" => "https", "statusCode" => 301 } }] }],
      }),
      gateway_route("cma-event-gateway-public", {
        "parentRefs" => parent("https"), "hostnames" => ["bot.chrono-ai.fun"],
        "rules" => [
          gateway_public_route("PathPrefix", "v1", "/v1"),
          gateway_public_route("PathPrefix", "mcp", "/mcp"),
          gateway_public_route("PathPrefix", "openapi.json", "/openapi.json"),
          gateway_public_route("PathPrefix", "docs", "/docs"),
          { "matches" => [path("PathPrefix", "/event-gateway/connect"), path("PathPrefix", "/event-gateway/legacy-console")], "backendRefs" => backend("cma-event-gateway") },
        ],
      }),
    ]
    gateway_routes = values.fetch("infra/cmaeg/httproute.yaml")
    gateway_routes << nyxid_route(gateway_routes[2], "cma-event-gateway-public-nyxid")
    values["infra/api/service.yaml"] = [service("cma-agent-manager", "cma-agent-manager"), service("cma-api", "cma-agent-manager")]
    values["infra/cmaeg/service.yaml"] = [service("cma-event-gateway", "cma-event-gateway"), service("cmaeg", "cma-event-gateway")]
    %w[api web].each do |name|
      service = name == "api" ? "cma-agent-manager" : "cma-web"
      values["infra/#{name}/gcpbackendpolicy.yaml"] = [object("GCPBackendPolicy", service, {
        "targetRef" => { "group" => "", "kind" => "Service", "name" => service },
        "default" => { "timeoutSec" => 660, "connectionDraining" => { "drainingTimeoutSec" => 660 } },
      })]
    end
    values
  end

  def self.validate(actual)
    raise "Gateway source objects differ from exact owned routes, readiness and timeout policies" unless actual == expected
  end

  def self.check(root)
    validate(expected.keys.to_h { |path| [path, documents(File.join(root, path))] })
    config = documents(File.join(root, "infra/cmaeg/configmap.yaml")).first
    validate_provider_origin(config)
  end

  def self.validate_provider_origin(config)
    raise "Existing ChannelPointer record authority must survive the service rename" unless config.dig("data", "CMAEG_CMA_BASE_URL") == "http://example-host-01.example-namespace.svc"
    data = config.fetch("data")
    providers = JSON.parse(data.fetch("CMAEG_PROVIDERS")).select { |provider| provider["slug"] == "cma_codex" }
    # The provider client selects its CMA service bearer by exact origin.
    # A DNS alias that reaches the same Pods still falls back to the Agent key.
    unless providers.length == 1 && providers.first["base_url"] == data.fetch("CMAEG_CMA_BASE_URL")
      raise "CMA provider origin must equal CMAEG_CMA_BASE_URL for service authentication"
    end
  end
end

GatewaySourceContract.check(File.expand_path("../..", __dir__))

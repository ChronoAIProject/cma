# frozen_string_literal: true

require "yaml"

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

  def self.expected
    all = [path("PathPrefix", "/")]
    routes = [
      object("HTTPRoute", "cma-api", {
        "parentRefs" => parent("https"), "hostnames" => ["bot-api.chrono-ai.fun"],
        "rules" => [{ "matches" => all, "backendRefs" => backend("cma-api") }],
      }),
      object("HTTPRoute", "cma-web", {
        "parentRefs" => parent("https"), "hostnames" => ["bot.chrono-ai.fun"],
        "rules" => [
          service_route("event-gateway", "cmaeg", "/"),
          service_route("image-manager", "cma-image-manager", "/"),
          service_route("agent-manager", "cma-api", "/api"),
          { "matches" => [path("PathPrefix", "/api"), path("PathPrefix", "/auth"), path("Exact", "/openapi.json"), path("Exact", "/.well-known/agent-card.json")], "backendRefs" => backend("cma-api") },
          { "matches" => all, "backendRefs" => backend("cma-web") },
        ],
      }),
      object("HTTPRoute", "cma-http-redirect", {
        "parentRefs" => parent("http"), "hostnames" => %w[bot.chrono-ai.fun bot-api.chrono-ai.fun],
        "rules" => [{ "matches" => all, "filters" => [{ "type" => "RequestRedirect", "requestRedirect" => { "scheme" => "https", "statusCode" => 301 } }] }],
      }),
    ]
    health = object("HealthCheckPolicy", "cma-api", {
      "targetRef" => { "group" => "", "kind" => "Service", "name" => "cma-api" },
      "default" => { "config" => { "type" => "HTTP", "httpHealthCheck" => { "port" => 8080, "requestPath" => "/ready" } } },
    })
    values = { "infra/api/httproute.yaml" => routes, "infra/api/healthcheckpolicy.yaml" => [health] }
    %w[api web].each do |name|
      values["infra/#{name}/gcpbackendpolicy.yaml"] = [object("GCPBackendPolicy", "cma-#{name}", {
        "targetRef" => { "group" => "", "kind" => "Service", "name" => "cma-#{name}" },
        "default" => { "timeoutSec" => 660 },
      })]
    end
    values
  end

  def self.validate(actual)
    raise "Gateway source objects differ from exact owned routes, readiness and timeout policies" unless actual == expected
  end

  def self.check(root)
    validate(expected.keys.to_h { |path| [path, documents(File.join(root, path))] })
  end
end

GatewaySourceContract.check(File.expand_path("../..", __dir__))

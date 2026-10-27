# frozen_string_literal: true

require "yaml"

path = File.expand_path("../../infra/api/httproute.yaml", __dir__)
routes = YAML.load_stream(File.read(path)).select { |doc| doc&.fetch("kind", nil) == "HTTPRoute" }

def route!(routes, name, hostname)
  route = routes.find { |doc| doc.dig("metadata", "name") == name }
  abort("missing HTTPRoute #{name}") unless route
  abort("#{name} has unexpected namespace") unless route.dig("metadata", "namespace") == "chrono-bot"
  abort("#{name} has unexpected hostnames") unless route.dig("spec", "hostnames") == [hostname]

  expected_parent = [{ "name" => "chronoai-gateway", "namespace" => "gateway-infra", "sectionName" => "https" }]
  abort("#{name} has unexpected parentRefs") unless route.dig("spec", "parentRefs") == expected_parent
  route
end

def ownership(route)
  route.dig("spec", "rules").each_with_object({}) do |rule, paths|
    backends = rule.fetch("backendRefs")
    rule.fetch("matches").each do |match|
      abort("route match has constraints beyond its path") unless match.keys == ["path"]
      path = match.fetch("path")
      abort("route path has unexpected fields") unless path.keys.sort == %w[type value]
      key = [path.fetch("type"), path.fetch("value")]
      abort("duplicate route match #{key.inspect}") if paths.key?(key)
      paths[key] = { "backends" => backends, "filters" => rule.fetch("filters", []) }
    end
  end
end

https_routes = routes.select do |route|
  route.dig("spec", "parentRefs").any? { |parent| parent["sectionName"] != "http" }
end
expected_names = %w[cma-agent-manager cma-web]
actual_names = https_routes.map { |route| route.dig("metadata", "name") }.sort
abort("unexpected competing HTTPS routes: #{actual_names.inspect}") unless actual_names == expected_names

api = ownership(route!(routes, "cma-agent-manager", "bot-api.chrono-ai.fun"))
web = ownership(route!(routes, "cma-web", "bot.chrono-ai.fun"))

api_backend = [{ "group" => "", "kind" => "Service", "name" => "cma-agent-manager", "port" => 80, "weight" => 1 }]
web_backend = [{ "group" => "", "kind" => "Service", "name" => "cma-web", "port" => 80, "weight" => 1 }]
expected_api = { ["PathPrefix", "/"] => api_backend }
expected_web = {
  # The cookie-authenticated portal bridge retains its full path at Agent Manager.
  ["PathPrefix", "/api/event-gateway"] => api_backend,
  ["PathPrefix", "/api"] => api_backend,
  ["PathPrefix", "/auth"] => api_backend,
  ["Exact", "/openapi.json"] => api_backend,
  ["Exact", "/agent-manager/openapi.json"] => api_backend,
  ["Exact", "/image-manager/openapi.json"] => [{ "group" => "", "kind" => "Service", "name" => "cma-image-manager", "port" => 80, "weight" => 1 }],
  ["Exact", "/.well-known/agent-card.json"] => api_backend,
  ["PathPrefix", "/"] => web_backend,
}

expected_api.transform_values! { |backend| { "backends" => backend, "filters" => [] } }
expected_web.transform_values! { |backend| { "backends" => backend, "filters" => [] } }
{
  "/api/image-manager" => ["cma-image-manager", "/"],
  "/api/agent-manager" => ["cma-agent-manager", "/api"],
}.each do |prefix, (service, replacement)|
  expected_web[["PathPrefix", prefix]] = {
    "backends" => [{ "group" => "", "kind" => "Service", "name" => service, "port" => 80, "weight" => 1 }],
    "filters" => [{ "type" => "URLRewrite", "urlRewrite" => { "path" => { "type" => "ReplacePrefixMatch", "replacePrefixMatch" => replacement } } }],
  }
end

abort("unexpected API route ownership: #{api.inspect}") unless api == expected_api
abort("unexpected app route ownership: #{web.inspect}") unless web == expected_web

puts "HTTPRoute backend ownership is exact"

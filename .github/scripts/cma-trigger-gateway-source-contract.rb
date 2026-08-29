#!/usr/bin/env ruby
# frozen_string_literal: true

require "json"
require "yaml"

ROOT = File.expand_path("../..", __dir__)
ROUTE_PATH = File.join(ROOT, "infra/api/httproute.yaml")
HEALTH_PATH = File.join(ROOT, "infra/api/healthcheckpolicy.yaml")
BUNDLE_FILENAMES = %w[
  cma-api-health.json
  cma-api-route.json
  cma-http-redirect-route.json
  cma-web-route.json
].freeze

class GatewaySourceContractError < StandardError; end

def api_backend
  [{ "group" => "", "kind" => "Service", "name" => "cma-api", "port" => 80, "weight" => 1 }]
end

def web_backend
  [{ "group" => "", "kind" => "Service", "name" => "cma-web", "port" => 80, "weight" => 1 }]
end

def https_parent
  [{ "name" => "chronoai-gateway", "namespace" => "gateway-infra", "sectionName" => "https" }]
end

def route_metadata(name, labels)
  {
    "name" => name,
    "namespace" => "chrono-bot",
    "labels" => labels,
  }
end

def expected_routes
  [
    {
      "apiVersion" => "gateway.networking.k8s.io/v1",
      "kind" => "HTTPRoute",
      "metadata" => route_metadata(
        "cma-api",
        { "app.kubernetes.io/name" => "cma-api", "app.kubernetes.io/part-of" => "cma" },
      ),
      "spec" => {
        "parentRefs" => https_parent,
        "hostnames" => ["bot-api.chrono-ai.fun"],
        "rules" => [
          {
            "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
            "backendRefs" => api_backend,
          },
        ],
      },
    },
    {
      "apiVersion" => "gateway.networking.k8s.io/v1",
      "kind" => "HTTPRoute",
      "metadata" => route_metadata(
        "cma-web",
        { "app.kubernetes.io/name" => "cma-web", "app.kubernetes.io/part-of" => "cma" },
      ),
      "spec" => {
        "parentRefs" => https_parent,
        "hostnames" => ["bot.chrono-ai.fun"],
        "rules" => [
          {
            "matches" => [
              { "path" => { "type" => "PathPrefix", "value" => "/api" } },
              { "path" => { "type" => "PathPrefix", "value" => "/auth" } },
              { "path" => { "type" => "Exact", "value" => "/openapi.json" } },
              { "path" => { "type" => "Exact", "value" => "/.well-known/agent-card.json" } },
            ],
            "backendRefs" => api_backend,
          },
          {
            "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
            "backendRefs" => web_backend,
          },
        ],
      },
    },
    {
      "apiVersion" => "gateway.networking.k8s.io/v1",
      "kind" => "HTTPRoute",
      "metadata" => route_metadata(
        "cma-http-redirect",
        { "app.kubernetes.io/part-of" => "cma" },
      ),
      "spec" => {
        "parentRefs" => [
          { "name" => "chronoai-gateway", "namespace" => "gateway-infra", "sectionName" => "http" },
        ],
        "hostnames" => ["bot.chrono-ai.fun", "bot-api.chrono-ai.fun"],
        "rules" => [
          {
            "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
            "filters" => [
              {
                "type" => "RequestRedirect",
                "requestRedirect" => { "scheme" => "https", "statusCode" => 301 },
              },
            ],
          },
        ],
      },
    },
  ]
end

def expected_health
  {
    "apiVersion" => "networking.gke.io/v1",
    "kind" => "HealthCheckPolicy",
    "metadata" => route_metadata(
      "cma-api",
      { "app.kubernetes.io/name" => "cma-api", "app.kubernetes.io/part-of" => "cma" },
    ),
    "spec" => {
      "targetRef" => { "group" => "", "kind" => "Service", "name" => "cma-api" },
      "default" => {
        "config" => {
          "type" => "HTTP",
          "httpHealthCheck" => { "port" => 8080, "requestPath" => "/ready" },
        },
      },
    },
  }
end

def safe_documents(path)
  stream = Psych.parse_stream(File.read(path))
  stream.children.map do |document|
    one = Psych::Nodes::Stream.new
    one.children << document
    YAML.safe_load(
      one.to_yaml,
      permitted_classes: [],
      permitted_symbols: [],
      aliases: false,
    )
  end
rescue Psych::Exception => error
  raise GatewaySourceContractError, "#{path} must be safe YAML: #{error.message}"
end

def validate_documents(routes, health)
  unless routes == expected_routes
    raise GatewaySourceContractError,
          "httproute.yaml must contain exactly the three owned routes with exact metadata and specs"
  end
  unless health == [expected_health]
    raise GatewaySourceContractError,
          "healthcheckpolicy.yaml must contain exactly the owned /ready policy"
  end
end

def load_and_validate
  routes = safe_documents(ROUTE_PATH)
  health = safe_documents(HEALTH_PATH)
  validate_documents(routes, health)
  [routes, health.fetch(0)]
end

def normalized_route(route)
  normalized = cloned(route)
  normalized.fetch("spec").fetch("parentRefs").each do |parent|
    parent["group"] = "gateway.networking.k8s.io"
    parent["kind"] = "Gateway"
    parent.replace(parent.sort.to_h)
  end
  normalized
end

def emit_json(directory, routes, health)
  stat = File.lstat(directory)
  unless stat.directory? && !stat.symlink? && stat.uid == Process.uid && (stat.mode & 0o077).zero?
    raise GatewaySourceContractError, "emit directory must be private, owned, and not a symlink"
  end
  unless Dir.children(directory).empty?
    raise GatewaySourceContractError, "emit directory must be empty"
  end

  objects = {
    "cma-api-route.json" => normalized_route(routes.fetch(0)),
    "cma-web-route.json" => normalized_route(routes.fetch(1)),
    "cma-http-redirect-route.json" => normalized_route(routes.fetch(2)),
    "cma-api-health.json" => health,
  }
  objects.each do |name, object|
    path = File.join(directory, name)
    File.open(path, File::WRONLY | File::CREAT | File::EXCL, 0o600) do |file|
      file.write(JSON.pretty_generate(object))
      file.write("\n")
    end
  end
end

def fsync_directory(path)
  File.open(path, File::RDONLY) { |directory| directory.fsync }
end

def seal_bundle(directory)
  directory_stat = File.lstat(directory)
  unless File.absolute_path(directory) == directory &&
         directory_stat.directory? && !directory_stat.symlink? &&
         directory_stat.uid == Process.uid && (directory_stat.mode & 0o077).zero?
    raise GatewaySourceContractError,
          "bundle directory must be absolute, private, owned, and not a symlink"
  end
  unless Dir.children(directory).sort == BUNDLE_FILENAMES
    raise GatewaySourceContractError, "bundle must contain exactly the four owned JSON files"
  end

  BUNDLE_FILENAMES.each do |name|
    path = File.join(directory, name)
    stat = File.lstat(path)
    unless stat.file? && !stat.symlink? && stat.uid == Process.uid &&
           (stat.mode & 0o777) == 0o600
      raise GatewaySourceContractError,
            "bundle file must be owned, regular, non-symlink, and mode 0600: #{name}"
    end
    File.open(path, File::RDWR) do |file|
      File.chmod(0o400, path)
      file.fsync
    end
  end
  fsync_directory(directory)
  fsync_directory(File.dirname(directory))
end

def cloned(value)
  Marshal.load(Marshal.dump(value))
end

def expect_rejection(name)
  yield
rescue GatewaySourceContractError
  return
end

def run_self_tests
  routes = expected_routes
  health = [expected_health]
  validate_documents(cloned(routes), cloned(health))

  mutations = {
    "extra route document" => lambda do |candidate_routes, _candidate_health|
      candidate_routes << cloned(candidate_routes.fetch(0))
    end,
    "alternate route identity" => lambda do |candidate_routes, _candidate_health|
      candidate_routes.fetch(0).fetch("metadata")["name"] = "cma-api-shadow"
    end,
    "unowned route metadata" => lambda do |candidate_routes, _candidate_health|
      candidate_routes.fetch(1).fetch("metadata")["finalizers"] = ["example.invalid/hold"]
    end,
    "alternate API backend" => lambda do |candidate_routes, _candidate_health|
      candidate_routes.fetch(0).fetch("spec").fetch("rules").fetch(0)
        .fetch("backendRefs").fetch(0)["name"] = "cma-web"
    end,
    "liveness health policy" => lambda do |_candidate_routes, candidate_health|
      candidate_health.fetch(0).fetch("spec").fetch("default").fetch("config")
        .fetch("httpHealthCheck")["requestPath"] = "/livez"
    end,
    "extra health document" => lambda do |_candidate_routes, candidate_health|
      candidate_health << cloned(candidate_health.fetch(0))
    end,
  }

  mutations.each do |name, mutation|
    candidate_routes = cloned(routes)
    candidate_health = cloned(health)
    mutation.call(candidate_routes, candidate_health)
    rejected = false
    begin
      validate_documents(candidate_routes, candidate_health)
    rescue GatewaySourceContractError
      rejected = true
    end
    raise GatewaySourceContractError, "self-test accepted #{name}" unless rejected
  end

  puts "CMA Trigger Gateway source contract self-tests passed (#{mutations.length} mutations)"
end

unless ARGV.empty? || ARGV == ["--self-test"] ||
       (ARGV.length == 2 && %w[--emit --seal-bundle].include?(ARGV[0]))
  warn "usage: #{File.basename($PROGRAM_NAME)} [--self-test | --emit DIR | --seal-bundle DIR]"
  exit 2
end

begin
  routes, health = load_and_validate
  if ARGV == ["--self-test"]
    run_self_tests
  elsif ARGV.first == "--emit"
    emit_json(ARGV.fetch(1), routes, health)
    puts "CMA Trigger Gateway source bundle emitted"
  elsif ARGV.first == "--seal-bundle"
    seal_bundle(ARGV.fetch(1))
    puts "CMA Trigger Gateway rollback bundle durably sealed"
  else
    puts "CMA Trigger Gateway source contract passed"
  end
rescue GatewaySourceContractError, SystemCallError, KeyError, TypeError => error
  warn "CMA Trigger Gateway source contract: #{error.message}"
  exit 1
end

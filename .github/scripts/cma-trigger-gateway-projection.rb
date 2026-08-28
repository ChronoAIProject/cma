#!/usr/bin/env ruby
# frozen_string_literal: true

require "digest"
require "json"
require "openssl"
require "time"

API_ROOT = "https://www.googleapis.com/compute/v1"
PROJECT_ID = "example-project-02"
PROJECT_NUMBER = "191845086376"
CMA_HOSTS = %w[bot.chrono-ai.fun bot-api.chrono-ai.fun].freeze
CMA_SERVICES = %w[cma-api cma-web].freeze
LIVE_POLICY_PATH = File.expand_path("cma-trigger-gateway-live-policy.json", __dir__)
OBSERVATIONAL_COMPUTE_KEYS = %w[
  creationTimestamp description fingerprint id kind name selfLink
].freeze

class GatewayProjectionError < StandardError; end

def reject!(message)
  raise GatewayProjectionError, message
end

def assert!(condition, message)
  reject!(message) unless condition
end

def object!(value, label)
  assert!(value.is_a?(Hash), "#{label} must be an object")
  value
end

def array!(value, label)
  assert!(value.is_a?(Array), "#{label} must be an array")
  value
end

def string!(value, label)
  assert!(value.is_a?(String) && !value.empty?, "#{label} must be a nonempty string")
  value
end

def canonical(value)
  case value
  when Hash
    value.keys.sort.each_with_object({}) { |key, result| result[key] = canonical(value.fetch(key)) }
  when Array
    value.map { |entry| canonical(entry) }
  else
    value
  end
end

def canonical_json(value)
  JSON.generate(canonical(value))
end

def without_keys(value, keys)
  object!(value, "behavior-bearing resource").reject { |key, _child| keys.include?(key) }
end

def certificate_behavior(certificate)
  body = without_keys(certificate, OBSERVATIONAL_COMPUTE_KEYS)
  if body.key?("certificate")
    body["certificateContract"] = "active-secret-derived-public-chain"
    body.delete("certificate")
  end
  canonical(body)
end

def pem_certificate_identities(pem, label)
  pem = string!(pem, "#{label} PEM")
  blocks = pem.scan(/-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----/m)
  assert!(!blocks.empty? && pem.gsub(/-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----/m, "").strip.empty?,
          "#{label} is not a closed PEM certificate chain")
  blocks.map.with_index do |block, index|
    certificate = OpenSSL::X509::Certificate.new(block)
    {
      "derSha256" => Digest::SHA256.hexdigest(certificate.to_der),
      "serialHex" => certificate.serial.to_i.to_s(16),
      "subject" => certificate.subject.to_s(OpenSSL::X509::Name::RFC2253),
      "issuer" => certificate.issuer.to_s(OpenSSL::X509::Name::RFC2253),
      "notBefore" => certificate.not_before.utc.iso8601,
      "notAfter" => certificate.not_after.utc.iso8601,
      "chainIndex" => index,
    }
  end
rescue OpenSSL::X509::CertificateError => error
  raise GatewayProjectionError, "#{label} is not a valid X.509 chain: #{error.message}"
end

def certificate_secret_projection(secret)
  assert!(secret["apiVersion"] == "v1" && secret["kind"] == "Secret",
          "Gateway certificate Secret type is not exact")
  assert!(secret.dig("metadata", "namespace") == "gateway-infra" &&
          secret.dig("metadata", "name") == "chrono-ai-fun-cert",
          "Gateway certificate Secret identity is not exact")
  identity = resource_identity(secret, "Gateway certificate Secret")
  assert!(secret.dig("metadata", "deletionTimestamp").nil?,
          "Gateway certificate Secret is being deleted")
  assert!(secret["type"] == "kubernetes.io/tls" && secret["dataKeys"] == %w[tls.crt tls.key],
          "Gateway certificate Secret type/key set is not exact")
  pem = string!(secret["certificatePem"], "Gateway certificate Secret public certificate")
  assert!(secret["keyPresent"] == true, "Gateway certificate Secret lacks its in-memory key proof")
  private_key_spki = string!(secret["publicSpkiSha256"],
                             "Gateway certificate Secret public-SPKI digest")
  assert!(private_key_spki.match?(/\A[0-9a-f]{64}\z/),
          "Gateway certificate Secret public-SPKI digest is malformed")
  chain = pem_certificate_identities(pem, "Gateway certificate Secret public certificate")
  leaf = OpenSSL::X509::Certificate.new(pem.scan(/-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----/m).first)
  leaf_spki = Digest::SHA256.hexdigest(leaf.public_key.to_der)
  assert!(leaf_spki == private_key_spki,
          "Gateway certificate Secret private key does not match its leaf certificate")
  identity.merge(
    "type" => secret["type"],
    "dataKeys" => secret["dataKeys"],
    "certificatePemSha256" => Digest::SHA256.hexdigest(pem),
    "certificateChain" => chain,
    "publicSpkiSha256" => leaf_spki,
  )
end

def compute_behavior_policy(state)
  compute = object!(state["compute"], "compute capture")
  backends = array!(compute["backendServices"], "backend services")
  health_by_link = indexed(compute["healthChecks"], "health checks")
  neg_by_link = array!(compute["networkEndpointGroups"], "network endpoint groups").to_h do |entry|
    resource = object!(entry["resource"], "NEG resource")
    [compute_ref(resource["selfLink"]), resource]
  end
  service_policies = CMA_SERVICES.to_h do |service_name|
    marker = "-chrono-bot-#{service_name}-80-"
    matches = backends.select { |backend| compute_ref(backend["selfLink"]).include?(marker) }
    assert!(matches.length == 1, "#{service_name} behavior policy backend is ambiguous")
    backend = matches.fetch(0)
    group_behaviors = array!(backend["backends"], "#{service_name} backend groups").map do |group|
      canonical(without_keys(group, ["group"]))
    end.sort_by { |entry| canonical_json(entry) }
    health_behaviors = array!(backend["healthChecks"], "#{service_name} health links").map do |link|
      behavior = canonical(without_keys(
        health_by_link.fetch(compute_ref(link)), OBSERVATIONAL_COMPUTE_KEYS,
      ))
      if service_name == "cma-api"
        expected_path = state["routeMode"] == "legacy" ? "/livez" : "/ready"
        assert!(behavior.dig("httpHealthCheck", "requestPath") == expected_path,
                "cma-api effective health path does not match the route phase")
        behavior["httpHealthCheck"]["requestPath"] = "$CMA_ROUTE_MODE_HEALTH_PATH"
      end
      canonical(behavior)
    end.sort_by { |entry| canonical_json(entry) }
    neg_behaviors = array!(backend["backends"], "#{service_name} backend groups").map do |group|
      resource = neg_by_link.fetch(compute_ref(group.fetch("group")))
      canonical(without_keys(resource, OBSERVATIONAL_COMPUTE_KEYS + %w[zone]))
    end.sort_by { |entry| canonical_json(entry) }
    [service_name, {
      "backend" => canonical(without_keys(
        backend,
        OBSERVATIONAL_COMPUTE_KEYS + %w[backends healthChecks],
      )),
      "backendGroups" => group_behaviors,
      "healthChecks" => health_behaviors,
      "networkEndpointGroups" => neg_behaviors,
    }]
  end
  http_proxy = array!(compute["targetHttpProxies"], "target HTTP proxies")
  https_proxy = array!(compute["targetHttpsProxies"], "target HTTPS proxies")
  assert!(http_proxy.length == 1 && https_proxy.length == 1,
          "behavior policy requires exactly one HTTP and HTTPS proxy")
  https_map_ref = compute_ref(https_proxy.fetch(0)["urlMap"])
  url_map_behaviors = array!(compute["urlMaps"], "URL maps").map do |url_map|
    body = canonical(without_keys(url_map, OBSERVATIONAL_COMPUTE_KEYS))
    if compute_ref(url_map["selfLink"]) == https_map_ref
      host_rule = array!(body["hostRules"], "serving behavior hostRules").find do |rule|
        array!(rule["hosts"], "serving behavior hosts").include?("bot.chrono-ai.fun")
      end
      assert!(host_rule, "serving behavior lacks the CMA app host")
      matcher = array!(body["pathMatchers"], "serving behavior pathMatchers").find do |candidate|
        candidate["name"] == host_rule["pathMatcher"]
      end
      assert!(matcher, "serving behavior lacks the CMA app matcher")
      matcher.delete("pathRules")
      matcher.delete("routeRules")
      matcher["cmaRouteModePaths"] = "$CMA_ROUTE_MODE_APP_API_PATHS"
    end
    canonical(body)
  end.sort_by { |entry| canonical_json(entry) }
  {
    "address" => canonical(without_keys(
      object!(compute["address"], "global address"), OBSERVATIONAL_COMPUTE_KEYS,
    )),
    "forwardingRules" => array!(compute["forwardingRules"], "forwarding rules")
      .map { |rule| canonical(without_keys(rule, OBSERVATIONAL_COMPUTE_KEYS)) }
      .sort_by { |entry| canonical_json(entry) },
    "urlMaps" => url_map_behaviors,
    "firewalls" => array!(compute["firewalls"], "Gateway firewalls")
      .map { |firewall| canonical(without_keys(firewall, OBSERVATIONAL_COMPUTE_KEYS)) }
      .sort_by { |entry| canonical_json(entry) },
    "certificateSource" => "active-secret-derived-public-chain",
    "targetHttpProxy" => canonical(without_keys(
      http_proxy.fetch(0), OBSERVATIONAL_COMPUTE_KEYS + %w[urlMap],
    )),
    "targetHttpsProxy" => canonical(without_keys(
      https_proxy.fetch(0),
      OBSERVATIONAL_COMPUTE_KEYS + %w[sslCertificates sslPolicy urlMap],
    )),
    "sslCertificates" => array!(compute["sslCertificates"], "SSL certificates")
      .map { |certificate| certificate_behavior(certificate) }
      .sort_by { |entry| canonical_json(entry) },
    "sslPolicies" => array!(compute["sslPolicies"], "SSL policies")
      .map { |policy| canonical(without_keys(policy, OBSERVATIONAL_COMPUTE_KEYS)) }
      .sort_by { |entry| canonical_json(entry) },
    "services" => service_policies,
  }
end

def load_live_policy
  bytes = File.binread(LIVE_POLICY_PATH)
  policy = JSON.parse(bytes)
  assert!(policy.is_a?(Hash) && policy.keys.sort == %w[expected schemaVersion sourceEvidenceSha256 status],
          "live behavior policy shape is not closed")
  assert!(policy["schemaVersion"] == 1, "live behavior policy schema is unsupported")
  assert!(policy["status"] == "approved", "live behavior policy is blocked pending reviewed capture")
  assert!(policy["sourceEvidenceSha256"].is_a?(String) &&
          policy["sourceEvidenceSha256"].match?(/\A[0-9a-f]{64}\z/),
          "live behavior policy lacks a reviewed source-evidence digest")
  {
    "expected" => object!(policy["expected"], "approved live behavior policy"),
    "policySha256" => Digest::SHA256.hexdigest(bytes),
    "sourceEvidenceSha256" => policy["sourceEvidenceSha256"],
  }
rescue JSON::ParserError => error
  raise GatewayProjectionError, "live behavior policy is invalid JSON: #{error.message}"
end

def parse_json_file(path)
  stat = File.lstat(path)
  assert!(stat.file? && !stat.symlink?, "capture must be a regular non-symlink file")
  JSON.parse(File.read(path))
rescue JSON::ParserError => error
  raise GatewayProjectionError, "capture must be valid JSON: #{error.message}"
end

def compute_ref(value)
  ref = string!(value, "Compute reference")
  return ref if ref.start_with?("#{API_ROOT}/projects/")
  return "#{API_ROOT}#{ref}" if ref.start_with?("/projects/")

  reject!("Compute reference is not an exact v1 self-link: #{ref}")
end

def annotation_refs(gateway, annotation)
  raw = gateway.dig("metadata", "annotations", annotation)
  return [] if raw == ""

  string!(raw, "Gateway annotation #{annotation}").split(",").map(&:strip).map do |ref|
    compute_ref(ref)
  end.sort
end

def indexed(resources, label)
  array!(resources, label).each_with_object({}) do |resource, result|
    body = object!(resource, "#{label} entry")
    link = compute_ref(body["selfLink"])
    assert!(!result.key?(link), "#{label} contains duplicate selfLink #{link}")
    result[link] = body
  end
end

def list_items(resource, label)
  body = object!(resource, label)
  assert!(body["kind"] == "List", "#{label} must be a Kubernetes List")
  array!(body["items"], "#{label}.items")
end

def named(items, kind, namespace, name)
  matches = items.select do |item|
    item["kind"] == kind && item.dig("metadata", "namespace") == namespace &&
      item.dig("metadata", "name") == name
  end
  assert!(matches.length == 1, "expected exactly one #{kind} #{namespace}/#{name}")
  matches.fetch(0)
end

def resource_identity(resource, label)
  metadata = object!(resource["metadata"], "#{label}.metadata")
  {
    "uid" => string!(metadata["uid"], "#{label} UID"),
    "resourceVersion" => string!(metadata["resourceVersion"], "#{label} resourceVersion"),
  }
end

def validate_gateway(gateway)
  assert!(gateway["apiVersion"] == "gateway.networking.k8s.io/v1", "Gateway apiVersion is not exact")
  assert!(gateway["kind"] == "Gateway", "Gateway kind is not exact")
  assert!(gateway.dig("metadata", "namespace") == "gateway-infra" &&
          gateway.dig("metadata", "name") == "chronoai-gateway", "Gateway identity is not exact")
  identity = resource_identity(gateway, "Gateway")
  generation = gateway.dig("metadata", "generation")
  assert!(generation.is_a?(Integer) && generation.positive?, "Gateway generation is not exact")
  unsupported_annotations = %w[
    networking.gke.io/addresses networking.gke.io/lb-edge-extensions
    networking.gke.io/lb-route-extensions networking.gke.io/lb-traffic-extensions
    networking.gke.io/target-tcp-proxies networking.gke.io/wasm-plugin-versions
    networking.gke.io/wasm-plugins
  ]
  unsupported_annotations.each do |annotation|
    assert!(gateway.dig("metadata", "annotations", annotation).to_s.empty?,
            "Gateway unsupported graph annotation #{annotation} is nonempty")
  end
  assert!(!annotation_refs(gateway, "networking.gke.io/firewalls").empty?,
          "Gateway firewall graph root is absent")

  expected_spec = {
    "addresses" => [{ "type" => "NamedAddress", "value" => "chronoai-gateway-external-ip" }],
    "gatewayClassName" => "gke-l7-global-external-managed",
    "listeners" => [
      {
        "allowedRoutes" => { "namespaces" => { "from" => "All" } },
        "name" => "http", "port" => 80, "protocol" => "HTTP",
      },
      {
        "allowedRoutes" => { "namespaces" => { "from" => "All" } },
        "name" => "https", "port" => 443, "protocol" => "HTTPS",
        "tls" => {
          "certificateRefs" => [{ "group" => "", "kind" => "Secret", "name" => "chrono-ai-fun-cert" }],
          "mode" => "Terminate",
        },
      },
    ],
  }
  assert!(gateway["spec"] == expected_spec, "shared Gateway specification is not exact")
  addresses = gateway.dig("status", "addresses")
  assert!(addresses.is_a?(Array) && addresses.length == 1 &&
          addresses.fetch(0)["type"] == "IPAddress", "Gateway must report one IP address")
  gateway_ip = string!(addresses.fetch(0)["value"], "Gateway status IP")
  assert!(gateway_ip.match?(/\A(?:\d{1,3}\.){3}\d{1,3}\z/), "Gateway status IP is not IPv4")

  programmed = array!(gateway.dig("status", "conditions"), "Gateway status conditions").select do |condition|
    condition["type"] == "Programmed" && condition["status"] == "True" &&
      condition["observedGeneration"] == generation
  end
  assert!(programmed.length == 1, "Gateway is not Programmed at its current generation")
  listeners = array!(gateway.dig("status", "listeners"), "Gateway listener status")
  assert!(listeners.map { |listener| listener["name"] }.sort == %w[http https],
          "Gateway listener status is not closed over http and https")
  listeners.each do |listener|
    conditions = array!(listener["conditions"], "Gateway listener conditions")
    %w[Accepted Programmed ResolvedRefs].each do |type|
      matches = conditions.select do |condition|
        condition["type"] == type && condition["status"] == "True" &&
          condition["observedGeneration"] == generation
      end
      assert!(matches.length == 1, "Gateway listener #{listener['name']} lacks current #{type}")
    end
  end
  [identity, gateway_ip]
end

def validate_service(service, name, expected_selector)
  assert!(service["apiVersion"] == "v1" && service["kind"] == "Service",
          "Service #{name} type is not exact")
  assert!(service.dig("metadata", "namespace") == "chrono-bot" &&
          service.dig("metadata", "name") == name, "Service #{name} identity is not exact")
  resource_identity(service, "Service #{name}")
  assert!(service.dig("metadata", "deletionTimestamp").nil?, "Service #{name} is being deleted")
  expected_labels = { "app.kubernetes.io/name" => name, "app.kubernetes.io/part-of" => "cma" }
  assert!(service.dig("metadata", "labels") == expected_labels, "Service #{name} labels are not exact")
  annotations = object!(service.dig("metadata", "annotations"), "Service #{name} annotations")
  assert!(annotations.keys.sort == %w[
    cloud.google.com/neg
    cloud.google.com/neg-status
    kubectl.kubernetes.io/last-applied-configuration
  ],
          "Service #{name} annotations include an unreviewed desired or controller input")
  neg_request = JSON.parse(string!(service.dig("metadata", "annotations", "cloud.google.com/neg"),
                                   "Service #{name} NEG annotation"))
  assert!(neg_request == { "exposed_ports" => { "80" => {} } },
          "Service #{name} desired NEG annotation is not exact")
  last_applied = JSON.parse(string!(
                              service.dig(
                                "metadata", "annotations",
                                "kubectl.kubernetes.io/last-applied-configuration",
                              ),
                              "Service #{name} last-applied annotation",
                            ))
  expected_last_applied = {
    "apiVersion" => "v1", "kind" => "Service",
    "metadata" => {
      "annotations" => {}, "labels" => expected_labels,
      "name" => name, "namespace" => "chrono-bot",
    },
    "spec" => {
      "ports" => [{ "name" => "http", "port" => 80, "protocol" => "TCP", "targetPort" => "http" }],
      "selector" => { "app.kubernetes.io/name" => name }, "type" => "ClusterIP",
    },
  }
  assert!(last_applied == expected_last_applied,
          "Service #{name} last-applied annotation is not the reviewed source manifest")
  neg_status = JSON.parse(string!(service.dig("metadata", "annotations", "cloud.google.com/neg-status"),
                                  "Service #{name} NEG status"))
  assert!(neg_status.keys.sort == %w[network_endpoint_groups zones],
          "Service #{name} NEG status shape is open")
  assert!(neg_status["network_endpoint_groups"].keys == ["80"],
          "Service #{name} NEG status ports are not exact")
  neg_name = string!(neg_status.dig("network_endpoint_groups", "80"), "Service #{name} NEG name")
  assert!(neg_name.match?(/\Ak8s1-[a-z0-9]+-chrono-bot-#{Regexp.escape(name)}-80-[a-z0-9]+\z/),
          "Service #{name} NEG name is not exact")
  zones = array!(neg_status["zones"], "Service #{name} NEG zones")
  assert!(!zones.empty? && zones.uniq.length == zones.length &&
          zones.all? { |zone| zone.match?(/\Aasia-northeast1-[a-z]\z/) },
          "Service #{name} NEG zones are not exact")
  spec = object!(service["spec"], "Service #{name} spec")
  expected_spec_keys = %w[
    clusterIP clusterIPs internalTrafficPolicy ipFamilies ipFamilyPolicy ports selector
    sessionAffinity type
  ]
  assert!(spec.keys.sort == expected_spec_keys,
          "Service #{name} specification includes an unreviewed traffic field")
  cluster_ip = string!(spec["clusterIP"], "Service #{name} clusterIP")
  assert!(cluster_ip.match?(/\A(?:\d{1,3}\.){3}\d{1,3}\z/) && spec["clusterIPs"] == [cluster_ip],
          "Service #{name} cluster IP shape is not exact")
  assert!(spec["type"] == "ClusterIP" && spec["selector"] == expected_selector,
          "Service #{name} selector or type is not exact")
  assert!(spec["ports"] == [{ "name" => "http", "port" => 80, "protocol" => "TCP", "targetPort" => "http" }],
          "Service #{name} ports are not exact")
  assert!(spec["internalTrafficPolicy"] == "Cluster" && spec["ipFamilies"] == ["IPv4"] &&
          spec["ipFamilyPolicy"] == "SingleStack" && spec["sessionAffinity"] == "None" &&
          !spec.key?("externalTrafficPolicy") && !spec.key?("allocateLoadBalancerNodePorts") &&
          !spec.key?("loadBalancerClass"), "Service #{name} traffic-policy defaults are not exact")
  { "uid" => service.dig("metadata", "uid"), "name" => neg_name, "zones" => zones.sort }
rescue JSON::ParserError
  reject!("Service #{name} NEG annotations must be valid JSON")
end

def gateway_parent(section)
  {
    "group" => "gateway.networking.k8s.io", "kind" => "Gateway",
    "name" => "chronoai-gateway", "namespace" => "gateway-infra",
    "sectionName" => section,
  }
end

def service_backend(name)
  [{ "group" => "", "kind" => "Service", "name" => name, "port" => 80, "weight" => 1 }]
end

def validate_route_status(route, section)
  generation = route.dig("metadata", "generation")
  assert!(generation.is_a?(Integer) && generation.positive?, "HTTPRoute generation is not exact")
  parents = array!(route.dig("status", "parents"), "HTTPRoute parents").select do |parent|
    parent["controllerName"] == "networking.gke.io/gateway" &&
      parent["parentRef"] == gateway_parent(section)
  end
  assert!(parents.length == 1, "HTTPRoute has an ambiguous Gateway parent status")
  conditions = array!(parents.fetch(0)["conditions"], "HTTPRoute conditions")
  %w[Accepted Reconciled ResolvedRefs].each do |type|
    matches = conditions.select do |condition|
      condition["type"] == type && condition["status"] == "True" &&
        condition["observedGeneration"] == generation
    end
    assert!(matches.length == 1, "HTTPRoute lacks current #{type}")
  end
end

def route_attaches_to_shared_gateway?(route)
  route_namespace = route.dig("metadata", "namespace")
  array!(route.dig("spec", "parentRefs") || [], "HTTPRoute parentRefs").any? do |parent|
    parent = object!(parent, "HTTPRoute parentRef")
    group = parent.fetch("group", "gateway.networking.k8s.io")
    kind = parent.fetch("kind", "Gateway")
    namespace = parent.fetch("namespace", route_namespace)
    next false unless group == "gateway.networking.k8s.io" && kind == "Gateway" &&
                      namespace == "gateway-infra" && parent["name"] == "chronoai-gateway"

    section = parent["sectionName"]
    port = parent["port"]
    (section.nil? || %w[http https].include?(section)) &&
      (port.nil? || [80, 443].include?(port))
  end
end

def validate_owned_routes(routes, route_mode, health)
  owned_names = %w[cma-api cma-http-redirect cma-web]
  routes.each do |route|
    next if route.dig("metadata", "namespace") == "chrono-bot" &&
            owned_names.include?(route.dig("metadata", "name"))

    next unless route_attaches_to_shared_gateway?(route)

    hostnames = array!(route.dig("spec", "hostnames") || [], "attached route hostnames")
    assert!(!hostnames.empty?, "an unowned hostname-less HTTPRoute attaches to the shared Gateway")
    hostnames.each do |hostname|
      assert!(!CMA_HOSTS.include?(hostname) && !wildcard_overlaps_cma?(hostname),
              "an unowned attached HTTPRoute overlaps a CMA hostname")
    end
  end

  api = named(routes, "HTTPRoute", "chrono-bot", "cma-api")
  web = named(routes, "HTTPRoute", "chrono-bot", "cma-web")
  redirect = named(routes, "HTTPRoute", "chrono-bot", "cma-http-redirect")
  expected_api = {
    "parentRefs" => [gateway_parent("https")],
    "hostnames" => ["bot-api.chrono-ai.fun"],
    "rules" => [{
      "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
      "backendRefs" => service_backend("cma-api"),
    }],
  }
  legacy_rules = [{
    "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
    "backendRefs" => service_backend("cma-web"),
  }]
  adopted_rules = [
    {
      "matches" => [
        { "path" => { "type" => "PathPrefix", "value" => "/api" } },
        { "path" => { "type" => "PathPrefix", "value" => "/auth" } },
        { "path" => { "type" => "Exact", "value" => "/openapi.json" } },
        { "path" => { "type" => "Exact", "value" => "/.well-known/agent-card.json" } },
      ],
      "backendRefs" => service_backend("cma-api"),
    },
    {
      "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
      "backendRefs" => service_backend("cma-web"),
    },
  ]
  expected_web = {
    "parentRefs" => [gateway_parent("https")], "hostnames" => ["bot.chrono-ai.fun"],
    "rules" => route_mode == "legacy" ? legacy_rules : adopted_rules,
  }
  expected_redirect = {
    "parentRefs" => [gateway_parent("http")],
    "hostnames" => ["bot.chrono-ai.fun", "bot-api.chrono-ai.fun"],
    "rules" => [{
      "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
      "filters" => [{
        "type" => "RequestRedirect",
        "requestRedirect" => { "scheme" => "https", "statusCode" => 301 },
      }],
    }],
  }
  assert!(api["spec"] == expected_api, "cma-api HTTPRoute specification is not exact")
  assert!(web["spec"] == expected_web, "cma-web HTTPRoute specification is not exact for #{route_mode}")
  assert!(redirect["spec"] == expected_redirect, "CMA redirect HTTPRoute specification is not exact")
  validate_route_status(api, "https")
  validate_route_status(web, "https")
  validate_route_status(redirect, "http")

  expected_health_path = route_mode == "legacy" ? "/livez" : "/ready"
  assert!(health.dig("spec", "targetRef") == {
            "group" => "", "kind" => "Service", "name" => "cma-api",
          } && health.dig("spec", "default", "config") == {
            "type" => "HTTP",
            "httpHealthCheck" => { "port" => 8080, "requestPath" => expected_health_path },
          }, "HealthCheckPolicy does not match the stable #{route_mode} route mode")
end

def validate_policy_status(policy, label)
  generation = policy.dig("metadata", "generation")
  assert!(generation.is_a?(Integer) && generation.positive?, "#{label} generation is not exact")
  conditions = array!(policy.dig("status", "conditions"), "#{label} conditions")
  attached = conditions.select do |condition|
    condition["type"] == "Attached" && condition["status"] == "True" &&
      condition["reason"] == "Attached"
  end
  assert!(attached.length == 1, "#{label} is not Attached at its current generation")
  assert!(attached.fetch(0).keys.sort == %w[lastTransitionTime message reason status type] &&
          attached.fetch(0)["lastTransitionTime"].is_a?(String) &&
          attached.fetch(0)["message"].is_a?(String),
          "#{label} Attached condition shape is not exact")
end

def validate_health_policy(policy, route_mode, service)
  assert!(policy["apiVersion"] == "networking.gke.io/v1" &&
          policy["kind"] == "HealthCheckPolicy" &&
          policy.dig("metadata", "namespace") == "chrono-bot" &&
          policy.dig("metadata", "name") == "cma-api",
          "HealthCheckPolicy identity/type is not exact")
  identity = resource_identity(policy, "HealthCheckPolicy cma-api")
  assert!(policy.dig("metadata", "deletionTimestamp").nil?, "HealthCheckPolicy is being deleted")
  expected_path = route_mode == "legacy" ? "/livez" : "/ready"
  assert!(policy["spec"] == {
            "targetRef" => { "group" => "", "kind" => "Service", "name" => "cma-api" },
            "default" => { "config" => {
              "type" => "HTTP", "httpHealthCheck" => { "port" => 8080, "requestPath" => expected_path },
            } },
          }, "HealthCheckPolicy desired state is not exact for #{route_mode}")
  assert!(service.dig("metadata", "name") == policy.dig("spec", "targetRef", "name"),
          "HealthCheckPolicy target is not the captured Service root")
  validate_policy_status(policy, "HealthCheckPolicy cma-api")
  identity.merge("targetServiceUid" => string!(service.dig("metadata", "uid"), "cma-api Service UID"))
end

def validate_backend_policy(policy, service_name, service)
  assert!(policy["apiVersion"] == "networking.gke.io/v1" &&
          policy["kind"] == "GCPBackendPolicy" &&
          policy.dig("metadata", "namespace") == "chrono-bot" &&
          policy.dig("metadata", "name") == service_name,
          "GCPBackendPolicy #{service_name} identity/type is not exact")
  identity = resource_identity(policy, "GCPBackendPolicy #{service_name}")
  assert!(policy.dig("metadata", "deletionTimestamp").nil?,
          "GCPBackendPolicy #{service_name} is being deleted")
  assert!(policy["spec"] == {
            "targetRef" => { "group" => "", "kind" => "Service", "name" => service_name },
            "default" => { "timeoutSec" => 660 },
          }, "GCPBackendPolicy #{service_name} desired state is not exact")
  assert!(service.dig("metadata", "name") == policy.dig("spec", "targetRef", "name"),
          "GCPBackendPolicy #{service_name} target is not the captured Service root")
  validate_policy_status(policy, "GCPBackendPolicy #{service_name}")
  identity.merge("targetServiceUid" => string!(service.dig("metadata", "uid"), "#{service_name} Service UID"))
end

def validate_fence_admission(policy, binding)
  labels = { "app.kubernetes.io/name" => "cma-api", "app.kubernetes.io/part-of" => "cma" }
  resource_rules = [{
    "apiGroups" => [""], "apiVersions" => ["v1"],
    "operations" => %w[CREATE UPDATE], "resources" => ["pods"], "scope" => "Namespaced",
  }]
  namespace_selector = { "matchLabels" => { "kubernetes.io/metadata.name" => "chrono-bot" } }
  assert!(policy["apiVersion"] == "admissionregistration.k8s.io/v1" &&
          policy["kind"] == "ValidatingAdmissionPolicy" &&
          policy.dig("metadata", "name") == "cma-cutover-fence-label-reservation",
          "cutover-fence admission policy identity/type is not exact")
  policy_identity = resource_identity(policy, "cutover-fence admission policy")
  assert!(policy.dig("metadata", "labels") == labels &&
          policy.dig("metadata", "deletionTimestamp").nil?,
          "cutover-fence admission policy metadata is not exact")
  assert!(policy["spec"] == {
            "failurePolicy" => "Fail",
            "matchConstraints" => {
              "matchPolicy" => "Equivalent", "namespaceSelector" => namespace_selector,
              "resourceRules" => resource_rules,
            },
            "matchConditions" => [{
              "name" => "exact-chrono-bot-namespace",
              "expression" => "request.namespace == 'chrono-bot'",
            }],
            "validations" => [{
              "expression" => "!has(object.metadata.labels) || !('release.chrono-ai.fun/cutover-fence' in object.metadata.labels) || object.metadata.labels['release.chrono-ai.fun/cutover-fence'] != 'mongodb-715'",
              "message" => "Pods in chrono-bot must not use the reserved CMA cutover-fence label value mongodb-715",
              "reason" => "Forbidden",
            }],
          }, "cutover-fence admission policy specification is not exact")
  generation = policy.dig("metadata", "generation")
  assert!(generation.is_a?(Integer) && generation.positive? &&
          policy.dig("status", "observedGeneration") == generation &&
          array!(policy.dig("status", "typeChecking", "expressionWarnings") || [],
                 "cutover-fence admission expression warnings").empty?,
          "cutover-fence admission policy is not observed without type warnings")

  assert!(binding["apiVersion"] == "admissionregistration.k8s.io/v1" &&
          binding["kind"] == "ValidatingAdmissionPolicyBinding" &&
          binding.dig("metadata", "name") == "cma-cutover-fence-label-reservation",
          "cutover-fence admission binding identity/type is not exact")
  binding_identity = resource_identity(binding, "cutover-fence admission binding")
  assert!(binding.dig("metadata", "labels") == labels &&
          binding.dig("metadata", "deletionTimestamp").nil?,
          "cutover-fence admission binding metadata is not exact")
  assert!(binding["spec"] == {
            "policyName" => "cma-cutover-fence-label-reservation",
            "validationActions" => ["Deny"],
            "matchResources" => {
              "matchPolicy" => "Equivalent", "namespaceSelector" => namespace_selector,
              "resourceRules" => resource_rules,
            },
          }, "cutover-fence admission binding specification is not exact")
  {
    "policy" => policy_identity.merge("generation" => generation),
    "binding" => binding_identity.merge(
      "generation" => binding.dig("metadata", "generation"),
      "policyUid" => policy_identity.fetch("uid"),
    ),
  }
end

def service_endpoint_projection(endpoint_slices, pods, nodes, service, name)
  service_uid = service.dig("metadata", "uid")
  slices = list_items(endpoint_slices, "#{name} EndpointSlices")
  pods_by_uid = list_items(pods, "#{name} Pods").each_with_object({}) do |pod, result|
    uid = string!(pod.dig("metadata", "uid"), "#{name} Pod UID")
    assert!(!result.key?(uid), "#{name} Pod list contains a duplicate UID")
    result[uid] = pod
  end
  nodes_by_name = list_items(nodes, "Nodes").each_with_object({}) do |node, result|
    node_name = string!(node.dig("metadata", "name"), "Node name")
    result[node_name] = node
  end
  ready = []
  slices.each do |slice|
    assert!(slice["apiVersion"] == "discovery.k8s.io/v1" && slice["kind"] == "EndpointSlice" &&
            slice["addressType"] == "IPv4", "#{name} EndpointSlice type is not exact")
    owners = array!(slice.dig("metadata", "ownerReferences"), "#{name} EndpointSlice owners")
    assert!(owners.length == 1 && owners.fetch(0)["kind"] == "Service" &&
            owners.fetch(0)["name"] == name && owners.fetch(0)["uid"] == service_uid &&
            owners.fetch(0)["controller"] == true, "#{name} EndpointSlice owner is not the Service UID")
    assert!(slice["ports"] == [{ "name" => "http", "port" => 8080, "protocol" => "TCP" }],
            "#{name} EndpointSlice ports are not exact")
    array!(slice["endpoints"], "#{name} EndpointSlice endpoints").each do |endpoint|
      conditions = object!(endpoint["conditions"], "#{name} endpoint conditions")
      next if conditions == { "ready" => false, "serving" => false, "terminating" => false }
      assert!(conditions == { "ready" => true, "serving" => true, "terminating" => false },
              "#{name} serving endpoint conditions are not exact")

      addresses = array!(endpoint["addresses"], "#{name} endpoint addresses")
      assert!(addresses.length == 1, "#{name} ready endpoint must have one address")
      target = object!(endpoint["targetRef"], "#{name} endpoint targetRef")
      assert!(target["kind"] == "Pod" && target["namespace"] == "chrono-bot",
              "#{name} endpoint target must be a chrono-bot Pod")
      pod = pods_by_uid[target["uid"]]
      assert!(pod && pod.dig("metadata", "name") == target["name"],
              "#{name} endpoint target Pod UID/name is not exact")
      assert!(pod.dig("status", "podIP") == addresses.fetch(0) &&
              pod.dig("metadata", "deletionTimestamp").nil?,
              "#{name} endpoint address does not bind the live Pod")
      node_name = string!(pod.dig("spec", "nodeName"), "#{name} Pod nodeName")
      assert!(endpoint["nodeName"] == node_name,
              "#{name} EndpointSlice nodeName does not bind its Pod")
      node = nodes_by_name[node_name]
      assert!(node, "#{name} Pod node is absent from the capture")
      provider_id = string!(node.dig("spec", "providerID"), "#{name} node providerID")
      match = provider_id.match(%r{\Agce://#{Regexp.escape(PROJECT_ID)}/([^/]+)/([^/]+)\z})
      assert!(match, "#{name} node providerID is not the exact GCE identity")
      assert!(endpoint["zone"] == match[1],
              "#{name} EndpointSlice zone does not bind the provider zone")
      ready_conditions = array!(pod.dig("status", "conditions"), "#{name} Pod conditions").select do |condition|
        condition["type"] == "Ready" && condition["status"] == "True"
      end
      assert!(pod.dig("status", "phase") == "Running" && ready_conditions.length == 1,
              "#{name} endpoint Pod is not currently Ready and Running")
      expected_container = name == "cma-api" ? "api" : "web"
      container_statuses = array!(pod.dig("status", "containerStatuses"), "#{name} container statuses")
      assert!(container_statuses.length == 1 && container_statuses.fetch(0)["name"] == expected_container,
              "#{name} endpoint Pod container set is not exact")
      container = container_statuses.fetch(0)
      assert!(container["ready"] == true && container["started"] == true &&
              container.dig("state", "running", "startedAt").is_a?(String) &&
              container.dig("state", "waiting").nil? && container.dig("state", "terminated").nil? &&
              container["restartCount"].is_a?(Integer) && container["restartCount"] >= 0,
              "#{name} endpoint container is not in the exact serving state")
      ready << {
        "ipAddress" => addresses.fetch(0), "port" => 8080,
        "podUid" => target["uid"], "podName" => target["name"],
        "nodeName" => node_name, "providerZone" => match[1], "providerInstance" => match[2],
        "containerStartedAt" => container.dig("state", "running", "startedAt"),
        "containerRestartCount" => container["restartCount"],
      }
    end
  end
  ready.sort_by { |entry| [entry["ipAddress"], entry["podUid"]] }
end

def exact_ports(forwarding_rule)
  return forwarding_rule["ports"].map(&:to_s).sort if forwarding_rule["ports"].is_a?(Array)
  return [forwarding_rule["portRange"].split("-").first] if forwarding_rule["portRange"].is_a?(String) &&
                                                           forwarding_rule["portRange"].split("-").uniq.length == 1

  []
end

def referenced_backend_links(value, result = [])
  case value
  when Hash
    value.each do |key, child|
      if %w[backendService defaultService service].include?(key) && child.is_a?(String) &&
         child.include?("/backendServices/")
        result << compute_ref(child)
      else
        referenced_backend_links(child, result)
      end
    end
  when Array
    value.each { |child| referenced_backend_links(child, result) }
  end
  result
end

def wildcard_overlaps_cma?(pattern)
  return true if pattern == "*"
  return false unless pattern.start_with?("*.")

  suffix = pattern.delete_prefix("*")
  CMA_HOSTS.any? { |host| host.end_with?(suffix) }
end

def path_pattern_matches?(pattern, path)
  if pattern.end_with?("/*")
    base = pattern.delete_suffix("/*")
    path == base || path.start_with?("#{base}/")
  else
    path == pattern
  end
end

def matcher_backend_for_path(matcher, path)
  path_rules = matcher["pathRules"] || []
  route_rules = matcher["routeRules"] || []
  assert!(path_rules.empty? || route_rules.empty?, "URL matcher mixes pathRules and routeRules")
  unless route_rules.empty?
    priorities = route_rules.map { |rule| rule["priority"] }
    assert!(priorities.all? { |priority| priority.is_a?(Integer) } &&
            priorities.uniq.length == priorities.length,
            "URL routeRules priorities must be unique integers")
    route_rules.sort_by { |rule| rule.fetch("priority") }.each do |rule|
      matches = array!(rule["matchRules"], "URL routeRule matchRules")
      matched = matches.any? do |match|
        allowed = match.keys & %w[fullPathMatch prefixMatch]
        assert!(allowed.length == 1 && (match.keys - allowed).empty?,
                "URL routeRule uses an unsupported match dimension")
        match["fullPathMatch"] == path ||
          (match["prefixMatch"].is_a?(String) && path.start_with?(match["prefixMatch"]))
      end
      return compute_ref(rule["service"]) if matched
    end
  end
  unless path_rules.empty?
    candidates = path_rules.flat_map do |rule|
      array!(rule["paths"], "URL pathRule paths").map do |pattern|
        [pattern.length, compute_ref(rule["service"])] if path_pattern_matches?(pattern, path)
      end.compact
    end
    return candidates.max_by(&:first).fetch(1) unless candidates.empty?
  end
  compute_ref(matcher["defaultService"])
end

def validate_serving_matcher(matcher, host, route_mode, api_backend, web_backend)
  assert!(!matcher.key?("defaultRouteAction") && !matcher.key?("defaultUrlRedirect"),
          "serving URL matcher has an alternate default action")
  if host == "bot-api.chrono-ai.fun"
    assert!(compute_ref(matcher["defaultService"]) == api_backend,
            "direct API hostname default does not target cma-api")
    referenced_backend_links(matcher).each do |link|
      assert!(link == api_backend, "direct API hostname contains a non-API backend")
    end
    %w[/ /api /auth /openapi.json /.well-known/agent-card.json /other].each do |path|
      assert!(matcher_backend_for_path(matcher, path) == api_backend,
              "direct API hostname path #{path} does not resolve to cma-api")
    end
    return
  end

  assert!(compute_ref(matcher["defaultService"]) == web_backend,
          "app hostname default does not target cma-web")
  if route_mode == "legacy"
    referenced_backend_links(matcher).each do |link|
      assert!(link == web_backend, "legacy app hostname contains a direct API backend")
    end
    %w[/ /api /api/x /auth/x /openapi.json /.well-known/agent-card.json /other].each do |path|
      assert!(matcher_backend_for_path(matcher, path) == web_backend,
              "legacy app hostname path #{path} does not resolve to cma-web")
    end
    return
  end

  allowed_api_patterns = %w[
    /.well-known/agent-card.json /api /api/* /auth /auth/* /openapi.json
  ]
  array!(matcher["pathRules"] || [], "app URL pathRules").each do |rule|
    assert!(compute_ref(rule["service"]) == api_backend,
            "app pathRule does not target cma-api")
    array!(rule["paths"], "app pathRule paths").each do |pattern|
      assert!(allowed_api_patterns.include?(pattern),
              "app pathRule broadens the approved direct-API path set")
    end
  end
  array!(matcher["routeRules"] || [], "app URL routeRules").each do |rule|
    assert!(compute_ref(rule["service"]) == api_backend,
            "app routeRule does not target cma-api")
    array!(rule["matchRules"], "app routeRule matches").each do |match|
      value = match["prefixMatch"] || match["fullPathMatch"]
      allowed = match.key?("prefixMatch") ? %w[/api /auth] :
        %w[/openapi.json /.well-known/agent-card.json]
      assert!(allowed.include?(value), "app routeRule broadens the approved direct-API path set")
    end
  end
  {
    "/api" => api_backend, "/api/" => api_backend, "/api/x" => api_backend,
    "/auth" => api_backend, "/auth/x" => api_backend,
    "/openapi.json" => api_backend,
    "/.well-known/agent-card.json" => api_backend,
    "/" => web_backend, "/other" => web_backend, "/apiary" => web_backend,
    "/authentication" => web_backend, "/openapi.json/extra" => web_backend,
  }.each do |path, expected|
    assert!(matcher_backend_for_path(matcher, path) == expected,
            "adopted app hostname path #{path} resolves to the wrong backend")
  end
end

def validate_url_maps(url_maps, http_proxy, https_proxy, backend_refs, route_mode,
                      api_backend, web_backend)
  maps = indexed(url_maps, "URL maps")
  expected_links = [compute_ref(http_proxy["urlMap"]), compute_ref(https_proxy["urlMap"])].uniq.sort
  assert!(maps.keys.sort == expected_links, "captured URL maps are not exactly the proxy targets")
  http_map = maps.fetch(compute_ref(http_proxy["urlMap"]))
  https_map = maps.fetch(compute_ref(https_proxy["urlMap"]))

  expected_redirect = {
    "httpsRedirect" => true,
    "redirectResponseCode" => "MOVED_PERMANENTLY_DEFAULT",
    "stripQuery" => false,
  }
  http_redirect = http_map["defaultUrlRedirect"]
  assert!(http_redirect == expected_redirect && !http_map.key?("defaultService") &&
          !http_map.key?("defaultRouteAction") &&
          array!(http_map["hostRules"] || [], "HTTP redirect hostRules").empty? &&
          array!(http_map["pathMatchers"] || [], "HTTP redirect pathMatchers").empty?,
          "HTTP URL map is not a closed permanent HTTPS redirect")

  host_rules = array!(https_map["hostRules"], "HTTPS URL map hostRules")
  matchers = array!(https_map["pathMatchers"], "HTTPS URL map pathMatchers")
  CMA_HOSTS.each do |host|
    matches = host_rules.select { |rule| array!(rule["hosts"], "URL map host list").include?(host) }
    assert!(matches.length == 1, "HTTPS URL map must bind #{host} exactly once")
    matcher_name = string!(matches.fetch(0)["pathMatcher"], "#{host} pathMatcher")
    assert!(matchers.count { |matcher| matcher["name"] == matcher_name } == 1,
            "HTTPS URL map pathMatcher for #{host} is not exact")
    matcher = matchers.find { |candidate| candidate["name"] == matcher_name }
    validate_serving_matcher(matcher, host, route_mode, api_backend, web_backend)
  end
  host_rules.each do |rule|
    array!(rule["hosts"], "URL map host list").each do |host_pattern|
      assert!(!wildcard_overlaps_cma?(host_pattern), "HTTPS URL map contains a wildcard overlapping CMA")
    end
  end

  prohibited = %w[
    corsPolicy faultInjectionPolicy headerAction requestMirrorPolicy retryPolicy
    urlRewrite weightedBackendServices
  ]
  walk = lambda do |value|
    case value
    when Hash
      prohibited.each do |key|
        assert!(!value.key?(key), "HTTPS URL map contains prohibited CMA-affecting action #{key}")
      end
      value.each_value { |child| walk.call(child) }
    when Array
      value.each { |child| walk.call(child) }
    end
  end
  walk.call(https_map)
  linked = referenced_backend_links(https_map).uniq.sort
  assert!((linked - backend_refs).empty?, "HTTPS URL map references an uncaptured backend")
  linked
end

def validate_backend_health(compute, backends, negs, service_name, backend_link,
                            effective_members, service_mode)
  receipts = array!(compute["backendHealth"], "backend health receipts")
  matching = receipts.select { |entry| compute_ref(entry["backendService"]) == backend_link }
  assert!(matching.length == 1, "#{service_name} backend health receipt is not exact")
  groups = array!(matching.fetch(0)["groups"], "#{service_name} backend group health")
  backend_groups = array!(backends.fetch(backend_link)["backends"], "#{service_name} backend groups")
    .map { |group| compute_ref(group.fetch("group")) }.sort
  groups_by_link = groups.each_with_object({}) do |group, result|
    link = compute_ref(group["backend"])
    assert!(!result.key?(link), "#{service_name} has duplicate health for #{link}")
    result[link] = object!(group["status"], "#{service_name} backend group health status")
  end
  assert!(groups_by_link.keys.sort == backend_groups,
          "#{service_name} health receipt does not cover every effective backend group")

  observed = groups_by_link.flat_map do |group_link, status|
    assert!(status.keys.all? { |key| %w[healthStatus kind].include?(key) },
            "#{service_name} backend health status shape is open")
    array!(status["healthStatus"] || [], "#{service_name} endpoint health").map do |health|
      assert!(health["healthState"] == "HEALTHY",
              "#{service_name} has an unhealthy or unknown load-balancer endpoint")
      ip = string!(health["ipAddress"], "#{service_name} healthy endpoint IP")
      port = health["port"]
      member = effective_members.find do |candidate|
        candidate["group"] == group_link && candidate.dig("member", "ipAddress") == ip &&
          candidate.dig("member", "port") == port
      end
      assert!(member, "#{service_name} health is not bound to an exact NEG/Pod member")
      if health.key?("instance")
        instance = string!(health["instance"], "#{service_name} healthy endpoint instance").split("/").last
        assert!(instance == member.dig("podBinding", "providerInstance"),
                "#{service_name} health instance differs from the Pod provider identity")
      end
      { "group" => group_link, "health" => health, "member" => member }
    end
  end
  expected_pairs = effective_members.map do |entry|
    [entry["group"], entry.dig("member", "ipAddress"), entry.dig("member", "port")]
  end.sort
  observed_pairs = observed.map do |entry|
    [entry["group"], entry.dig("health", "ipAddress"), entry.dig("health", "port")]
  end.sort
  assert!(observed_pairs == expected_pairs,
          "#{service_name} load-balancer health does not equal complete NEG membership")
  if service_mode == "fenced" && service_name == "cma-api"
    assert!(observed.empty?, "fenced cma-api still has routable load-balancer health")
  end
  observed
end

def validate_backend_graph(compute, gateway, services, endpoint_projections, service_mode)
  backends = indexed(compute["backendServices"], "backend services")
  annotated_backends = annotation_refs(gateway, "networking.gke.io/backend-services")
  assert!(backends.keys.sort == annotated_backends, "backend capture does not equal the Gateway annotation")
  health_checks = indexed(compute["healthChecks"], "health checks")
  annotated_health = annotation_refs(gateway, "networking.gke.io/health-checks")
  assert!(health_checks.keys.sort == annotated_health, "health-check capture does not equal the Gateway annotation")
  neg_entries = array!(compute["networkEndpointGroups"], "network endpoint groups")
  negs = neg_entries.each_with_object({}) do |entry, result|
    resource = object!(entry["resource"], "NEG resource")
    link = compute_ref(resource["selfLink"])
    assert!(!result.key?(link), "duplicate captured NEG #{link}")
    endpoints = array!(entry["endpoints"], "NEG endpoints")
    result[link] = { "resource" => resource, "endpoints" => endpoints }
  end

  referenced_health = []
  referenced_groups = []
  backends.each_value do |backend|
    assert!(backend["loadBalancingScheme"] == "EXTERNAL_MANAGED" && backend["protocol"] == "HTTP",
            "Gateway backend protocol or scheme is not exact")
    referenced_health.concat(array!(backend["healthChecks"], "backend healthChecks").map { |ref| compute_ref(ref) })
    backend_groups = array!(backend["backends"], "backend groups")
    assert!(!backend_groups.empty?, "Gateway backend has no effective group")
    backend_groups.each do |group|
      referenced_groups << compute_ref(string!(group["group"], "backend group reference"))
    end
  end
  assert!(referenced_health.uniq.sort == health_checks.keys.sort,
          "health-check capture is not exactly the complete backend reference set")
  assert!(referenced_groups.uniq.sort == negs.keys.sort,
          "NEG capture is not exactly the complete backend group set")

  CMA_SERVICES.each do |service_name|
    marker = "-chrono-bot-#{service_name}-80-"
    backend_matches = backends.select { |link, _body| link.include?(marker) }
    assert!(backend_matches.length == 1, "#{service_name} effective backend is ambiguous")
    backend_link, backend = backend_matches.first
    assert!(backend["timeoutSec"] == 660, "#{service_name} effective timeout is not 660")
    root = services.fetch(service_name)
    expected_groups = root.fetch("zones").map do |zone|
      "#{API_ROOT}/projects/#{PROJECT_NUMBER}/zones/#{zone}/networkEndpointGroups/#{root.fetch('name')}"
    end.sort
    actual_groups = backend.fetch("backends").map { |group| compute_ref(group.fetch("group")) }.sort
    assert!(actual_groups == expected_groups, "#{service_name} backend is not bound to the complete Service NEG set")

    effective = actual_groups.flat_map do |group_link|
      neg = negs.fetch(group_link)
      resource = neg.fetch("resource")
      zone = string!(resource["zone"], "NEG zone").split("/").last
      assert!(resource["name"] == root.fetch("name") && resource["networkEndpointType"] == "GCE_VM_IP_PORT",
              "#{service_name} NEG resource identity/type is not exact")
      assert!(zone == group_link.split("/zones/").fetch(1).split("/").fetch(0),
              "#{service_name} NEG zone does not match its backend group")
      neg.fetch("endpoints").map do |endpoint|
        ip = string!(endpoint["ipAddress"], "NEG endpoint IP")
        port = endpoint["port"]
        match = endpoint_projections.fetch(service_name).find do |candidate|
          candidate["ipAddress"] == ip && candidate["port"] == port
        end
        assert!(match, "#{service_name} NEG endpoint is not a ready Service Pod projection")
        if endpoint.key?("instance")
          instance = string!(endpoint["instance"], "NEG instance").split("/").last
          assert!(instance == match.fetch("providerInstance") && zone == match.fetch("providerZone"),
                  "#{service_name} NEG instance/zone is not the Pod node provider identity")
        end
        { "group" => group_link, "member" => endpoint, "podBinding" => match }
      end
    end
    expected = endpoint_projections.fetch(service_name)
    effective_pairs = effective.map { |entry| [entry.dig("member", "ipAddress"), entry.dig("member", "port")] }.sort
    expected_pairs = expected.map { |entry| [entry["ipAddress"], entry["port"]] }.sort
    assert!(effective_pairs == expected_pairs, "#{service_name} complete NEG membership differs from EndpointSlices")
    root["backendSelfLink"] = backend_link
    root["backendFingerprint"] = string!(backend["fingerprint"], "#{service_name} backend fingerprint")
    root["backendGroups"] = actual_groups
    root["effectiveMembers"] = effective
    root["effectiveHealth"] = validate_backend_health(
      compute, backends, negs, service_name, backend_link, effective, service_mode,
    )
  end
  [backends, health_checks, negs]
end

def validate_compute(state, gateway, gateway_ip, services, endpoint_projections,
                     certificate_secret, service_mode)
  compute = object!(state["compute"], "compute capture")
  expected_keys = %w[
    address backendHealth backendServices firewalls forwardingRules healthChecks
    networkEndpointGroups sslCertificates sslPolicies targetHttpProxies targetHttpsProxies urlMaps
  ]
  assert!(compute.keys.sort == expected_keys, "Compute capture key set is not closed")
  address = object!(compute["address"], "global address")
  assert!(address["name"] == "chronoai-gateway-external-ip" && address["address"] == gateway_ip &&
          address["addressType"] == "EXTERNAL" && address["ipVersion"] == "IPV4" &&
          address["status"] == "IN_USE", "global address does not equal the Gateway status IP")
  string!(address["fingerprint"], "global address fingerprint") if address.key?("fingerprint")

  firewalls = indexed(compute["firewalls"], "Gateway firewalls")
  assert!(!firewalls.empty? &&
          firewalls.keys.sort == annotation_refs(gateway, "networking.gke.io/firewalls"),
          "firewall capture does not equal the complete Gateway annotation")

  http_proxies = indexed(compute["targetHttpProxies"], "target HTTP proxies")
  https_proxies = indexed(compute["targetHttpsProxies"], "target HTTPS proxies")
  assert!(http_proxies.keys.sort == annotation_refs(gateway, "networking.gke.io/target-http-proxies"),
          "HTTP proxy capture does not equal the Gateway annotation")
  assert!(https_proxies.keys.sort == annotation_refs(gateway, "networking.gke.io/target-https-proxies"),
          "HTTPS proxy capture does not equal the Gateway annotation")
  assert!(http_proxies.length == 1 && https_proxies.length == 1,
          "Gateway must have exactly one HTTP and one HTTPS proxy")
  http_proxy = http_proxies.values.fetch(0)
  https_proxy = https_proxies.values.fetch(0)
  assert!(!http_proxy.key?("proxyHeader") || http_proxy["proxyHeader"] == "NONE",
          "HTTP proxy header mode is not closed")
  assert!(!https_proxy.key?("proxyHeader") || https_proxy["proxyHeader"] == "NONE",
          "HTTPS proxy header mode is not closed")
  assert!(!https_proxy.key?("quicOverride") || %w[NONE ENABLE DISABLE].include?(https_proxy["quicOverride"]),
          "HTTPS QUIC override is not a closed value")
  assert!(!https_proxy.key?("certificateMap"),
          "Certificate Manager chains require a separately reviewed closed capture")

  certificates = indexed(compute["sslCertificates"], "SSL certificates")
  expected_certificates = array!(https_proxy["sslCertificates"], "HTTPS proxy SSL certificates").map do |ref|
    compute_ref(ref)
  end.sort
  assert!(certificates.keys.sort == expected_certificates &&
          certificates.keys.sort == annotation_refs(gateway, "networking.gke.io/ssl-certificates"),
          "SSL certificate capture is not exactly the HTTPS proxy/Gateway set")
  assert!(certificates.length == 1, "Gateway TLS must have exactly one captured Compute certificate")
  certificates.each_value do |certificate|
    assert!(certificate["type"] == "SELF_MANAGED" && certificate["certificate"].is_a?(String),
            "Gateway TLS certificate is not a Secret-derived self-managed certificate")
    compute_chain = pem_certificate_identities(certificate["certificate"], "Compute SSL certificate")
    assert!(compute_chain == certificate_secret.fetch("certificateChain"),
            "Gateway certificate Secret does not equal the active Compute certificate chain")
  end
  ssl_policies = indexed(compute["sslPolicies"], "SSL policies")
  expected_ssl = https_proxy["sslPolicy"] ? [compute_ref(https_proxy["sslPolicy"])] : []
  assert!(ssl_policies.keys.sort == expected_ssl.sort, "SSL-policy capture is not exactly the proxy reference")

  forwarding_rules = indexed(compute["forwardingRules"], "forwarding rules")
  assert!(forwarding_rules.keys.sort == annotation_refs(gateway, "networking.gke.io/forwarding-rules"),
          "forwarding-rule capture does not equal the Gateway annotation")
  assert!(forwarding_rules.length == 2, "Gateway must expose exactly two forwarding rules")
  targets = http_proxies.keys + https_proxies.keys
  observed_ports = forwarding_rules.values.map do |rule|
    assert!(rule["IPAddress"] == gateway_ip && rule["IPProtocol"] == "TCP" &&
            rule["loadBalancingScheme"] == "EXTERNAL_MANAGED" && rule["networkTier"] == "PREMIUM",
            "forwarding rule address/protocol/scheme/tier is not exact")
    target = compute_ref(rule["target"])
    assert!(targets.include?(target), "forwarding rule target is not a captured proxy")
    ports = exact_ports(rule)
    assert!(ports.length == 1 && %w[80 443].include?(ports.fetch(0)),
            "forwarding rule is not exact TCP port 80 or 443")
    expected_target = ports.fetch(0) == "80" ? http_proxies.keys.fetch(0) : https_proxies.keys.fetch(0)
    assert!(target == expected_target, "forwarding rule port terminates at the wrong proxy")
    ports.fetch(0)
  end
  assert!(observed_ports.sort == %w[443 80].sort, "forwarding rules do not close TCP 80 and 443")

  backends, health_checks, negs = validate_backend_graph(
    compute, gateway, services, endpoint_projections, service_mode,
  )
  api_backend = services.fetch("cma-api").fetch("backendSelfLink")
  web_backend = services.fetch("cma-web").fetch("backendSelfLink")
  linked = validate_url_maps(
    compute["urlMaps"], http_proxy, https_proxy, backends.keys.sort,
    state.fetch("routeMode"), api_backend, web_backend,
  )
  assert!(linked.include?(api_backend) && linked.include?(web_backend),
          "serving URL map does not reference both CMA backends")
  {
    "address" => address,
    "forwardingRules" => forwarding_rules,
    "httpProxy" => http_proxy,
    "httpsProxy" => https_proxy,
    "backendServices" => backends,
    "healthChecks" => health_checks,
    "networkEndpointGroups" => negs,
    "backendHealth" => compute["backendHealth"],
    "certificateSecret" => certificate_secret,
    "firewalls" => firewalls,
  }
end

def validate_state(state, service_mode)
  state = object!(state, "captured state")
  assert!(state.keys.sort == %w[compute kubernetes routeMode], "captured state key set is not closed")
  assert!(%w[legacy adopted].include?(state["routeMode"]), "captured routeMode is not exact")
  kubernetes = object!(state["kubernetes"], "Kubernetes capture")
  expected_keys = %w[
    admissionBinding admissionPolicy backendPolicies certificateSecret endpointSlices gateway
    healthPolicy nodes pods routes services
  ]
  assert!(kubernetes.keys.sort == expected_keys, "Kubernetes capture key set is not closed")
  gateway = object!(kubernetes["gateway"], "Gateway")
  gateway_identity, gateway_ip = validate_gateway(gateway)

  routes = list_items(kubernetes["routes"], "HTTPRoutes")
  %w[cma-api cma-web cma-http-redirect].each do |route_name|
    route = named(routes, "HTTPRoute", "chrono-bot", route_name)
    resource_identity(route, "HTTPRoute #{route_name}")
  end
  service_items = list_items(kubernetes["services"], "Services")
  service_objects = CMA_SERVICES.to_h { |name| [name, named(service_items, "Service", "chrono-bot", name)] }
  selectors = {
    "cma-api" => if service_mode == "source"
                   { "app.kubernetes.io/name" => "cma-api" }
                 else
                   { "release.chrono-ai.fun/cutover-fence" => "mongodb-715" }
                 end,
    "cma-web" => { "app.kubernetes.io/name" => "cma-web" },
  }
  service_roots = CMA_SERVICES.to_h do |name|
    [name, validate_service(service_objects.fetch(name), name, selectors.fetch(name))]
  end
  health = object!(kubernetes["healthPolicy"], "HealthCheckPolicy")
  validate_owned_routes(routes, state["routeMode"], health)
  policy_roots = {
    "healthCheckPolicy" => validate_health_policy(
      health, state["routeMode"], service_objects.fetch("cma-api"),
    ),
  }
  policies = list_items(kubernetes["backendPolicies"], "GCPBackendPolicies")
  policy_roots["backendPolicies"] = CMA_SERVICES.to_h do |name|
    policy = named(policies, "GCPBackendPolicy", "chrono-bot", name)
    [name, validate_backend_policy(policy, name, service_objects.fetch(name))]
  end
  certificate_secret = certificate_secret_projection(
    object!(kubernetes["certificateSecret"], "Gateway certificate Secret"),
  )
  admission_root = validate_fence_admission(
    object!(kubernetes["admissionPolicy"], "cutover-fence admission policy"),
    object!(kubernetes["admissionBinding"], "cutover-fence admission binding"),
  )
  endpoint_projections = CMA_SERVICES.to_h do |name|
    slices = object!(kubernetes.fetch("endpointSlices").fetch(name), "#{name} EndpointSlices")
    pods = object!(kubernetes.fetch("pods").fetch(name), "#{name} Pods")
    [name, service_endpoint_projection(
      slices, pods, kubernetes["nodes"], service_objects.fetch(name), name,
    )]
  end
  compute_projection = validate_compute(
    state, gateway, gateway_ip, service_roots, endpoint_projections,
    certificate_secret, service_mode,
  )
  {
    "gatewayIdentity" => gateway_identity,
    "gatewayIp" => gateway_ip,
    "services" => service_roots,
    "policies" => policy_roots,
    "fenceAdmission" => admission_root,
    "compute" => compute_projection,
  }
end

def validate_capture(capture, expected_behavior = nil, policy_metadata = nil)
  capture = object!(capture, "capture")
  assert!(capture.keys.sort == %w[admissionReceipt first lifecycle mode schemaVersion second],
          "capture key set is not closed")
  assert!(capture["schemaVersion"] == 1, "capture schemaVersion is not supported")
  mode = capture["mode"]
  assert!(%w[source fenced].include?(mode), "capture mode must be source or fenced")
  lifecycle = object!(capture["lifecycle"], "projection lifecycle binding")
  assert!(lifecycle.keys.sort == %w[predecessorSha256 stage],
          "projection lifecycle binding shape is not closed")
  allowed_stages = %w[
    adopted fenced-post-scale fenced-pre-reopen fenced-pre-scale final-reopen source
  ]
  assert!(allowed_stages.include?(lifecycle["stage"]), "projection lifecycle stage is invalid")
  assert!((lifecycle["stage"] == "source" && lifecycle["predecessorSha256"] == "genesis") ||
          (lifecycle["stage"] != "source" &&
           lifecycle["predecessorSha256"].is_a?(String) &&
           lifecycle["predecessorSha256"].match?(/\A[0-9a-f]{64}\z/)),
          "projection lifecycle predecessor is invalid")
  assert!((mode == "fenced") == lifecycle["stage"].start_with?("fenced-"),
          "projection mode and lifecycle stage disagree")
  unless expected_behavior
    policy_metadata = load_live_policy
    expected_behavior = policy_metadata.fetch("expected")
  end
  policy_metadata ||= {
    "policySha256" => Digest::SHA256.hexdigest(canonical_json(expected_behavior)),
    "sourceEvidenceSha256" => Digest::SHA256.hexdigest(canonical_json(capture)),
  }
  first_behavior = compute_behavior_policy(capture["first"])
  second_behavior = compute_behavior_policy(capture["second"])
  assert!(canonical_json(first_behavior) == canonical_json(expected_behavior),
          "first live read differs from the approved behavior policy")
  assert!(canonical_json(second_behavior) == canonical_json(expected_behavior),
          "second live read differs from the approved behavior policy")
  first_projection = validate_state(capture["first"], mode)
  second_projection = validate_state(capture["second"], mode)
  admission_receipt = object!(capture["admissionReceipt"], "admission bootstrap receipt binding")
  assert!(admission_receipt.keys.sort == %w[
    binding manifestSha256 policy receiptSha256 releaseRevision sourceSetSha256
  ], "admission bootstrap receipt binding shape is not closed")
  %w[receiptSha256 manifestSha256 sourceSetSha256].each do |key|
    assert!(admission_receipt[key].is_a?(String) &&
            admission_receipt[key].match?(/\A[0-9a-f]{64}\z/),
            "admission bootstrap #{key} is malformed")
  end
  assert!(admission_receipt["releaseRevision"].is_a?(String) &&
          admission_receipt["releaseRevision"].match?(/\A[0-9a-f]{40}\z/),
          "admission bootstrap release revision is malformed")
  %w[policy binding].each do |kind|
    identity = object!(admission_receipt[kind], "admission bootstrap #{kind} identity")
    assert!(identity.keys.sort == %w[generation resourceVersion uid] &&
            identity["uid"].is_a?(String) && !identity["uid"].empty? &&
            identity["generation"].is_a?(Integer) && identity["generation"].positive? &&
            identity["resourceVersion"].is_a?(String) && !identity["resourceVersion"].empty?,
            "admission bootstrap #{kind} identity is malformed")
    %w[first second].each do |read|
      live = capture.dig(read, "kubernetes", "admission#{kind.capitalize}", "metadata")
      assert!(live && live["uid"] == identity["uid"] &&
              live["generation"] == identity["generation"] &&
              live["resourceVersion"] == identity["resourceVersion"],
              "#{read} admission #{kind} differs from the bootstrap receipt")
    end
  end
  assert!(canonical_json(capture["first"]) == canonical_json(capture["second"]),
          "the two live reads are not byte-for-byte equivalent after canonical JSON sorting")
  assert!(canonical_json(first_projection) == canonical_json(second_projection),
          "the two derived routing graphs are not identical")
  %w[policySha256 sourceEvidenceSha256].each do |digest_key|
    digest = policy_metadata[digest_key]
    assert!(digest.is_a?(String) && digest.match?(/\A[0-9a-f]{64}\z/),
            "behavior policy #{digest_key} is malformed")
  end
  {
    "schemaVersion" => 1,
    "mode" => mode,
    "lifecycle" => canonical(lifecycle),
    "captureSha256" => Digest::SHA256.hexdigest(canonical_json(capture)),
    "behaviorPolicy" => {
      "policySha256" => string!(policy_metadata["policySha256"], "behavior policy digest"),
      "sourceEvidenceSha256" => string!(policy_metadata["sourceEvidenceSha256"],
                                        "behavior policy source-evidence digest"),
    },
    "admissionReceipt" => canonical(admission_receipt),
    "graph" => first_projection,
    "fullCapture" => capture["first"],
  }
end

def fixture_list(items)
  { "apiVersion" => "v1", "kind" => "List", "metadata" => { "resourceVersion" => "" }, "items" => items }
end

def fixture_service(name, uid, ip)
  neg = "k8s1-002d2683-chrono-bot-#{name}-80-abc123"
  {
    "apiVersion" => "v1", "kind" => "Service",
    "metadata" => {
      "name" => name, "namespace" => "chrono-bot", "uid" => uid, "resourceVersion" => "10",
      "labels" => { "app.kubernetes.io/name" => name, "app.kubernetes.io/part-of" => "cma" },
      "annotations" => {
        "cloud.google.com/neg" => JSON.generate({ "exposed_ports" => { "80" => {} } }),
        "cloud.google.com/neg-status" => JSON.generate({
          "network_endpoint_groups" => { "80" => neg }, "zones" => ["asia-northeast1-a"],
        }),
        "kubectl.kubernetes.io/last-applied-configuration" => JSON.generate({
          "apiVersion" => "v1", "kind" => "Service",
          "metadata" => {
            "annotations" => {},
            "labels" => { "app.kubernetes.io/name" => name, "app.kubernetes.io/part-of" => "cma" },
            "name" => name, "namespace" => "chrono-bot",
          },
          "spec" => {
            "ports" => [{ "name" => "http", "port" => 80, "protocol" => "TCP", "targetPort" => "http" }],
            "selector" => { "app.kubernetes.io/name" => name }, "type" => "ClusterIP",
          },
        }) + "\n",
      },
    },
    "spec" => {
      "clusterIP" => ip, "clusterIPs" => [ip], "internalTrafficPolicy" => "Cluster",
      "ipFamilies" => ["IPv4"], "ipFamilyPolicy" => "SingleStack", "sessionAffinity" => "None",
      "ports" => [{ "name" => "http", "port" => 80, "protocol" => "TCP", "targetPort" => "http" }],
      "selector" => { "app.kubernetes.io/name" => name }, "type" => "ClusterIP",
    },
    "status" => { "loadBalancer" => {} },
  }
end

def fixture_certificate_pair(algorithm = :rsa)
  key = if algorithm == :rsa
          OpenSSL::PKey::RSA.new(1024)
        else
          OpenSSL::PKey::EC.generate("prime256v1")
        end
  certificate = OpenSSL::X509::Certificate.new
  certificate.version = 2
  certificate.serial = 1
  certificate.subject = OpenSSL::X509::Name.parse("/CN=bot.chrono-ai.fun")
  certificate.issuer = certificate.subject
  certificate.public_key = key
  certificate.not_before = Time.utc(2026, 1, 1)
  certificate.not_after = Time.utc(2027, 1, 1)
  extension_factory = OpenSSL::X509::ExtensionFactory.new
  extension_factory.subject_certificate = certificate
  extension_factory.issuer_certificate = certificate
  certificate.add_extension(extension_factory.create_extension(
                              "subjectAltName", "DNS:bot.chrono-ai.fun,DNS:bot-api.chrono-ai.fun",
                            ))
  certificate.sign(key, OpenSSL::Digest::SHA256.new)
  [certificate.to_pem, key.to_pem]
end

def build_fixture_state
  api_service = fixture_service("cma-api", "api-service-uid", "10.42.0.8")
  web_service = fixture_service("cma-web", "web-service-uid", "10.42.0.9")
  api_neg = JSON.parse(api_service.dig("metadata", "annotations", "cloud.google.com/neg-status"))
  web_neg = JSON.parse(web_service.dig("metadata", "annotations", "cloud.google.com/neg-status"))
  api_backend = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/backendServices/gkegw1-x-chrono-bot-cma-api-80-a"
  web_backend = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/backendServices/gkegw1-x-chrono-bot-cma-web-80-b"
  api_health = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/healthChecks/gkegw1-x-chrono-bot-cma-api-80-a"
  web_health = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/healthChecks/gkegw1-x-chrono-bot-cma-web-80-b"
  address = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/addresses/chronoai-gateway-external-ip"
  http_proxy = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/targetHttpProxies/http-proxy"
  https_proxy = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/targetHttpsProxies/https-proxy"
  redirect_map = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/urlMaps/redirect-map"
  serving_map = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/urlMaps/serving-map"
  certificate = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/sslCertificates/cma-cert"
  ssl_policy = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/sslPolicies/cma-policy"
  fr_http = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/forwardingRules/fr-http"
  fr_https = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/forwardingRules/fr-https"
  firewall = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/firewalls/gkegw1-l7-cma"
  certificate_pem, = fixture_certificate_pair
  annotations = {
    "networking.gke.io/backend-services" => [api_backend, web_backend].map { |ref| ref.delete_prefix(API_ROOT) }.join(", "),
    "networking.gke.io/health-checks" => [api_health, web_health].map { |ref| ref.delete_prefix(API_ROOT) }.join(", "),
    "networking.gke.io/forwarding-rules" => [fr_http, fr_https].map { |ref| ref.delete_prefix(API_ROOT) }.join(", "),
    "networking.gke.io/target-http-proxies" => http_proxy.delete_prefix(API_ROOT),
    "networking.gke.io/target-https-proxies" => https_proxy.delete_prefix(API_ROOT),
    "networking.gke.io/url-maps" => [redirect_map, serving_map].map { |ref| ref.delete_prefix(API_ROOT) }.join(", "),
    "networking.gke.io/ssl-certificates" => certificate.delete_prefix(API_ROOT),
    "networking.gke.io/firewalls" => firewall.delete_prefix(API_ROOT),
    "networking.gke.io/addresses" => "",
    "networking.gke.io/lb-edge-extensions" => "",
    "networking.gke.io/lb-route-extensions" => "",
    "networking.gke.io/lb-traffic-extensions" => "",
    "networking.gke.io/target-tcp-proxies" => "",
    "networking.gke.io/wasm-plugin-versions" => "",
    "networking.gke.io/wasm-plugins" => "",
  }
  gateway = {
    "apiVersion" => "gateway.networking.k8s.io/v1", "kind" => "Gateway",
    "metadata" => {
      "name" => "chronoai-gateway", "namespace" => "gateway-infra", "uid" => "gateway-uid",
      "resourceVersion" => "20", "generation" => 3, "annotations" => annotations,
    },
    "spec" => {
      "addresses" => [{ "type" => "NamedAddress", "value" => "chronoai-gateway-external-ip" }],
      "gatewayClassName" => "gke-l7-global-external-managed",
      "listeners" => [
        { "allowedRoutes" => { "namespaces" => { "from" => "All" } }, "name" => "http", "port" => 80, "protocol" => "HTTP" },
        {
          "allowedRoutes" => { "namespaces" => { "from" => "All" } }, "name" => "https", "port" => 443,
          "protocol" => "HTTPS", "tls" => {
            "certificateRefs" => [{ "group" => "", "kind" => "Secret", "name" => "chrono-ai-fun-cert" }],
            "mode" => "Terminate",
          },
        },
      ],
    },
    "status" => {
      "addresses" => [{ "type" => "IPAddress", "value" => "203.0.113.10" }],
      "conditions" => [{ "type" => "Programmed", "status" => "True", "observedGeneration" => 3 }],
      "listeners" => %w[http https].map do |name|
        {
          "name" => name,
          "conditions" => %w[Accepted Programmed ResolvedRefs].map do |type|
            { "type" => type, "status" => "True", "observedGeneration" => 3 }
          end,
        }
      end,
    },
  }
  route = lambda do |name|
    section = name == "cma-http-redirect" ? "http" : "https"
    spec = case name
           when "cma-api"
             {
               "parentRefs" => [gateway_parent(section)],
               "hostnames" => ["bot-api.chrono-ai.fun"],
               "rules" => [{
                 "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
                 "backendRefs" => service_backend("cma-api"),
               }],
             }
           when "cma-web"
             {
               "parentRefs" => [gateway_parent(section)],
               "hostnames" => ["bot.chrono-ai.fun"],
               "rules" => [
                 {
                   "matches" => [
                     { "path" => { "type" => "PathPrefix", "value" => "/api" } },
                     { "path" => { "type" => "PathPrefix", "value" => "/auth" } },
                     { "path" => { "type" => "Exact", "value" => "/openapi.json" } },
                     { "path" => { "type" => "Exact", "value" => "/.well-known/agent-card.json" } },
                   ],
                   "backendRefs" => service_backend("cma-api"),
                 },
                 {
                   "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
                   "backendRefs" => service_backend("cma-web"),
                 },
               ],
             }
           else
             {
               "parentRefs" => [gateway_parent(section)],
               "hostnames" => ["bot.chrono-ai.fun", "bot-api.chrono-ai.fun"],
               "rules" => [{
                 "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
                 "filters" => [{
                   "type" => "RequestRedirect",
                   "requestRedirect" => { "scheme" => "https", "statusCode" => 301 },
                 }],
               }],
             }
           end
    {
      "apiVersion" => "gateway.networking.k8s.io/v1", "kind" => "HTTPRoute",
      "metadata" => {
        "name" => name, "namespace" => "chrono-bot", "uid" => "#{name}-uid",
        "resourceVersion" => "30", "generation" => 3,
      },
      "spec" => spec,
      "status" => { "parents" => [{
        "controllerName" => "networking.gke.io/gateway", "parentRef" => gateway_parent(section),
        "conditions" => %w[Accepted Reconciled ResolvedRefs].map do |type|
          { "type" => type, "status" => "True", "observedGeneration" => 3 }
        end,
      }] },
    }
  end
  policy = lambda do |kind, name|
    generation = 2
    {
      "apiVersion" => "networking.gke.io/v1", "kind" => kind,
      "metadata" => {
        "name" => name, "namespace" => "chrono-bot", "uid" => "#{kind}-#{name}-uid",
        "resourceVersion" => "40", "generation" => generation,
      },
      "spec" => kind == "HealthCheckPolicy" ? {
        "targetRef" => { "group" => "", "kind" => "Service", "name" => "cma-api" },
        "default" => {
          "config" => {
            "type" => "HTTP", "httpHealthCheck" => { "port" => 8080, "requestPath" => "/ready" },
          },
        },
      } : {
        "targetRef" => { "group" => "", "kind" => "Service", "name" => name },
        "default" => { "timeoutSec" => 660 },
      },
      "status" => { "conditions" => [{
        "type" => "Attached", "status" => "True", "reason" => "Attached",
        "message" => "", "lastTransitionTime" => "2026-01-01T00:00:00Z",
      }] },
    }
  end
  pod_and_slice = lambda do |name, service, ip, pod_ip, pod_uid|
    pod = {
      "apiVersion" => "v1", "kind" => "Pod",
      "metadata" => { "name" => "#{name}-pod", "namespace" => "chrono-bot", "uid" => pod_uid },
      "spec" => { "nodeName" => "node-a" },
      "status" => {
        "podIP" => pod_ip, "phase" => "Running",
        "conditions" => [{ "type" => "Ready", "status" => "True" }],
        "containerStatuses" => [{
          "name" => name == "cma-api" ? "api" : "web", "ready" => true, "started" => true,
          "restartCount" => 0, "state" => { "running" => { "startedAt" => "2026-01-01T00:00:00Z" } },
        }],
      },
    }
    slice = {
      "apiVersion" => "discovery.k8s.io/v1", "kind" => "EndpointSlice", "addressType" => "IPv4",
      "metadata" => {
        "name" => "#{name}-slice", "namespace" => "chrono-bot", "uid" => "#{name}-slice-uid",
        "resourceVersion" => "50", "ownerReferences" => [{
          "apiVersion" => "v1", "kind" => "Service", "name" => name,
          "uid" => service.dig("metadata", "uid"), "controller" => true,
        }],
      },
      "ports" => [{ "name" => "http", "port" => 8080, "protocol" => "TCP" }],
      "endpoints" => [{
        "addresses" => [pod_ip],
        "conditions" => { "ready" => true, "serving" => true, "terminating" => false },
        "nodeName" => "node-a", "zone" => "asia-northeast1-a",
        "targetRef" => { "kind" => "Pod", "name" => "#{name}-pod", "namespace" => "chrono-bot", "uid" => pod_uid },
      }],
    }
    [fixture_list([slice]), fixture_list([pod])]
  end
  api_slices, api_pods = pod_and_slice.call("cma-api", api_service, "10.42.0.8", "10.0.0.8", "api-pod-uid")
  web_slices, web_pods = pod_and_slice.call("cma-web", web_service, "10.42.0.9", "10.0.0.9", "web-pod-uid")
  node = {
    "apiVersion" => "v1", "kind" => "Node",
    "metadata" => { "name" => "node-a", "uid" => "node-uid", "resourceVersion" => "60" },
    "spec" => { "providerID" => "gce://#{PROJECT_ID}/asia-northeast1-a/instance-a" },
    "status" => {},
  }
  group_link = lambda do |service, neg_status|
    "#{API_ROOT}/projects/#{PROJECT_NUMBER}/zones/asia-northeast1-a/networkEndpointGroups/#{neg_status.fetch('network_endpoint_groups').fetch('80')}"
  end
  api_group = group_link.call(api_service, api_neg)
  web_group = group_link.call(web_service, web_neg)
  backend_body = lambda do |link, health, group|
    {
      "selfLink" => link, "name" => link.split("/").last, "fingerprint" => "backend-fingerprint",
      "loadBalancingScheme" => "EXTERNAL_MANAGED", "protocol" => "HTTP", "timeoutSec" => 660,
      "healthChecks" => [health], "enableCDN" => false,
      "customRequestHeaders" => [], "customResponseHeaders" => [], "sessionAffinity" => "NONE",
      "connectionDraining" => { "drainingTimeoutSec" => 0 }, "logConfig" => { "enable" => false },
      "backends" => [{ "group" => group, "balancingMode" => "RATE", "maxRatePerEndpoint" => 1.0 }],
    }
  end
  neg_body = lambda do |link, ip|
    {
      "resource" => {
        "selfLink" => link, "name" => link.split("/").last,
        "zone" => "#{API_ROOT}/projects/#{PROJECT_ID}/zones/asia-northeast1-a",
        "networkEndpointType" => "GCE_VM_IP_PORT", "fingerprint" => "neg-fingerprint",
      },
      "endpoints" => [{ "instance" => "instance-a", "ipAddress" => ip, "port" => 8080 }],
    }
  end
  backend_health = lambda do |backend, group, ip|
    {
      "backendService" => backend,
      "groups" => [{
        "backend" => group,
        "status" => {
          "kind" => "compute#backendServiceGroupHealth",
          "healthStatus" => [{
            "healthState" => "HEALTHY", "instance" => "instance-a",
            "ipAddress" => ip, "port" => 8080,
          }],
        },
      }],
    }
  end
  admission_policy = {
    "apiVersion" => "admissionregistration.k8s.io/v1", "kind" => "ValidatingAdmissionPolicy",
    "metadata" => {
      "name" => "cma-cutover-fence-label-reservation", "uid" => "fence-policy-uid",
      "resourceVersion" => "70", "generation" => 1,
      "labels" => { "app.kubernetes.io/name" => "cma-api", "app.kubernetes.io/part-of" => "cma" },
    },
    "spec" => {
      "failurePolicy" => "Fail",
      "matchConstraints" => {
        "matchPolicy" => "Equivalent",
        "namespaceSelector" => { "matchLabels" => { "kubernetes.io/metadata.name" => "chrono-bot" } },
        "resourceRules" => [{
          "apiGroups" => [""], "apiVersions" => ["v1"], "operations" => %w[CREATE UPDATE],
          "resources" => ["pods"], "scope" => "Namespaced",
        }],
      },
      "matchConditions" => [{
        "name" => "exact-chrono-bot-namespace", "expression" => "request.namespace == 'chrono-bot'",
      }],
      "validations" => [{
        "expression" => "!has(object.metadata.labels) || !('release.chrono-ai.fun/cutover-fence' in object.metadata.labels) || object.metadata.labels['release.chrono-ai.fun/cutover-fence'] != 'mongodb-715'",
        "message" => "Pods in chrono-bot must not use the reserved CMA cutover-fence label value mongodb-715",
        "reason" => "Forbidden",
      }],
    },
    "status" => { "observedGeneration" => 1, "typeChecking" => { "expressionWarnings" => [] } },
  }
  admission_binding = {
    "apiVersion" => "admissionregistration.k8s.io/v1",
    "kind" => "ValidatingAdmissionPolicyBinding",
    "metadata" => {
      "name" => "cma-cutover-fence-label-reservation", "uid" => "fence-binding-uid",
      "resourceVersion" => "71", "generation" => 1,
      "labels" => { "app.kubernetes.io/name" => "cma-api", "app.kubernetes.io/part-of" => "cma" },
    },
    "spec" => {
      "policyName" => "cma-cutover-fence-label-reservation", "validationActions" => ["Deny"],
      "matchResources" => {
        "matchPolicy" => "Equivalent",
        "namespaceSelector" => { "matchLabels" => { "kubernetes.io/metadata.name" => "chrono-bot" } },
        "resourceRules" => [{
          "apiGroups" => [""], "apiVersions" => ["v1"], "operations" => %w[CREATE UPDATE],
          "resources" => ["pods"], "scope" => "Namespaced",
        }],
      },
    },
  }
  {
    "routeMode" => "adopted",
    "kubernetes" => {
      "gateway" => gateway,
      "certificateSecret" => {
        "apiVersion" => "v1", "kind" => "Secret", "type" => "kubernetes.io/tls",
        "metadata" => {
          "name" => "chrono-ai-fun-cert", "namespace" => "gateway-infra",
          "uid" => "certificate-secret-uid", "resourceVersion" => "25",
        },
        "dataKeys" => %w[tls.crt tls.key], "certificatePem" => certificate_pem,
        "keyPresent" => true,
        "publicSpkiSha256" => Digest::SHA256.hexdigest(
          OpenSSL::X509::Certificate.new(certificate_pem).public_key.to_der,
        ),
      },
      "admissionPolicy" => admission_policy,
      "admissionBinding" => admission_binding,
      "routes" => fixture_list(%w[cma-api cma-web cma-http-redirect].map { |name| route.call(name) }),
      "healthPolicy" => policy.call("HealthCheckPolicy", "cma-api"),
      "backendPolicies" => fixture_list(CMA_SERVICES.map { |name| policy.call("GCPBackendPolicy", name) }),
      "services" => fixture_list([api_service, web_service]),
      "endpointSlices" => { "cma-api" => api_slices, "cma-web" => web_slices },
      "pods" => { "cma-api" => api_pods, "cma-web" => web_pods },
      "nodes" => fixture_list([node]),
    },
    "compute" => {
      "address" => {
        "selfLink" => address, "name" => "chronoai-gateway-external-ip", "address" => "203.0.113.10",
        "addressType" => "EXTERNAL", "ipVersion" => "IPV4", "status" => "IN_USE", "fingerprint" => "address-fingerprint",
      },
      "firewalls" => [{
        "selfLink" => firewall, "name" => "gkegw1-l7-cma", "direction" => "INGRESS",
        "disabled" => false, "priority" => 1000, "sourceRanges" => ["35.191.0.0/16"],
        "allowed" => [{ "IPProtocol" => "tcp", "ports" => ["8080"] }],
        "targetTags" => ["gke-node"], "fingerprint" => "firewall-fingerprint",
      }],
      "forwardingRules" => [
        { "selfLink" => fr_http, "IPAddress" => "203.0.113.10", "IPProtocol" => "TCP", "portRange" => "80-80", "target" => http_proxy, "loadBalancingScheme" => "EXTERNAL_MANAGED", "networkTier" => "PREMIUM", "fingerprint" => "fr-http-fingerprint" },
        { "selfLink" => fr_https, "IPAddress" => "203.0.113.10", "IPProtocol" => "TCP", "portRange" => "443-443", "target" => https_proxy, "loadBalancingScheme" => "EXTERNAL_MANAGED", "networkTier" => "PREMIUM", "fingerprint" => "fr-https-fingerprint" },
      ],
      "targetHttpProxies" => [{ "selfLink" => http_proxy, "urlMap" => redirect_map, "proxyHeader" => "NONE", "fingerprint" => "http-proxy-fingerprint" }],
      "targetHttpsProxies" => [{ "selfLink" => https_proxy, "urlMap" => serving_map, "sslCertificates" => [certificate], "sslPolicy" => ssl_policy, "quicOverride" => "NONE", "proxyHeader" => "NONE", "fingerprint" => "https-proxy-fingerprint" }],
      "urlMaps" => [
        { "selfLink" => redirect_map, "defaultUrlRedirect" => { "httpsRedirect" => true, "redirectResponseCode" => "MOVED_PERMANENTLY_DEFAULT", "stripQuery" => false }, "fingerprint" => "redirect-fingerprint" },
        {
          "selfLink" => serving_map,
          "hostRules" => [
            { "hosts" => ["bot-api.chrono-ai.fun"], "pathMatcher" => "api" },
            { "hosts" => ["bot.chrono-ai.fun"], "pathMatcher" => "web" },
          ],
          "pathMatchers" => [
            { "name" => "api", "defaultService" => api_backend },
            { "name" => "web", "defaultService" => web_backend, "pathRules" => [{ "paths" => ["/api/*", "/auth/*", "/openapi.json", "/.well-known/agent-card.json"], "service" => api_backend }] },
          ],
          "defaultService" => web_backend, "fingerprint" => "serving-fingerprint",
        },
      ],
      "sslCertificates" => [{
        "selfLink" => certificate, "type" => "SELF_MANAGED", "certificate" => certificate_pem,
        "fingerprint" => "cert-fingerprint",
      }],
      "sslPolicies" => [{ "selfLink" => ssl_policy, "profile" => "MODERN", "minTlsVersion" => "TLS_1_2", "fingerprint" => "ssl-fingerprint" }],
      "backendServices" => [backend_body.call(api_backend, api_health, api_group), backend_body.call(web_backend, web_health, web_group)],
      "backendHealth" => [
        backend_health.call(api_backend, api_group, "10.0.0.8"),
        backend_health.call(web_backend, web_group, "10.0.0.9"),
      ],
      "healthChecks" => [
        { "selfLink" => api_health, "fingerprint" => "api-health-fingerprint", "type" => "HTTP", "checkIntervalSec" => 5, "timeoutSec" => 5, "healthyThreshold" => 1, "unhealthyThreshold" => 2, "httpHealthCheck" => { "port" => 8080, "portSpecification" => "USE_FIXED_PORT", "proxyHeader" => "NONE", "requestPath" => "/ready" }, "logConfig" => { "enable" => false } },
        { "selfLink" => web_health, "fingerprint" => "web-health-fingerprint", "type" => "HTTP", "checkIntervalSec" => 5, "timeoutSec" => 5, "healthyThreshold" => 1, "unhealthyThreshold" => 2, "httpHealthCheck" => { "port" => 8080, "portSpecification" => "USE_FIXED_PORT", "proxyHeader" => "NONE", "requestPath" => "/" }, "logConfig" => { "enable" => false } },
      ],
      "networkEndpointGroups" => [neg_body.call(api_group, "10.0.0.8"), neg_body.call(web_group, "10.0.0.9")],
    },
  }
end

def cloned(value)
  Marshal.load(Marshal.dump(value))
end

def run_self_tests
  state = build_fixture_state
  admission_receipt = {
    "receiptSha256" => "a" * 64,
    "releaseRevision" => "b" * 40,
    "sourceSetSha256" => "c" * 64,
    "manifestSha256" => "d" * 64,
    "policy" => {
      "uid" => state.dig("kubernetes", "admissionPolicy", "metadata", "uid"),
      "generation" => state.dig("kubernetes", "admissionPolicy", "metadata", "generation"),
      "resourceVersion" => state.dig("kubernetes", "admissionPolicy", "metadata", "resourceVersion"),
    },
    "binding" => {
      "uid" => state.dig("kubernetes", "admissionBinding", "metadata", "uid"),
      "generation" => state.dig("kubernetes", "admissionBinding", "metadata", "generation"),
      "resourceVersion" => state.dig("kubernetes", "admissionBinding", "metadata", "resourceVersion"),
    },
  }
  capture = {
    "schemaVersion" => 1, "mode" => "source",
    "lifecycle" => { "stage" => "source", "predecessorSha256" => "genesis" },
    "admissionReceipt" => admission_receipt,
    "first" => state, "second" => cloned(state),
  }
  expected_behavior = compute_behavior_policy(state)
  projection = validate_capture(cloned(capture), expected_behavior)
  assert!(projection["captureSha256"].match?(/\A[0-9a-f]{64}\z/), "self-test digest is malformed")

  ec_capture = cloned(capture)
  ec_certificate, = fixture_certificate_pair(:ec)
  ec_spki = Digest::SHA256.hexdigest(
    OpenSSL::X509::Certificate.new(ec_certificate).public_key.to_der,
  )
  %w[first second].each do |read|
    secret = ec_capture.dig(read, "kubernetes", "certificateSecret")
    secret["certificatePem"] = ec_certificate
    secret["publicSpkiSha256"] = ec_spki
    ec_capture.dig(read, "compute", "sslCertificates", 0)["certificate"] = ec_certificate
  end
  validate_capture(ec_capture, expected_behavior)

  legacy = cloned(capture)
  %w[first second].each do |read|
    live = legacy.fetch(read)
    live["routeMode"] = "legacy"
    web_route = live.dig("kubernetes", "routes", "items").find do |route|
      route.dig("metadata", "name") == "cma-web"
    end
    web_route["spec"]["rules"] = [{
      "matches" => [{ "path" => { "type" => "PathPrefix", "value" => "/" } }],
      "backendRefs" => service_backend("cma-web"),
    }]
    live.dig("kubernetes", "healthPolicy", "spec", "default", "config", "httpHealthCheck")["requestPath"] = "/livez"
    api_health = live.dig("compute", "healthChecks").find { |health| health["selfLink"].include?("-cma-api-") }
    api_health.dig("httpHealthCheck")["requestPath"] = "/livez"
    web_matcher = live.dig("compute", "urlMaps", 1, "pathMatchers").find { |matcher| matcher["name"] == "web" }
    web_matcher.delete("pathRules")
  end
  validate_capture(legacy, expected_behavior)

  unrelated = cloned(capture)
  unrelated_route = {
    "apiVersion" => "gateway.networking.k8s.io/v1", "kind" => "HTTPRoute",
    "metadata" => { "namespace" => "other", "name" => "same-host-other-gateway" },
    "spec" => {
      "parentRefs" => [{ "name" => "other-gateway" }],
      "hostnames" => ["bot.chrono-ai.fun"], "rules" => [],
    },
  }
  %w[first second].each do |read|
    unrelated.dig(read, "kubernetes", "routes", "items") << cloned(unrelated_route)
  end
  validate_capture(unrelated, expected_behavior)

  fenced = cloned(capture)
  fenced["mode"] = "fenced"
  fenced["lifecycle"] = { "stage" => "fenced-pre-scale", "predecessorSha256" => "e" * 64 }
  %w[first second].each do |read|
    live = fenced.fetch(read)
    api_service = live.dig("kubernetes", "services", "items").find do |service|
      service.dig("metadata", "name") == "cma-api"
    end
    api_service["spec"]["selector"] = { "release.chrono-ai.fun/cutover-fence" => "mongodb-715" }
    live.dig("kubernetes", "endpointSlices", "cma-api", "items").each do |slice|
      slice["endpoints"] = []
    end
    api_neg = live.dig("compute", "networkEndpointGroups").find do |entry|
      entry.dig("resource", "selfLink").include?("-cma-api-")
    end
    api_neg["endpoints"] = []
    api_health = live.dig("compute", "backendHealth").find do |entry|
      entry["backendService"].include?("-cma-api-")
    end
    api_health["groups"].each { |group| group.dig("status")["healthStatus"] = [] }
  end
  validate_capture(fenced, expected_behavior)

  mutations = {
    "Gateway UID drift" => ->(candidate) { candidate["second"]["kubernetes"]["gateway"]["metadata"]["uid"] = "replacement" },
    "admission receipt policy drift" => ->(candidate) { candidate["admissionReceipt"]["policy"]["uid"] = "replacement" },
    "Gateway spec drift" => ->(candidate) { candidate["first"]["kubernetes"]["gateway"]["spec"]["gatewayClassName"] = "other" },
    "address drift" => ->(candidate) { candidate["first"]["compute"]["address"]["address"] = "203.0.113.11" },
    "forwarding port drift" => ->(candidate) { candidate["first"]["compute"]["forwardingRules"][0]["portRange"] = "81-81" },
    "forwarding target drift" => ->(candidate) { candidate["first"]["compute"]["forwardingRules"][0]["target"] = candidate["first"]["compute"]["targetHttpsProxies"][0]["selfLink"] },
    "stable unannotated forwarding rule on Gateway IP" => lambda do |candidate|
      %w[first second].each do |read|
        rule = cloned(candidate.dig(read, "compute", "forwardingRules", 0))
        rule["selfLink"] = "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/forwardingRules/extra"
        rule["name"] = "extra"
        candidate.dig(read, "compute", "forwardingRules") << rule
      end
    end,
    "stable firewall behavior drift" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "compute", "firewalls", 0)["sourceRanges"] = ["0.0.0.0/0"]
      end
    end,
    "proxy map drift" => ->(candidate) { candidate["first"]["compute"]["targetHttpsProxies"][0]["urlMap"] = candidate["first"]["compute"]["urlMaps"][0]["selfLink"] },
    "certificate omission" => ->(candidate) { candidate["first"]["compute"]["sslCertificates"] = [] },
    "SSL policy omission" => ->(candidate) { candidate["first"]["compute"]["sslPolicies"] = [] },
    "QUIC drift" => ->(candidate) { candidate["first"]["compute"]["targetHttpsProxies"][0]["quicOverride"] = "UNKNOWN" },
    "wildcard overlap" => ->(candidate) { candidate["first"]["compute"]["urlMaps"][1]["hostRules"] << { "hosts" => ["*.chrono-ai.fun"], "pathMatcher" => "web" } },
    "weighted backend" => ->(candidate) { candidate["first"]["compute"]["urlMaps"][1]["pathMatchers"][0]["defaultRouteAction"] = { "weightedBackendServices" => [] } },
    "backend group omission" => ->(candidate) { candidate["first"]["compute"]["backendServices"][0]["backends"] = [] },
    "backend behavior drift" => ->(candidate) { candidate["second"]["compute"]["backendServices"][0]["enableCDN"] = true },
    "health behavior drift" => ->(candidate) { candidate["second"]["compute"]["healthChecks"][0]["timeoutSec"] = 6 },
    "NEG capacity drift" => ->(candidate) { candidate["second"]["compute"]["backendServices"][0]["backends"][0]["maxRatePerEndpoint"] = 2.0 },
    "stable CDN enablement" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["enableCDN"] = true }
    end,
    "stable custom request header" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["customRequestHeaders"] = ["X-Test: unsafe"] }
    end,
    "stable security policy" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["securityPolicy"] = "policy" }
    end,
    "stable IAP enablement" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["iap"] = { "enabled" => true } }
    end,
    "stable affinity change" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["sessionAffinity"] = "CLIENT_IP" }
    end,
    "stable draining change" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["connectionDraining"]["drainingTimeoutSec"] = 30 }
    end,
    "stable logging change" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["logConfig"]["enable"] = true }
    end,
    "stable locality policy" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["localityLbPolicy"] = "RING_HASH" }
    end,
    "stable outlier policy" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["outlierDetection"] = { "consecutiveErrors" => 2 } }
    end,
    "stable balancing mode" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["backends"][0]["balancingMode"] = "UTILIZATION" }
    end,
    "stable capacity scaler" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["backendServices"][0]["backends"][0]["capacityScaler"] = 0.5 }
    end,
    "stable health timing" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["healthChecks"][0]["checkIntervalSec"] = 30 }
    end,
    "stable health proxy header" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["healthChecks"][0]["httpHealthCheck"]["proxyHeader"] = "PROXY_V1" }
    end,
    "stable health host" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["healthChecks"][0]["httpHealthCheck"]["host"] = "other.invalid" }
    end,
    "stable health response" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["healthChecks"][0]["httpHealthCheck"]["response"] = "unsafe" }
    end,
    "stable Compute certificate replacement" => lambda do |candidate|
      replacement, = fixture_certificate_pair
      %w[first second].each { |read| candidate[read]["compute"]["sslCertificates"][0]["certificate"] = replacement }
    end,
    "stable SSL profile" => lambda do |candidate|
      %w[first second].each { |read| candidate[read]["compute"]["sslPolicies"][0]["profile"] = "COMPATIBLE" }
    end,
    "unowned overlapping route" => lambda do |candidate|
      %w[first second].each do |read|
        candidate[read]["kubernetes"]["routes"]["items"] << {
          "apiVersion" => "gateway.networking.k8s.io/v1", "kind" => "HTTPRoute",
          "metadata" => { "namespace" => "other", "name" => "shadow" },
          "spec" => {
            "parentRefs" => [gateway_parent("https")], "hostnames" => ["bot.chrono-ai.fun"],
          },
        }
      end
    end,
    "unowned hostname-less attached route" => lambda do |candidate|
      %w[first second].each do |read|
        candidate[read]["kubernetes"]["routes"]["items"] << {
          "apiVersion" => "gateway.networking.k8s.io/v1", "kind" => "HTTPRoute",
          "metadata" => { "namespace" => "gateway-infra", "name" => "all-hosts" },
          "spec" => { "parentRefs" => [{ "name" => "chronoai-gateway", "sectionName" => "https" }] },
        }
      end
    end,
    "misrouted API path" => lambda do |candidate|
      candidate["first"]["compute"]["urlMaps"][1]["pathMatchers"][1]["pathRules"][0]["service"] =
        candidate["first"]["compute"]["backendServices"][1]["selfLink"]
    end,
    "wrong app default" => lambda do |candidate|
      candidate["first"]["compute"]["urlMaps"][1]["pathMatchers"][1]["defaultService"] =
        candidate["first"]["compute"]["backendServices"][0]["selfLink"]
    end,
    "misordered route priority" => lambda do |candidate|
      matcher = candidate["first"]["compute"]["urlMaps"][1]["pathMatchers"][1]
      matcher.delete("pathRules")
      matcher["routeRules"] = [
        { "priority" => 1, "matchRules" => [{ "prefixMatch" => "/" }],
          "service" => candidate["first"]["compute"]["backendServices"][1]["selfLink"] },
        { "priority" => 2, "matchRules" => [{ "prefixMatch" => "/api" }],
          "service" => candidate["first"]["compute"]["backendServices"][0]["selfLink"] },
      ]
    end,
    "HTTP redirect drift" => ->(candidate) { candidate["first"]["compute"]["urlMaps"][0]["defaultUrlRedirect"]["redirectResponseCode"] = "FOUND" },
    "Service NEG drift" => ->(candidate) { candidate["first"]["kubernetes"]["services"]["items"][0]["metadata"]["annotations"]["cloud.google.com/neg-status"] = JSON.generate({ "network_endpoint_groups" => { "80" => "wrong" }, "zones" => ["asia-northeast1-a"] }) },
    "stable Service last-applied drift" => lambda do |candidate|
      %w[first second].each do |read|
        service = candidate.dig(read, "kubernetes", "services", "items", 0)
        last_applied = JSON.parse(service.dig(
                                    "metadata", "annotations",
                                    "kubectl.kubernetes.io/last-applied-configuration",
                                  ))
        last_applied["spec"]["selector"] = { "app.kubernetes.io/name" => "replacement" }
        service["metadata"]["annotations"]["kubectl.kubernetes.io/last-applied-configuration"] =
          JSON.generate(last_applied)
      end
    end,
    "stable Service publishNotReadyAddresses" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "kubernetes", "services", "items", 0, "spec")["publishNotReadyAddresses"] = true
      end
    end,
    "stable pending Service controller annotation" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "kubernetes", "services", "items", 0, "metadata", "annotations")[
          "networking.gke.io/max-rate-per-endpoint"
        ] = "100"
      end
    end,
    "stable stale backend policy" => lambda do |candidate|
      %w[first second].each do |read|
        policy = candidate.dig(read, "kubernetes", "backendPolicies", "items", 0)
        policy["metadata"]["generation"] = 3
        policy["spec"]["default"]["timeoutSec"] = 1
        policy["status"]["conditions"] = [{
          "type" => "Attached", "status" => "False", "reason" => "Pending",
          "message" => "pending", "lastTransitionTime" => "2026-01-01T00:00:00Z",
        }]
      end
    end,
    "stable stale health policy" => lambda do |candidate|
      %w[first second].each do |read|
        policy = candidate.dig(read, "kubernetes", "healthPolicy")
        policy["metadata"]["generation"] = 3
        policy["status"]["conditions"] = [{
          "type" => "Attached", "status" => "False", "reason" => "Pending",
          "message" => "pending", "lastTransitionTime" => "2026-01-01T00:00:00Z",
        }]
      end
    end,
    "EndpointSlice pod UID drift" => ->(candidate) { candidate["first"]["kubernetes"]["endpointSlices"]["cma-api"]["items"][0]["endpoints"][0]["targetRef"]["uid"] = "wrong" },
    "stable EndpointSlice serving false" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "kubernetes", "endpointSlices", "cma-api", "items", 0,
                      "endpoints", 0, "conditions")["serving"] = false
      end
    end,
    "stable EndpointSlice node mismatch" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "kubernetes", "endpointSlices", "cma-api", "items", 0,
                      "endpoints", 0)["nodeName"] = "node-b"
      end
    end,
    "stable EndpointSlice zone mismatch" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "kubernetes", "endpointSlices", "cma-api", "items", 0,
                      "endpoints", 0)["zone"] = "asia-northeast1-b"
      end
    end,
    "stable Pod not Ready" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "kubernetes", "pods", "cma-api", "items", 0,
                      "status", "conditions", 0)["status"] = "False"
      end
    end,
    "stable container not started" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "kubernetes", "pods", "cma-api", "items", 0,
                      "status", "containerStatuses", 0)["started"] = false
      end
    end,
    "stable certificate Secret rotation pending controller" => lambda do |candidate|
      replacement, replacement_key = fixture_certificate_pair
      %w[first second].each do |read|
        secret = candidate.dig(read, "kubernetes", "certificateSecret")
        secret["certificatePem"] = replacement
        secret["resourceVersion"] = "26"
        secret["publicSpkiSha256"] = Digest::SHA256.hexdigest(
          OpenSSL::PKey.read(replacement_key).public_key.to_der,
        )
      end
    end,
    "stable mismatched certificate private key" => lambda do |candidate|
      _, mismatch_key = fixture_certificate_pair
      %w[first second].each do |read|
        secret = candidate.dig(read, "kubernetes", "certificateSecret")
        secret["publicSpkiSha256"] = Digest::SHA256.hexdigest(
          OpenSSL::PKey.read(mismatch_key).public_key.to_der,
        )
      end
    end,
    "stable unhealthy load-balancer endpoint" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "compute", "backendHealth", 0, "groups", 0,
                      "status", "healthStatus", 0)["healthState"] = "UNHEALTHY"
      end
    end,
    "stable unknown load-balancer endpoint" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "compute", "backendHealth", 0, "groups", 0,
                      "status", "healthStatus", 0)["healthState"] = "UNKNOWN"
      end
    end,
    "stable missing load-balancer health" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "compute", "backendHealth", 0, "groups", 0,
                      "status")["healthStatus"] = []
      end
    end,
    "stable unexpected Compute consumer" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "compute", "backendServices", 0)["usedBy"] = ["unexpected-consumer"]
      end
    end,
    "stable unsupported Gateway extension" => lambda do |candidate|
      %w[first second].each do |read|
        candidate.dig(read, "kubernetes", "gateway", "metadata", "annotations")[
          "networking.gke.io/lb-route-extensions"
        ] = "/projects/191845086376/global/lbRouteExtensions/unsafe"
      end
    end,
    "Pod address drift" => ->(candidate) { candidate["first"]["kubernetes"]["pods"]["cma-api"]["items"][0]["status"]["podIP"] = "10.0.0.99" },
    "provider instance drift" => ->(candidate) { candidate["first"]["compute"]["networkEndpointGroups"][0]["endpoints"][0]["instance"] = "other-instance" },
    "provider zone drift" => ->(candidate) { candidate["first"]["kubernetes"]["nodes"]["items"][0]["spec"]["providerID"] = "gce://#{PROJECT_ID}/asia-northeast1-b/instance-a" },
    "extra health check" => ->(candidate) { candidate["first"]["compute"]["healthChecks"] << cloned(candidate["first"]["compute"]["healthChecks"][0]).merge("selfLink" => "#{API_ROOT}/projects/#{PROJECT_NUMBER}/global/healthChecks/extra") },
    "open compute key" => ->(candidate) { candidate["first"]["compute"]["firewalls"] = [] },
  }
  mutations.each do |name, mutation|
    candidate = cloned(capture)
    mutation.call(candidate)
    begin
      validate_capture(candidate, expected_behavior)
    rescue GatewayProjectionError, KeyError, TypeError, JSON::ParserError
      next
    end
    reject!("self-test accepted #{name}")
  end
  puts "CMA Trigger closed Gateway projection self-tests passed (#{mutations.length} mutations)"
end

unless ARGV == ["--self-test"] || (ARGV.length == 2 && ARGV[0] == "--validate") ||
       (ARGV.length == 3 && %w[--derive-policy --emit].include?(ARGV[0]))
  warn "usage: #{File.basename($PROGRAM_NAME)} --self-test | --validate CAPTURE.json | --emit CAPTURE.json PROJECTION.json | --derive-policy CAPTURE.json POLICY.json"
  exit 2
end

begin
  if ARGV == ["--self-test"]
    run_self_tests
  elsif ARGV[0] == "--derive-policy"
    capture = parse_json_file(ARGV.fetch(1))
    assert!(capture.is_a?(Hash) && capture["schemaVersion"] == 1 &&
            %w[source fenced].include?(capture["mode"]), "capture identity is not exact")
    assert!(canonical_json(capture["first"]) == canonical_json(capture["second"]),
            "candidate policy requires two identical coherent live reads")
    expected = compute_behavior_policy(capture["first"])
    policy = {
      "schemaVersion" => 1,
      "status" => "candidate-requires-maintainer-review",
      "sourceEvidenceSha256" => Digest::SHA256.hexdigest(canonical_json(capture)),
      "expected" => expected,
    }
    File.open(ARGV.fetch(2), File::WRONLY | File::CREAT | File::EXCL, 0o600) do |file|
      file.write(JSON.pretty_generate(canonical(policy)))
      file.write("\n")
      file.fsync
    end
    puts "CMA Trigger candidate behavior policy emitted; it is not release approval"
  else
    projection = validate_capture(parse_json_file(ARGV.fetch(1)))
    if ARGV[0] == "--emit"
      output = ARGV.fetch(2)
      File.open(output, File::WRONLY | File::CREAT | File::EXCL, 0o600) do |file|
        file.write(JSON.pretty_generate(canonical(projection)))
        file.write("\n")
        file.fsync
      end
    end
    puts "CMA Trigger closed Gateway projection is coherent and complete"
  end
rescue GatewayProjectionError, JSON::ParserError, KeyError, TypeError, SystemCallError => error
  warn "CMA Trigger Gateway projection: #{error.message}"
  exit 1
end

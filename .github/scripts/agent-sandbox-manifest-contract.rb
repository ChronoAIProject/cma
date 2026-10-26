# frozen_string_literal: true

require "yaml"

path = File.expand_path("../../infra/api/serviceaccount.yaml", __dir__)
documents = YAML.load_stream(File.read(path))
control = documents.find { |value| value["kind"] == "ServiceAccount" && value.dig("metadata", "name") == "chrono-bot-ksa" }
abort "Agent Manager Workload Identity binding changed" unless control && control.dig("metadata", "annotations", "iam.gke.io/gcp-service-account") == "chrono-cma@chronoai-501608.iam.gserviceaccount.com"
role = documents.find { |value| value["kind"] == "Role" && value.dig("metadata", "name") == "cma-batchsandbox-reclaimer" }
abort "Native Sandbox namespaced Role is missing" unless role && role.dig("metadata", "namespace") == "chrono-bot"
rules = role.fetch("rules")
required = {
  ["agents.x-k8s.io", "sandboxes"] => %w[get list create patch delete],
  ["extensions.agents.x-k8s.io", "sandboxtemplates"] => %w[get list create patch delete],
  ["extensions.agents.x-k8s.io", "sandboxwarmpools"] => %w[get list create patch delete],
  ["extensions.agents.x-k8s.io", "sandboxclaims"] => %w[get list create patch delete],
  ["", "persistentvolumeclaims"] => %w[get list create patch delete],
  ["", "pods"] => %w[get list],
  ["", "pods/exec"] => %w[get create],
  ["snapshot.storage.k8s.io", "volumesnapshots"] => %w[get],
}
required.each do |(group, resource), verbs|
  granted = rules.select { |rule| rule.fetch("apiGroups").include?(group) && rule.fetch("resources").include?(resource) }.flat_map { |rule| rule.fetch("verbs") }
  abort "Native Sandbox Role missing #{group}/#{resource} permissions" unless (verbs - granted).empty?
end
abort "Native Sandbox Role must not have wildcard authority" if rules.any? { |rule| %w[apiGroups resources verbs].any? { |key| rule.fetch(key).include?("*") } }
binding = documents.find { |value| value["kind"] == "RoleBinding" && value.dig("roleRef", "name") == role.dig("metadata", "name") }
abort "Native Sandbox RoleBinding identity changed" unless binding && binding.fetch("subjects") == [{ "kind" => "ServiceAccount", "name" => "chrono-bot-ksa", "namespace" => "chrono-bot" }]
runtime = documents.find { |value| value["kind"] == "ServiceAccount" && value.dig("metadata", "name") == "cma-runtime" }
abort "Runtime must not receive an automatic Kubernetes API token" unless runtime && runtime["automountServiceAccountToken"] == false

policy = documents.find { |value| value["kind"] == "NetworkPolicy" && value.dig("metadata", "name") == "cma-agent-sandbox-control" }
abort "Native runtime ingress policy missing" unless policy && policy.dig("metadata", "namespace") == "chrono-bot"
selector = policy.dig("spec", "podSelector")
abort "Native runtime selector must include cold and pooled Pods only" unless selector == {
  "matchLabels" => { "cma/control-plane" => "chrono-bot-prod" },
  "matchExpressions" => [{ "key" => "agents.x-k8s.io/sandbox-name-hash", "operator" => "Exists" }],
}
abort "Native runtime ingress must admit only Agent Manager TCP control" unless policy.dig("spec", "policyTypes") == ["Ingress"] && policy.dig("spec", "ingress") == [{
  "from" => [{ "podSelector" => { "matchLabels" => { "app.kubernetes.io/name" => "cma-agent-manager" } } }],
  "ports" => [{ "protocol" => "TCP", "port" => 9090 }],
}]
puts "Native Sandbox Workload Identity, RBAC, runtime token isolation and cold/warm network contract passed"

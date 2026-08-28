#!/usr/bin/env ruby

require "pathname"
require "json"
require "uri"
require "yaml"

# Ruby 2.6's Psych can safely load one document but does not expose the stream
# variant. Use parser-provided document boundaries and safe-load each document.
unless YAML.respond_to?(:safe_load_stream)
  def YAML.safe_load_stream(source, aliases: false, **options)
    lines = source.lines
    Psych.parse_stream(source).children.map do |document|
      fragment = lines[document.start_line...document.end_line].join
      YAML.safe_load(fragment, aliases: aliases, **options)
    end
  end
end

ROOT = File.expand_path("../..", __dir__)
TRIGGER_KERNEL_ID = "codex-trigger-native-v1"
CONTROL_PLANE_NAMESPACE = "chrono-bot"
CONTROL_PLANE_SERVICE_ACCOUNT = "chrono-bot-ksa"
ORDINARY_SECRET_PROVIDER_CLASS = "cma-opensandbox-api-key"
TRIGGER_AUTHORITY_KEYS = [
  "CHRONO_TRIGGER_SANDBOX_BASE_URL",
  "CHRONO_TRIGGER_SANDBOX_NAMESPACE",
  "CHRONO_TRIGGER_SANDBOX_API_KEY_FILE",
].freeze

PATHS = {
  workflow: ".github/workflows/cma-trigger-provider-smoke.yml",
  smoke: ".github/scripts/cma-trigger-provider-smoke.mjs",
  example: ".github/cma-trigger-provider-smoke.example.json",
  configmap: "infra/api/configmap.yaml",
  deployment: "infra/api/deployment.yaml",
  secretproviderclass: "infra/api/secretproviderclass.yaml",
  serviceaccount: "infra/api/serviceaccount.yaml",
  trigger: "api/crates/cma/src/trigger/mod.rs",
  config: "api/crates/cma/src/config.rs",
  kernels: "api/crates/cma/src/kernels.rs",
  codex: "api/crates/layer3-adapter/src/codex/mod.rs",
  hosted: "api/crates/layer3-adapter/src/hosted/mod.rs",
  reclaimer: "api/crates/layer3-adapter/src/sandbox_reclaimer.rs",
}.freeze

class ContractError < StandardError; end

def check(condition, message)
  raise ContractError, message unless condition
end

def named_step(steps, name)
  steps.find { |step| step.is_a?(Hash) && step["name"] == name }
end

def contains_in_order?(source, needles)
  offset = 0
  needles.all? do |needle|
    found = source.index(needle, offset)
    next false unless found

    offset = found + needle.length
    true
  end
end

def source_matches(source, pattern, message)
  check(source.match?(pattern), message)
end

def load_inputs
  workflow_source = File.read(File.join(ROOT, PATHS.fetch(:workflow)))
  example_source = File.read(File.join(ROOT, PATHS.fetch(:example)))
  manifests = {}
  [:configmap, :deployment, :secretproviderclass, :serviceaccount].each do |name|
    source = File.read(File.join(ROOT, PATHS.fetch(name)))
    manifests[name] = YAML.safe_load_stream(source, aliases: true).compact
  end
  sources = {}
  [:smoke, :trigger, :config, :kernels, :codex, :hosted, :reclaimer].each do |name|
    sources[name] = File.read(File.join(ROOT, PATHS.fetch(name)))
  end
  {
    workflow: YAML.safe_load(workflow_source, aliases: true),
    example: JSON.parse(example_source),
    manifests: manifests,
    sources: sources,
  }
end

def documents(inputs, file, kind = nil)
  found = inputs.fetch(:manifests).fetch(file)
  kind ? found.select { |document| document["kind"] == kind } : found
end

def find_document(inputs, file, kind, name, namespace = nil)
  matches = documents(inputs, file, kind).select do |document|
    metadata = document.fetch("metadata", {})
    metadata["name"] == name &&
      (namespace.nil? || metadata["namespace"] == namespace)
  end
  check(
    matches.length == 1,
    "#{file} must contain exactly one #{kind} #{name.inspect}",
  )
  matches.first
end

def verify_example(example)
  expected_keys = [
    "base_url",
    "configuration_revision",
    "contributor",
    "expected_build_sha",
    "expected_runtime_version",
    "fixture_policy",
    "github_api_url",
    "public_id",
    "revision_digest",
    "runtime_image_digest",
    "schema_version",
    "source_sha",
    "task",
    "timeouts",
    "trigger_sandbox_namespace",
    "upstream",
  ]
  check(
    example.is_a?(Hash) && example.keys.sort == expected_keys,
    "provider example must retain its exact closed top-level schema",
  )
  check(
    dns1123_label?(example["trigger_sandbox_namespace"]),
    "provider example must bind one exact Trigger sandbox namespace",
  )
end

def verify_workflow(workflow)
  triggers = workflow.fetch(true)
  check(
    triggers.keys == ["workflow_dispatch"],
    "provider smoke must remain workflow_dispatch-only",
  )
  check(
    workflow.fetch("permissions") == {
      "actions" => "read",
      "contents" => "read",
    },
    "provider smoke must retain only the read permissions needed to inspect its environment and checkout source",
  )
  check(
    workflow.dig("concurrency", "cancel-in-progress") == false,
    "provider smoke runs must never cancel an in-flight writer",
  )

  jobs = workflow.fetch("jobs")
  plan = jobs.fetch("plan")
  plan_steps = plan.fetch("steps")
  consent = named_step(
    plan_steps,
    "Require the deployed develop workflow and preservation consent",
  )
  check(consent, "provider smoke must require explicit preservation consent")
  consent_run = consent.fetch("run")
  check(
    consent_run.include?('"refs/heads/develop"'),
    "provider smoke must run only from develop",
  )
  check(
    consent_run.include?('"PRESERVE"'),
    "provider smoke must retain exact preservation confirmation",
  )

  environment = named_step(plan_steps, "Require the live protected smoke environment")
  check(environment, "provider smoke must verify its live GitHub environment")
  environment_run = environment.fetch("run")
  [
    "environments/cma-trigger-smoke",
    ".can_admins_bypass == false",
    '.type == "required_reviewers"',
    ".prevent_self_review == true",
    ".deployment_branch_policy.protected_branches == true",
    ".deployment_branch_policy.custom_branch_policies == false",
  ].each do |contract|
    check(
      environment_run.include?(contract),
      "provider environment verification is missing #{contract}",
    )
  end

  provider = jobs.fetch("provider")
  check(provider.fetch("needs") == "plan", "provider writer must depend on plan")
  check(
    provider.fetch("environment") == "cma-trigger-smoke",
    "provider writer must remain behind cma-trigger-smoke",
  )
  check(
    provider.dig("strategy", "matrix").nil?,
    "provider smoke must not fan out external writers",
  )
  check(
    provider.fetch("outputs") == {
      "evidence_id" => "${{ steps.evidence_audit.outputs.evidence_id }}",
      "evidence_result" => "${{ steps.evidence_audit.outputs.evidence_result }}",
      "retained_artifact_url" => "${{ steps.evidence_upload.outputs.artifact-url }}",
    },
    "provider smoke must expose the closed evidence identity, result, and retained artifact URL",
  )
  provider_env = provider.fetch("env")
  expected_provider_env = {
    "CMA_TRIGGER_SMOKE_BASE_URL" => "${{ vars.CMA_TRIGGER_SMOKE_BASE_URL }}",
    "CMA_TRIGGER_SMOKE_EXPECTED_BUILD_SHA" => "${{ vars.CMA_TRIGGER_SMOKE_EXPECTED_BUILD_SHA }}",
    "CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_DIGEST" => "${{ vars.CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_DIGEST }}",
    "CMA_TRIGGER_SMOKE_EXPECTED_RUNTIME_VERSION" => "${{ vars.CMA_TRIGGER_SMOKE_EXPECTED_RUNTIME_VERSION }}",
    "CMA_TRIGGER_SMOKE_CONFIGURATION_REVISION" => "${{ vars.CMA_TRIGGER_SMOKE_CONFIGURATION_REVISION }}",
    "CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE" => "${{ vars.CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE }}",
    "CMA_TRIGGER_SMOKE_PUBLIC_ID" => "${{ vars.CMA_TRIGGER_SMOKE_PUBLIC_ID }}",
    "CMA_TRIGGER_SMOKE_REVISION_DIGEST" => "${{ vars.CMA_TRIGGER_SMOKE_REVISION_DIGEST }}",
    "CMA_TRIGGER_SMOKE_SOURCE_SHA" => "${{ vars.CMA_TRIGGER_SMOKE_SOURCE_SHA }}",
    "CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID" => "${{ vars.CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID }}",
    "CMA_TRIGGER_SMOKE_UPSTREAM_OWNER" => "${{ vars.CMA_TRIGGER_SMOKE_UPSTREAM_OWNER }}",
    "CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY" => "${{ vars.CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY }}",
    "CMA_TRIGGER_SMOKE_UPSTREAM_DEFAULT_BRANCH" => "${{ vars.CMA_TRIGGER_SMOKE_UPSTREAM_DEFAULT_BRANCH }}",
    "CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN" => "${{ vars.CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN }}",
    "CMA_TRIGGER_SMOKE_FORK_REPOSITORY" => "${{ vars.CMA_TRIGGER_SMOKE_FORK_REPOSITORY }}",
    "CMA_TRIGGER_SMOKE_MARKER_INPUT" => "${{ vars.CMA_TRIGGER_SMOKE_MARKER_INPUT }}",
    "CMA_TRIGGER_SMOKE_EXPECTED_PATH_TEMPLATE" => "${{ vars.CMA_TRIGGER_SMOKE_EXPECTED_PATH_TEMPLATE }}",
    "CMA_TRIGGER_SMOKE_EXPECTED_CONTENT_TEMPLATE" => "${{ vars.CMA_TRIGGER_SMOKE_EXPECTED_CONTENT_TEMPLATE }}",
    "CMA_TRIGGER_SMOKE_EXPECTED_FORK_MODE" => "${{ inputs.expected_fork_mode }}",
    "CMA_TRIGGER_SMOKE_RUN_MARKER" => "gha-${{ github.run_id }}-${{ github.run_attempt }}",
  }
  check(
    provider_env == expected_provider_env,
    "provider smoke must bind exactly the reviewed protected variables and run identity",
  )

  provider_steps = provider.fetch("steps")
  provider_recheck = named_step(
    provider_steps,
    "Revalidate the protected smoke environment after approval",
  )
  check(provider_recheck, "provider writer must revalidate its environment after approval")
  provider_recheck_run = provider_recheck.fetch("run")
  [
    "environments/cma-trigger-smoke",
    ".can_admins_bypass == false",
    '.type == "required_reviewers"',
    ".prevent_self_review == true",
    ".deployment_branch_policy.protected_branches == true",
    ".deployment_branch_policy.custom_branch_policies == false",
  ].each do |contract|
    check(
      provider_recheck_run.include?(contract),
      "post-approval environment verification is missing #{contract}",
    )
  end

  approval = named_step(provider_steps, "Capture independent approval provenance")
  check(approval, "provider smoke must capture the actual environment approval")
  check(approval["id"] == "approval", "provider approval capture must retain its stable step id")
  approval_run = approval.fetch("run")
  [
    'actions/runs/$GITHUB_RUN_ID/approvals',
    'users/$GITHUB_TRIGGERING_ACTOR',
    '.name == "cma-trigger-smoke"',
    "approval_observed_at=$(date -u +'%Y-%m-%dT%H:%M:%SZ')",
    'approval_observed_at: $approval_observed_at',
    'unique_by(.account_id)',
    '.account_id != $operator.account_id',
    'reviewers_json=$reviewers',
  ].each do |contract|
    check(approval_run.include?(contract), "provider approval provenance is missing #{contract}")
  end
  check(
    !approval_run.include?(".submitted_at"),
    "provider approval provenance must not invent a timestamp absent from GitHub review history",
  )

  kubectl_steps = provider_steps.select do |step|
    step.is_a?(Hash) && step["uses"] == "azure/setup-kubectl@v4"
  end
  check(
    kubectl_steps.length == 1 && kubectl_steps.first.fetch("with") == { "version" => "v1.34.1" },
    "provider smoke must install exactly the reviewed Kubernetes inventory client",
  )

  kubeconfig = named_step(provider_steps, "Materialize the scoped Kubernetes inventory credential")
  check(kubeconfig, "provider smoke must materialize its scoped inventory credential")
  check(
    kubeconfig.fetch("env") == {
      "KUBECONFIG_B64" => "${{ secrets.CMA_TRIGGER_SMOKE_KUBECONFIG_B64 }}",
      "KUBECONFIG_PATH" => "${{ runner.temp }}/cma-trigger-smoke-kubeconfig",
    },
    "provider inventory credential must come only from the protected base64 kubeconfig secret",
  )
  [
    "umask 077",
    'printf \'%s\' "$KUBECONFIG_B64" | base64 --decode > "$KUBECONFIG_PATH"',
    'chmod 0600 "$KUBECONFIG_PATH"',
    'test -s "$KUBECONFIG_PATH"',
  ].each do |contract|
    check(kubeconfig.fetch("run").include?(contract), "kubeconfig materialization is missing #{contract}")
  end

  inventory_authority = named_step(
    provider_steps,
    "Verify exact read-only BatchSandbox inventory authority",
  )
  check(inventory_authority, "provider smoke must verify its exact inventory authority")
  check(
    inventory_authority.fetch("env") == {
      "KUBECONFIG_PATH" => "${{ runner.temp }}/cma-trigger-smoke-kubeconfig",
      "SANDBOX_NAMESPACE" => "${{ vars.CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE }}",
    },
    "inventory preflight must bind only the temporary kubeconfig and protected namespace",
  )
  inventory_authority_run = inventory_authority.fetch("run")
  namespace_guard = [
    'if [[ ! "$SANDBOX_NAMESPACE" =~ ^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$ ]]; then',
    'echo "::error::CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE must be one nonempty DNS-1123 label"',
    "exit 1",
    "fi",
  ]
  check(
    contains_in_order?(inventory_authority_run, namespace_guard),
    "inventory authority preflight must reject an empty or non-DNS-1123 namespace before kubectl",
  )
  check(
    inventory_authority_run.index(namespace_guard.first) <
      inventory_authority_run.index("resource='batchsandboxes.sandbox.opensandbox.io'"),
    "inventory namespace validation must precede every kubectl authority probe",
  )
  [
    "resource='batchsandboxes.sandbox.opensandbox.io'",
    'list "$resource" --namespace "$SANDBOX_NAMESPACE"',
    "for verb in get watch create update patch delete deletecollection",
    'list "$resource" --all-namespaces',
    "for resource in secrets configmaps pods",
  ].each do |contract|
    check(
      inventory_authority_run.include?(contract),
      "inventory authority preflight is missing #{contract}",
    )
  end

  materialize = named_step(provider_steps, "Materialize the non-secret pinned fixture contract")
  check(materialize, "provider smoke must materialize a reviewed fixture contract")
  materialize_run = materialize.fetch("run")
  check(
    contains_in_order?(
      materialize_run,
      [
        '--arg build_sha "$CMA_TRIGGER_SMOKE_EXPECTED_BUILD_SHA"',
        '--arg runtime_image_digest "$CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_DIGEST"',
        '--arg runtime_version "$CMA_TRIGGER_SMOKE_EXPECTED_RUNTIME_VERSION"',
        '--arg configuration_revision "$CMA_TRIGGER_SMOKE_CONFIGURATION_REVISION"',
        '--arg trigger_sandbox_namespace "$CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE"',
        "schema_version: 2",
        "expected_build_sha: $build_sha",
        "runtime_image_digest: $runtime_image_digest",
        "expected_runtime_version: $runtime_version",
        "configuration_revision: $configuration_revision",
        "trigger_sandbox_namespace: $trigger_sandbox_namespace",
      ],
    ),
    "provider fixture must bind the exact build, runtime, configuration, and sandbox namespace",
  )

  run_smoke = named_step(provider_steps, "Run one exact provider path")
  check(run_smoke, "provider smoke must retain its one exact writer path")
  run_env = run_smoke.fetch("env")
  expected_run_env = {
    "CONFIG_PATH" => "${{ runner.temp }}/cma-trigger-smoke-config.json",
    "EVIDENCE_PATH" => "${{ runner.temp }}/cma-trigger-smoke-evidence.json",
    "CMA_TRIGGER_SMOKE_ACCESS_TOKEN" => "${{ secrets.CMA_TRIGGER_SMOKE_ACCESS_TOKEN }}",
    "CMA_TRIGGER_SMOKE_SERVICE_ID" => "${{ secrets.CMA_TRIGGER_SMOKE_SERVICE_ID }}",
    "CMA_TRIGGER_SMOKE_KUBECONFIG" => "${{ runner.temp }}/cma-trigger-smoke-kubeconfig",
    "CMA_TRIGGER_SMOKE_OPERATOR_LOGIN" => "${{ steps.approval.outputs.operator_login }}",
    "CMA_TRIGGER_SMOKE_OPERATOR_ACCOUNT_ID" => "${{ steps.approval.outputs.operator_account_id }}",
    "CMA_TRIGGER_SMOKE_REVIEWERS_JSON" => "${{ steps.approval.outputs.reviewers_json }}",
  }
  check(
    run_env == expected_run_env,
    "provider writer must bind exactly the reviewed credentials, evidence paths, and actors",
  )

  audit = named_step(provider_steps, "Audit and validate closed provider evidence")
  check(audit, "provider smoke must always audit its closed evidence")
  remove_kubeconfig = named_step(provider_steps, "Remove the Kubernetes inventory credential")
  check(remove_kubeconfig, "provider smoke must remove its inventory credential")
  check(
    remove_kubeconfig["if"].to_s.include?("always()") &&
      remove_kubeconfig.fetch("env") == {
        "KUBECONFIG_PATH" => "${{ runner.temp }}/cma-trigger-smoke-kubeconfig",
      } &&
      remove_kubeconfig.fetch("run").include?('rm -- "$KUBECONFIG_PATH"'),
    "provider smoke must always remove exactly the temporary kubeconfig before evidence handling",
  )
  check(
    provider_steps.index(remove_kubeconfig) < provider_steps.index(audit),
    "provider smoke must remove the kubeconfig before auditing or uploading evidence",
  )

  check(audit["id"] == "evidence_audit", "provider evidence audit must retain its stable step id")
  check(audit["if"].to_s.include?("always()"), "provider evidence audit must run after failure")
  check(
    audit.fetch("env") == {
      "EVIDENCE_PATH" => "${{ runner.temp }}/cma-trigger-smoke-evidence.json",
      "CMA_TRIGGER_SMOKE_ACCESS_TOKEN" => "${{ secrets.CMA_TRIGGER_SMOKE_ACCESS_TOKEN }}",
      "CMA_TRIGGER_SMOKE_SERVICE_ID" => "${{ secrets.CMA_TRIGGER_SMOKE_SERVICE_ID }}",
    },
    "provider evidence audit must receive no Kubernetes credential or unreviewed input",
  )
  audit_run = audit.fetch("run")
  check(
    contains_in_order?(
      audit_run,
      [
        '--audit-evidence "$EVIDENCE_PATH"',
        '--validate-evidence "$EVIDENCE_PATH"',
        "evidence_id=$(jq -r '.evidence_id'",
        "evidence_result=$(jq -r '.result'",
        "artifact_name=$(jq -r '.provenance.workflow.artifact_name'",
      ],
    ),
    "provider evidence must pass redaction and publishable-schema validation before outputs",
  )

  upload = named_step(provider_steps, "Upload redacted provider evidence")
  check(upload, "provider smoke must retain its evidence upload")
  check(upload["id"] == "evidence_upload", "provider evidence upload must expose artifact outputs")
  check(
    upload["if"].to_s.include?("steps.evidence_audit.outcome == 'success'"),
    "provider evidence upload must fail closed on audit or schema rejection",
  )
  check(upload["uses"] == "actions/upload-artifact@v4", "provider evidence must use artifact v4")
  check(
    upload.fetch("with") == {
      "name" => "${{ steps.evidence_audit.outputs.artifact_name }}",
      "path" => "${{ runner.temp }}/cma-trigger-smoke-evidence.json",
      "if-no-files-found" => "error",
      "retention-days" => 30,
    },
    "provider evidence upload must retain only its validated exact file for 30 days",
  )
end

def verify_sources(sources)
  trigger = sources.fetch(:trigger)
  definitions = trigger.scan(
    /\bconst TRIGGER_KERNEL_ID: &str = "([^"]+)";/,
  ).flatten
  check(
    definitions == [TRIGGER_KERNEL_ID],
    "Trigger kernel source must define exactly #{TRIGGER_KERNEL_ID.inspect}",
  )

  smoke = sources.fetch(:smoke)
  check(
    smoke.scan(/const TRIGGER_KERNEL_ID = "([^"]+)";/).flatten == [TRIGGER_KERNEL_ID],
    "provider smoke must own the exact Trigger kernel constant",
  )
  [
    "schema_version: 3",
    "expected_kernel: TRIGGER_KERNEL_ID",
    "observed_kernel: null",
    "deployment.expected_kernel !== TRIGGER_KERNEL_ID",
    "deployment.observed_kernel !== deployment.expected_kernel",
    "sessionCleanupStages.length !== 1",
    'stage.name === "session_cleanup"',
    "raw.stages[raw.stages.length - 1] !== sessionCleanupStages[0]",
    'raw.cleanup.session !== "idle_and_preserved"',
    'const BATCH_SANDBOX_RESOURCE = "batchsandboxes.sandbox.opensandbox.io"',
    '"trigger_sandbox_namespace"',
    '"CMA_TRIGGER_SMOKE_KUBECONFIG"',
    '"kubernetes_credentials"',
    '"runtime_reservation_sha256"',
    '"--chunk-size=0"',
  ].each do |contract|
    check(smoke.include?(contract), "provider smoke source is missing #{contract}")
  end
  source_matches(
    smoke,
    /assertEqual\(\s*launch\.runtime_admission\.kernel,\s*TRIGGER_KERNEL_ID,\s*"trigger_kernel_mismatch"/m,
    "provider smoke must assert the reviewed Trigger kernel",
  )
  source_matches(
    smoke,
    /evidence\.document\.provenance\.deployment\.observed_kernel\s*=\s*launch\.runtime_admission\.kernel/,
    "provider smoke must record the reviewed Trigger kernel in evidence",
  )
  source_matches(
    smoke,
    /client\.readSession\(\s*launch\.session\.session_id,?\s*\).*?assertEqual\(\s*createdSession\.id,\s*launch\.session\.session_id.*?assertEqual\(\s*createdSession\.kernel,\s*TRIGGER_KERNEL_ID/m,
    "provider smoke must read back the created session identity and kernel",
  )
  source_matches(
    smoke,
    /client\.stopSession\(sessionId\).*?client\.readSession\(sessionId\).*?assertEqual\(\s*stopped\.id,\s*sessionId.*?assertEqual\(\s*stopped\.kernel,\s*TRIGGER_KERNEL_ID.*?assertEqual\(\s*stopped\.lifecycle,\s*"idle"/m,
    "provider smoke must prove the stopped session identity, kernel, and idle lifecycle",
  )
  source_matches(
    smoke,
    /currentStage = "sandbox_inventory_before_launch".*?phase: "before_launch".*?sandbox_inventory\.observations\.before_launch\s*=\s*inventory\.observation/m,
    "provider smoke must retain authoritative inventory before Launch creation",
  )
  source_matches(
    smoke,
    /currentStage = "sandbox_inventory_live_session".*?phase: "live_session".*?sandbox_inventory\.observations\.live_session\s*=\s*inventory\.observation/m,
    "provider smoke must retain authoritative inventory for the live session",
  )
  source_matches(
    smoke,
    /client\.stopSession\(sessionId\).*?phase: "after_stop".*?sandbox_inventory\.observations\.after_stop\s*=\s*inventory\.observation/m,
    "provider smoke must retain authoritative inventory after session stop",
  )
  source_matches(
    smoke,
    /document\.counts\.sandbox\s*=\s*document\.sandbox_inventory\.observations\.live_session\.entitled_objects\.length/,
    "provider smoke must derive the sandbox count from the live entitled inventory",
  )
  check(
    !smoke.match?(/counts\.sandbox\s*=\s*1\b/),
    "provider smoke must never fabricate one sandbox from session state",
  )
  source_matches(
    smoke,
    /sandboxBefore\.total_objects === 0.*?sandboxLive\.total_objects === 1.*?sandboxAfter\.total_objects === 0/m,
    "publishable evidence must enforce the authoritative 0/1/0 sandbox inventory",
  )

  config = sources.fetch(:config)
  source_matches(
    config,
    /let trigger_sandbox_base_url = strict_env\("CHRONO_TRIGGER_SANDBOX_BASE_URL"\)\?/,
    "CMA config must read the exact Trigger provider base URL key",
  )
  source_matches(
    config,
    /let trigger_sandbox_namespace = strict_env\("CHRONO_TRIGGER_SANDBOX_NAMESPACE"\)\?/,
    "CMA config must read the exact Trigger provider namespace key",
  )
  source_matches(
    config,
    /let trigger_sandbox_api_key = optional_file_secret\(\s*"CHRONO_TRIGGER_SANDBOX_API_KEY_FILE",\s*"CHRONO_TRIGGER_SANDBOX_API_KEY",\s*\)\?/m,
    "CMA config must read the exact Trigger provider key-file authority",
  )
  [
    "ordinary_base_url: &str",
    "ordinary_api_key: &str",
    "same_http_origin(&parsed, ordinary_base_url)",
    "api_key == ordinary_api_key",
    "forbidden_inline_key} is not accepted",
  ].each do |contract|
    check(config.include?(contract), "CMA config is missing provider separation: #{contract}")
  end
  [
    "match (base_url, namespace, api_key)",
    "(Some(base_url), Some(namespace), Some(api_key))",
    "(None, None, None) if !trigger_enabled",
    "(None, None, None) => Err(",
    "_ => Err(",
  ].each do |contract|
    check(config.include?(contract), "CMA config is missing all-or-none authority: #{contract}")
  end

  kernels = sources.fetch(:kernels)
  check(
    contains_in_order?(
      kernels,
      [
        "config.trigger_sandbox_provider.as_ref()",
        "provider_namespaces_are_separate(",
        "Some(ordinary_reclaimer.namespace())",
        ".in_namespace(&provider.namespace)",
        ".for_kernel(crate::trigger::TRIGGER_KERNEL_ID)",
        ".preflight_exclusive_cleanup_authority()",
        "config.trigger_sandbox_provider.as_ref()",
        "sandbox_client_from_parts(",
        "&provider.base_url",
        "&provider.api_key",
        "Some(reclaimer)",
        "KernelId::new(crate::trigger::TRIGGER_KERNEL_ID)",
        "CodexAdapter::named(",
        "crate::trigger::TRIGGER_KERNEL_ID",
        "trigger_lane.client",
      ],
    ),
    "kernel registry must build the dedicated provider and kernel-bound reclaimer",
  )

  codex = sources.fetch(:codex)
  source_matches(
    codex,
    /pub fn named\(.*?HostedKernel::new_strict\(.*?kernel,/m,
    "named Codex lanes must use strict kernel metadata",
  )
  hosted = sources.fetch(:hosted)
  source_matches(
    hosted,
    /pub fn new_strict\(.*?Self::new_with_kernel_metadata\(.*?true\)/m,
    "hosted strict construction must enable strict kernel metadata",
  )
  [
    "(Some(canonical), Some(legacy)) => canonical == self.kernel && legacy == self.kernel",
    "(Some(canonical), None) => canonical == self.kernel",
    "(None, Some(legacy)) => !self.strict_kernel_metadata && legacy == self.kernel",
    "(None, None) => !self.strict_kernel_metadata",
  ].each do |contract|
    check(hosted.include?(contract), "hosted kernel ownership is missing #{contract}")
  end

  reclaimer = sources.fetch(:reclaimer)
  check(
    contains_in_order?(
      reclaimer,
      [
        "pub async fn infer_namespaced(namespace: impl Into<String>)",
        "Self::from_config(config, namespace)",
        "Api::namespaced_with(client.clone(), &namespace, &resource)",
        "pub fn for_kernel",
        "self.expected_kernel = Some(kernel.into())",
        "if let Some(kernel) = self.expected_kernel.as_ref()",
        "KERNEL_METADATA_KEY.into()",
        "kernel.clone()",
      ],
    ),
    "sandbox reclaimer must retain the configured namespace and kernel selector",
  )
  [
    "Some(self.namespace.as_str())",
    "self.verify_kernel(labels, &identity.handle)?",
    "self.verify_kernel(labels, &handle)?",
    "pub async fn preflight_cleanup_authority(&self)",
    "for verb in [\"get\", \"list\", \"delete\"]",
    ".list(&ListParams::default().limit(1))",
    "require_label(labels, KERNEL_METADATA_KEY, kernel, handle)?",
    "LEGACY_KERNEL_METADATA_KEY",
    "has conflicting canonical and legacy kernel metadata",
    "pub async fn preflight_exclusive_cleanup_authority(&self)",
    "[\"watch\", \"create\", \"update\", \"patch\", \"deletecollection\"]",
    "[\"secrets\", \"configmaps\", \"pods\"]",
    "unexpectedly has cluster-wide {verb} authority",
  ].each do |contract|
    check(reclaimer.include?(contract), "sandbox reclaimer is missing #{contract}")
  end
end

def absolute_clean_path?(value)
  return false unless value.is_a?(String) && !value.empty?

  path = Pathname.new(value)
  path.absolute? && path.cleanpath.to_s == value && File.basename(value) != "/"
end

def root_http_origin?(value)
  uri = URI.parse(value)
  uri.is_a?(URI::HTTP) &&
    !uri.host.nil? &&
    !uri.host.empty? &&
    uri.userinfo.nil? &&
    uri.query.nil? &&
    uri.fragment.nil? &&
    ["", "/"].include?(uri.path)
rescue URI::InvalidURIError
  false
end

def same_http_origin?(left, right)
  left = URI.parse(left)
  right = URI.parse(right)
  left.scheme&.downcase == right.scheme&.downcase &&
    left.host&.downcase == right.host&.downcase &&
    left.port == right.port
rescue URI::InvalidURIError
  false
end

def dns1123_label?(value)
  value.is_a?(String) &&
    value.length <= 63 &&
    value.match?(/\A[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\z/)
end

def secret_entries(secret_provider_class, label)
  check(
    secret_provider_class.dig("spec", "provider") == "gke",
    "#{label} SecretProviderClass must use provider gke",
  )
  raw = secret_provider_class.dig("spec", "parameters", "secrets")
  check(raw.is_a?(String) && !raw.empty?, "#{label} SecretProviderClass must declare secrets")
  begin
    entries = YAML.safe_load(raw, aliases: false)
  rescue Psych::Exception
    raise ContractError, "#{label} SecretProviderClass secrets must be valid YAML"
  end
  check(entries.is_a?(Array) && !entries.empty?, "#{label} SecretProviderClass secrets must be a list")
  entries.each do |entry|
    check(entry.is_a?(Hash), "#{label} SecretProviderClass secret entry must be an object")
    check(
      entry["resourceName"].is_a?(String) && !entry["resourceName"].empty?,
      "#{label} SecretProviderClass secret must name a resource",
    )
    check(
      entry["path"].is_a?(String) && !entry["path"].empty?,
      "#{label} SecretProviderClass secret must expose a path",
    )
  end
  entries
end

def resolved_mount_file(mount, exposed_path)
  mount_path = mount["mountPath"]
  return nil unless absolute_clean_path?(mount_path)

  sub_path = mount["subPath"]
  return File.join(mount_path, exposed_path) if sub_path.nil?
  return mount_path if sub_path == exposed_path

  nil
end

def secret_resource_identity(resource_name)
  resource_name.sub(%r{/versions/[^/]+\z}, "")
end

def exact_reclaimer_role?(role)
  rules = role["rules"]
  return false unless rules.is_a?(Array) && rules.length == 1

  rule = rules.first
  return false unless rule.is_a?(Hash)
  return false unless rule.keys.sort == ["apiGroups", "resources", "verbs"]

  rule["apiGroups"] == ["sandbox.opensandbox.io"] &&
    rule["resources"] == ["batchsandboxes"] &&
    rule["verbs"].is_a?(Array) &&
    rule["verbs"].sort == ["delete", "get", "list"] &&
    rule["verbs"].uniq.length == 3
end

def binding_targets_role?(binding, role_name)
  role_ref = binding["roleRef"]
  subjects = binding["subjects"]
  role_ref == {
    "apiGroup" => "rbac.authorization.k8s.io",
    "kind" => "Role",
    "name" => role_name,
  } &&
    subjects == [
      {
        "kind" => "ServiceAccount",
        "name" => CONTROL_PLANE_SERVICE_ACCOUNT,
        "namespace" => CONTROL_PLANE_NAMESPACE,
      },
    ]
end

def binding_targets_control_plane?(binding)
  subjects = binding["subjects"]
  subjects.is_a?(Array) && subjects.any? do |subject|
    next false unless subject.is_a?(Hash)

    case subject["kind"]
    when "ServiceAccount"
      subject["name"] == CONTROL_PLANE_SERVICE_ACCOUNT &&
        subject["namespace"] == CONTROL_PLANE_NAMESPACE
    when "User"
      subject["name"] ==
        "system:serviceaccount:#{CONTROL_PLANE_NAMESPACE}:#{CONTROL_PLANE_SERVICE_ACCOUNT}"
    when "Group"
      [
        "system:authenticated",
        "system:serviceaccounts",
        "system:serviceaccounts:#{CONTROL_PLANE_NAMESPACE}",
      ].include?(subject["name"])
    else
      false
    end
  end
end

def rule_grants_batchsandbox_authority?(rule)
  return false unless rule.is_a?(Hash)

  groups = rule["apiGroups"]
  resources = rule["resources"]
  verbs = rule["verbs"]
  groups.is_a?(Array) &&
    groups.any? { |group| ["*", "sandbox.opensandbox.io"].include?(group) } &&
    resources.is_a?(Array) &&
    resources.any? do |resource|
      resource == "*" || resource == "batchsandboxes" || resource.start_with?("batchsandboxes/")
    end &&
    verbs.is_a?(Array) &&
    !verbs.empty?
end

def role_grants_batchsandbox_authority?(role)
  rules = role["rules"]
  rules.is_a?(Array) && rules.any? { |rule| rule_grants_batchsandbox_authority?(rule) }
end

def verify_adoption_ready(inputs, base_url, namespace, key_file)
  check(root_http_origin?(base_url), "Trigger provider base URL must be an absolute root HTTP(S) origin")
  check(dns1123_label?(namespace), "Trigger provider namespace must be one lowercase DNS-1123 label")
  check(absolute_clean_path?(key_file), "Trigger provider key file must be one absolute normalized path")

  configmap = find_document(
    inputs,
    :configmap,
    "ConfigMap",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  )
  ordinary_base_url = configmap.fetch("data").fetch("CHRONO_SANDBOX_BASE_URL")
  check(
    !same_http_origin?(base_url, ordinary_base_url),
    "Trigger provider origin must differ from the ordinary sandbox provider origin",
  )
  check(
    namespace != CONTROL_PLANE_NAMESPACE,
    "Trigger workload namespace must differ from the ordinary sandbox namespace",
  )

  deployment = find_document(
    inputs,
    :deployment,
    "Deployment",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  )
  pod_spec = deployment.dig("spec", "template", "spec")
  check(pod_spec.is_a?(Hash), "cma-api Deployment must contain a pod spec")
  check(
    pod_spec["serviceAccountName"] == CONTROL_PLANE_SERVICE_ACCOUNT,
    "adoption-ready deployment must use chrono-bot-ksa",
  )
  check(
    pod_spec["automountServiceAccountToken"] == true,
    "adoption-ready deployment must mount its service-account token",
  )
  service_account = documents(inputs, :serviceaccount, "ServiceAccount").select do |document|
    document.dig("metadata", "name") == CONTROL_PLANE_SERVICE_ACCOUNT &&
      document.dig("metadata", "namespace") == CONTROL_PLANE_NAMESPACE
  end
  check(service_account.length == 1, "chrono-bot-ksa must be declared exactly once")

  containers = pod_spec["containers"]
  check(containers.is_a?(Array), "cma-api Deployment must declare containers")
  api_containers = containers.select { |container| container["name"] == "api" }
  check(api_containers.length == 1, "cma-api Deployment must contain exactly one api container")
  mounts = api_containers.first.fetch("volumeMounts", [])
  volumes = pod_spec.fetch("volumes", [])
  check(mounts.is_a?(Array) && volumes.is_a?(Array), "deployment volumes and mounts must be lists")
  volume_names = volumes.map { |volume| volume["name"] }
  check(volume_names.compact.uniq.length == volume_names.length, "deployment volume names must be unique")
  volume_by_name = volumes.each_with_object({}) do |volume, found|
    found[volume["name"]] = volume
  end

  classes = documents(inputs, :secretproviderclass, "SecretProviderClass")
  ordinary_classes = classes.select do |candidate|
    candidate.dig("metadata", "name") == ORDINARY_SECRET_PROVIDER_CLASS &&
      candidate.dig("metadata", "namespace") == CONTROL_PLANE_NAMESPACE
  end
  check(ordinary_classes.length == 1, "ordinary OpenSandbox SecretProviderClass must remain unique")
  ordinary_entries = secret_entries(ordinary_classes.first, "ordinary OpenSandbox")
  ordinary_resources = ordinary_entries.map do |entry|
    secret_resource_identity(entry["resourceName"])
  end
  ordinary_paths = ordinary_entries.map { |entry| entry["path"] }
  ordinary_volume_names = volumes.select do |volume|
    volume.dig("csi", "volumeAttributes", "secretProviderClass") ==
      ORDINARY_SECRET_PROVIDER_CLASS
  end.map { |volume| volume["name"] }
  ordinary_mounts = mounts.select { |mount| ordinary_volume_names.include?(mount["name"]) }
  ordinary_mount_paths = ordinary_mounts.map { |mount| mount["mountPath"] }
  ordinary_files = ordinary_mounts.flat_map do |mount|
    ordinary_paths.map { |path| resolved_mount_file(mount, path) }
  end.compact

  expected_path = File.basename(key_file)
  lane_bindings = []
  mounts.each do |mount|
    next unless mount.is_a?(Hash) && mount["readOnly"] == true

    volume = volume_by_name[mount["name"]]
    next unless volume.is_a?(Hash)
    csi = volume["csi"]
    next unless csi.is_a?(Hash)
    next unless csi["driver"] == "secrets-store-gke.csi.k8s.io"
    next unless csi["readOnly"] == true

    class_name = csi.dig("volumeAttributes", "secretProviderClass")
    next unless class_name.is_a?(String) && !class_name.empty?
    next if class_name == ORDINARY_SECRET_PROVIDER_CLASS
    matching_classes = classes.select do |candidate|
      candidate.dig("metadata", "name") == class_name &&
        candidate.dig("metadata", "namespace") == CONTROL_PLANE_NAMESPACE
    end
    next unless matching_classes.length == 1

    begin
      entries = secret_entries(matching_classes.first, "Trigger provider")
    rescue ContractError
      next
    end
    next unless entries.length == 1
    entry = entries.first
    next unless entry["path"] == expected_path
    next unless resolved_mount_file(mount, expected_path) == key_file
    next if ordinary_resources.include?(secret_resource_identity(entry["resourceName"]))
    next if ordinary_paths.include?(entry["path"])
    next if ordinary_files.include?(key_file)
    next if ordinary_volume_names.include?(mount["name"])
    next if ordinary_mount_paths.include?(mount["mountPath"])

    lane_bindings << {
      mount: mount,
      volume: volume,
      secret_provider_class: matching_classes.first,
    }
  end
  check(
    lane_bindings.length == 1,
    "adoption-ready deployment must bind exactly one dedicated read-only Trigger CSI key file",
  )

  provider_roles = documents(inputs, :serviceaccount, "Role").select do |role|
    role_grants_batchsandbox_authority?(role)
  end
  unexpected_provider_roles = provider_roles.reject do |role|
    [CONTROL_PLANE_NAMESPACE, namespace].include?(role.dig("metadata", "namespace"))
  end
  check(
    unexpected_provider_roles.empty?,
    "checked-in BatchSandbox Roles must remain in the ordinary or exact Trigger namespace",
  )
  roles = provider_roles.select do |role|
    role.dig("metadata", "namespace") == namespace
  end
  check(
    roles.length == 1 && exact_reclaimer_role?(roles.first),
    "Trigger workload namespace must contain exactly one closed BatchSandbox reclaimer Role",
  )
  role_name = roles.first.dig("metadata", "name")
  check(role_name.is_a?(String) && !role_name.empty?, "Trigger reclaimer Role must have a name")
  bindings = documents(inputs, :serviceaccount, "RoleBinding").select do |binding|
    binding.dig("metadata", "namespace") == namespace &&
      binding_targets_control_plane?(binding)
  end
  check(
    bindings.length == 1 && binding_targets_role?(bindings.first, role_name),
    "Trigger workload Role must bind exactly to chrono-bot/chrono-bot-ksa",
  )
  provider_cluster_roles = documents(inputs, :serviceaccount, "ClusterRole").select do |role|
    role_grants_batchsandbox_authority?(role)
  end
  check(
    provider_cluster_roles.empty?,
    "Trigger provider authority must not be expressed by a checked-in ClusterRole",
  )
  cluster_bindings = documents(inputs, :serviceaccount, "ClusterRoleBinding").select do |binding|
    binding_targets_control_plane?(binding)
  end
  check(
    cluster_bindings.empty?,
    "chrono-bot/chrono-bot-ksa must not receive a checked-in ClusterRoleBinding",
  )
end

def verify_provider_manifests(inputs)
  configmap = find_document(
    inputs,
    :configmap,
    "ConfigMap",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  )
  data = configmap["data"]
  check(data.is_a?(Hash), "cma-api ConfigMap must contain data")
  TRIGGER_AUTHORITY_KEYS.each do |key|
    check(data.key?(key) && data[key].is_a?(String), "ConfigMap must declare #{key} as text")
  end
  enabled = data["CHRONO_TRIGGER_ENABLED"]
  check(["true", "false"].include?(enabled), "CHRONO_TRIGGER_ENABLED must be exactly true or false")

  authority = TRIGGER_AUTHORITY_KEYS.map { |key| data.fetch(key) }
  populated = authority.count { |value| !value.empty? }
  if populated.zero?
    check(enabled == "false", "Trigger cannot be enabled without complete provider authority")
    return :unprovisioned
  end
  check(
    populated == TRIGGER_AUTHORITY_KEYS.length,
    "Trigger provider base URL, namespace, and key file must be empty or populated together",
  )
  verify_adoption_ready(inputs, authority[0], authority[1], authority[2])
  :adoption_ready
end

def verify_contract(inputs)
  verify_example(inputs.fetch(:example))
  verify_workflow(inputs.fetch(:workflow))
  verify_sources(inputs.fetch(:sources))
  verify_provider_manifests(inputs)
end

def deep_copy(value)
  Marshal.load(Marshal.dump(value))
end

def adoption_ready_fixture(base)
  inputs = deep_copy(base)
  configmap = find_document(
    inputs,
    :configmap,
    "ConfigMap",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  )
  configmap["data"]["CHRONO_TRIGGER_SANDBOX_BASE_URL"] =
    "https://trigger-provider.example"
  configmap["data"]["CHRONO_TRIGGER_SANDBOX_NAMESPACE"] = "trigger-workloads"
  configmap["data"]["CHRONO_TRIGGER_SANDBOX_API_KEY_FILE"] =
    "/var/run/secrets/cma-trigger/provider-api-key"

  deployment = find_document(
    inputs,
    :deployment,
    "Deployment",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  )
  pod_spec = deployment.dig("spec", "template", "spec")
  api = pod_spec.fetch("containers").find { |container| container["name"] == "api" }
  test_lane_volumes = pod_spec.fetch("volumes").select do |volume|
    csi = volume["csi"]
    csi.is_a?(Hash) &&
      csi["driver"] == "secrets-store-gke.csi.k8s.io" &&
      csi.dig("volumeAttributes", "secretProviderClass") !=
        ORDINARY_SECRET_PROVIDER_CLASS
  end
  test_lane_volume_names = (
    test_lane_volumes.map { |volume| volume["name"] } + ["trigger-provider-api-key"]
  ).uniq
  test_lane_class_names = (test_lane_volumes.map do |volume|
    volume.dig("csi", "volumeAttributes", "secretProviderClass")
  end + ["cma-trigger-provider-api-key"]).uniq
  api.fetch("volumeMounts").reject! do |mount|
    test_lane_volume_names.include?(mount["name"])
  end
  pod_spec.fetch("volumes").reject! { |volume| test_lane_volume_names.include?(volume["name"]) }
  inputs.dig(:manifests, :secretproviderclass).reject! do |document|
    document["kind"] == "SecretProviderClass" &&
      test_lane_class_names.include?(document.dig("metadata", "name")) &&
      document.dig("metadata", "namespace") == CONTROL_PLANE_NAMESPACE
  end
  inputs.dig(:manifests, :serviceaccount).reject! do |document|
    ["Role", "RoleBinding"].include?(document["kind"]) &&
      document.dig("metadata", "namespace") == "trigger-workloads"
  end
  api.fetch("volumeMounts") << {
    "name" => "trigger-provider-api-key",
    "mountPath" => "/var/run/secrets/cma-trigger",
    "readOnly" => true,
  }
  pod_spec.fetch("volumes") << {
    "name" => "trigger-provider-api-key",
    "csi" => {
      "driver" => "secrets-store-gke.csi.k8s.io",
      "readOnly" => true,
      "volumeAttributes" => {
        "secretProviderClass" => "cma-trigger-provider-api-key",
      },
    },
  }
  inputs.dig(:manifests, :secretproviderclass) << {
    "apiVersion" => "secrets-store.csi.x-k8s.io/v1",
    "kind" => "SecretProviderClass",
    "metadata" => {
      "name" => "cma-trigger-provider-api-key",
      "namespace" => CONTROL_PLANE_NAMESPACE,
    },
    "spec" => {
      "provider" => "gke",
      "parameters" => {
        "secrets" => <<~YAML,
          - resourceName: "projects/example-project/secrets/example-trigger-provider-api-key/versions/latest"
            path: "provider-api-key"
        YAML
      },
    },
  }
  inputs.dig(:manifests, :serviceaccount) << {
    "apiVersion" => "rbac.authorization.k8s.io/v1",
    "kind" => "Role",
    "metadata" => {
      "name" => "cma-trigger-batchsandbox-reclaimer",
      "namespace" => "trigger-workloads",
    },
    "rules" => [
      {
        "apiGroups" => ["sandbox.opensandbox.io"],
        "resources" => ["batchsandboxes"],
        "verbs" => ["get", "list", "delete"],
      },
    ],
  }
  inputs.dig(:manifests, :serviceaccount) << {
    "apiVersion" => "rbac.authorization.k8s.io/v1",
    "kind" => "RoleBinding",
    "metadata" => {
      "name" => "cma-trigger-batchsandbox-reclaimer",
      "namespace" => "trigger-workloads",
    },
    "subjects" => [
      {
        "kind" => "ServiceAccount",
        "name" => CONTROL_PLANE_SERVICE_ACCOUNT,
        "namespace" => CONTROL_PLANE_NAMESPACE,
      },
    ],
    "roleRef" => {
      "apiGroup" => "rbac.authorization.k8s.io",
      "kind" => "Role",
      "name" => "cma-trigger-batchsandbox-reclaimer",
    },
  }
  inputs
end

def expect_rejected(name, inputs)
  begin
    verify_contract(inputs)
  rescue ContractError
    return
  end
  raise "provider contract self-test mutation was accepted: #{name}"
end

def self_test(base)
  checked_in_state = verify_contract(base)
  check(
    [:unprovisioned, :adoption_ready].include?(checked_in_state),
    "checked-in provider lane is not in a closed deployment state",
  )
  unprovisioned = deep_copy(base)
  unprovisioned_config = find_document(
    unprovisioned,
    :configmap,
    "ConfigMap",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  ).fetch("data")
  TRIGGER_AUTHORITY_KEYS.each { |key| unprovisioned_config[key] = "" }
  unprovisioned_config["CHRONO_TRIGGER_ENABLED"] = "false"
  check(
    verify_contract(unprovisioned) == :unprovisioned,
    "unprovisioned fixture was rejected",
  )

  adoption = adoption_ready_fixture(unprovisioned)
  check(verify_contract(adoption) == :adoption_ready, "adoption-ready fixture was rejected")
  enabled_adoption = deep_copy(adoption)
  find_document(
    enabled_adoption,
    :configmap,
    "ConfigMap",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  ).fetch("data")["CHRONO_TRIGGER_ENABLED"] = "true"
  check(
    verify_contract(enabled_adoption) == :adoption_ready,
    "complete enabled provider authority was rejected",
  )

  mutations = []

  missing_example_namespace = deep_copy(base)
  missing_example_namespace.fetch(:example).delete("trigger_sandbox_namespace")
  mutations << ["missing example sandbox namespace", missing_example_namespace]

  extended_example_schema = deep_copy(base)
  extended_example_schema.fetch(:example)["kubeconfig"] = "forbidden"
  mutations << ["extended example schema", extended_example_schema]

  missing_protected_namespace = deep_copy(base)
  missing_protected_namespace.dig(:workflow, "jobs", "provider", "env")
    .delete("CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE")
  mutations << ["missing protected sandbox namespace", missing_protected_namespace]

  missing_inventory_namespace_guard = deep_copy(base)
  inventory_run = named_step(
    missing_inventory_namespace_guard.dig(:workflow, "jobs", "provider", "steps"),
    "Verify exact read-only BatchSandbox inventory authority",
  ).fetch("run")
  removed_guard = inventory_run.sub!(
    'if [[ ! "$SANDBOX_NAMESPACE" =~ ^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$ ]]; then' + "\n",
    "",
  )
  check(removed_guard, "namespace-guard removal mutation did not change the fixture")
  mutations << ["missing inventory namespace guard", missing_inventory_namespace_guard]

  weakened_inventory_namespace_guard = deep_copy(base)
  inventory_step = named_step(
    weakened_inventory_namespace_guard.dig(:workflow, "jobs", "provider", "steps"),
    "Verify exact read-only BatchSandbox inventory authority",
  )
  inventory_step["run"] = inventory_step.fetch("run").sub(
    '^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$',
    '.+',
  )
  mutations << ["weakened inventory namespace guard", weakened_inventory_namespace_guard]

  extended_provider_env = deep_copy(base)
  extended_provider_env.dig(:workflow, "jobs", "provider", "env")["UNREVIEWED"] = "value"
  mutations << ["extended provider environment", extended_provider_env]

  wrong_kubeconfig_secret = deep_copy(base)
  named_step(
    wrong_kubeconfig_secret.dig(:workflow, "jobs", "provider", "steps"),
    "Materialize the scoped Kubernetes inventory credential",
  ).fetch("env")["KUBECONFIG_B64"] = "${{ secrets.OTHER_KUBECONFIG }}"
  mutations << ["wrong kubeconfig secret binding", wrong_kubeconfig_secret]

  missing_run_kubeconfig = deep_copy(base)
  named_step(
    missing_run_kubeconfig.dig(:workflow, "jobs", "provider", "steps"),
    "Run one exact provider path",
  ).fetch("env").delete("CMA_TRIGGER_SMOKE_KUBECONFIG")
  mutations << ["missing smoke kubeconfig path", missing_run_kubeconfig]

  extended_run_env = deep_copy(base)
  named_step(
    extended_run_env.dig(:workflow, "jobs", "provider", "steps"),
    "Run one exact provider path",
  ).fetch("env")["RAW_KUBECONFIG"] = "${{ secrets.CMA_TRIGGER_SMOKE_KUBECONFIG_B64 }}"
  mutations << ["extended smoke environment", extended_run_env]

  skipped_kubeconfig_removal = deep_copy(base)
  named_step(
    skipped_kubeconfig_removal.dig(:workflow, "jobs", "provider", "steps"),
    "Remove the Kubernetes inventory credential",
  )["if"] = "success()"
  mutations << ["non-always kubeconfig removal", skipped_kubeconfig_removal]

  widened_inventory_resource = deep_copy(base)
  replaced = widened_inventory_resource.dig(:sources, :smoke).sub!(
    'const BATCH_SANDBOX_RESOURCE = "batchsandboxes.sandbox.opensandbox.io"',
    'const BATCH_SANDBOX_RESOURCE = "batchsandboxes"',
  )
  check(replaced, "self-test could not mutate the inventory resource")
  mutations << ["unqualified inventory resource", widened_inventory_resource]

  missing_before_inventory = deep_copy(base)
  replaced = missing_before_inventory.dig(:sources, :smoke).sub!(
    'currentStage = "sandbox_inventory_before_launch"',
    'currentStage = "fixture_preflight"',
  )
  check(replaced, "self-test could not remove the before-Launch inventory stage")
  mutations << ["missing before-Launch inventory stage", missing_before_inventory]

  fabricated_sandbox_count = deep_copy(base)
  replaced = fabricated_sandbox_count.dig(:sources, :smoke).sub!(
    /document\.counts\.sandbox\s*=\s*\n?\s*document\.sandbox_inventory\.observations\.live_session\.entitled_objects\.length/,
    "document.counts.sandbox = 1",
  )
  check(replaced, "self-test could not fabricate the sandbox count")
  mutations << ["fabricated sandbox count", fabricated_sandbox_count]

  partial = deep_copy(unprovisioned)
  find_document(partial, :configmap, "ConfigMap", "cma-api", CONTROL_PLANE_NAMESPACE)
    .fetch("data")["CHRONO_TRIGGER_SANDBOX_BASE_URL"] = "https://trigger-provider.example"
  mutations << ["partial provider tuple", partial]

  enabled = deep_copy(unprovisioned)
  find_document(enabled, :configmap, "ConfigMap", "cma-api", CONTROL_PLANE_NAMESPACE)
    .fetch("data")["CHRONO_TRIGGER_ENABLED"] = "true"
  mutations << ["enabled empty provider tuple", enabled]

  same_origin = deep_copy(adoption)
  same_origin_config = find_document(
    same_origin,
    :configmap,
    "ConfigMap",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  ).fetch("data")
  same_origin_config["CHRONO_TRIGGER_SANDBOX_BASE_URL"] =
    same_origin_config.fetch("CHRONO_SANDBOX_BASE_URL")
  mutations << ["ordinary provider origin reuse", same_origin]

  same_namespace = deep_copy(adoption)
  find_document(
    same_namespace,
    :configmap,
    "ConfigMap",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  ).fetch("data")["CHRONO_TRIGGER_SANDBOX_NAMESPACE"] = CONTROL_PLANE_NAMESPACE
  mutations << ["ordinary provider namespace reuse", same_namespace]

  wrong_kernel = deep_copy(base)
  replaced = wrong_kernel.dig(:sources, :trigger).sub!(
    '"codex-trigger-native-v1"',
    '"wrong-trigger-kernel"',
  )
  check(replaced, "self-test could not mutate the source kernel")
  mutations << ["wrong source kernel literal", wrong_kernel]

  missing_assertion = deep_copy(base)
  replaced = missing_assertion.dig(:sources, :smoke).sub!(
    /assertEqual\(\n\s*launch\.runtime_admission\.kernel,\n\s*TRIGGER_KERNEL_ID,.*?\n\s*\);/m,
    "",
  )
  check(replaced, "self-test could not remove the smoke kernel assertion")
  mutations << ["missing smoke kernel assertion", missing_assertion]

  writable_mount = deep_copy(adoption)
  deployment = find_document(
    writable_mount,
    :deployment,
    "Deployment",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  )
  trigger_mount = deployment.dig("spec", "template", "spec", "containers")
    .find { |container| container["name"] == "api" }.fetch("volumeMounts")
    .find { |mount| mount["name"] == "trigger-provider-api-key" }
  trigger_mount["readOnly"] = false
  mutations << ["writable Trigger mount", writable_mount]

  mismatched_file = deep_copy(adoption)
  find_document(mismatched_file, :configmap, "ConfigMap", "cma-api", CONTROL_PLANE_NAMESPACE)
    .fetch("data")["CHRONO_TRIGGER_SANDBOX_API_KEY_FILE"] =
    "/var/run/secrets/other/provider-api-key"
  mutations << ["key file and mount mismatch", mismatched_file]

  ordinary_reuse = deep_copy(adoption)
  deployment = find_document(
    ordinary_reuse,
    :deployment,
    "Deployment",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  )
  trigger_volume = deployment.dig("spec", "template", "spec", "volumes")
    .find { |volume| volume["name"] == "trigger-provider-api-key" }
  trigger_volume.dig("csi", "volumeAttributes")["secretProviderClass"] =
    ORDINARY_SECRET_PROVIDER_CLASS
  mutations << ["ordinary SecretProviderClass reuse", ordinary_reuse]

  ordinary_resource_reuse = deep_copy(adoption)
  ordinary_class = find_document(
    ordinary_resource_reuse,
    :secretproviderclass,
    "SecretProviderClass",
    ORDINARY_SECRET_PROVIDER_CLASS,
    CONTROL_PLANE_NAMESPACE,
  )
  ordinary_resource = secret_resource_identity(
    secret_entries(ordinary_class, "ordinary OpenSandbox").first["resourceName"],
  )
  trigger_class = find_document(
    ordinary_resource_reuse,
    :secretproviderclass,
    "SecretProviderClass",
    "cma-trigger-provider-api-key",
    CONTROL_PLANE_NAMESPACE,
  )
  trigger_class.dig("spec", "parameters")["secrets"] = [
    {
      "resourceName" => "#{ordinary_resource}/versions/self-test",
      "path" => "provider-api-key",
    },
  ].to_yaml
  mutations << ["ordinary Secret Manager resource reuse", ordinary_resource_reuse]

  wrong_verbs = deep_copy(adoption)
  find_document(
    wrong_verbs,
    :serviceaccount,
    "Role",
    "cma-trigger-batchsandbox-reclaimer",
    "trigger-workloads",
  ).fetch("rules").first["verbs"] = ["get", "list"]
  mutations << ["missing Role delete verb", wrong_verbs]

  wrong_role_namespace = deep_copy(adoption)
  role = find_document(
    wrong_role_namespace,
    :serviceaccount,
    "Role",
    "cma-trigger-batchsandbox-reclaimer",
    "trigger-workloads",
  )
  role.fetch("metadata")["namespace"] = "other-workloads"
  mutations << ["Role in wrong namespace", wrong_role_namespace]

  wrong_subject_namespace = deep_copy(adoption)
  binding = find_document(
    wrong_subject_namespace,
    :serviceaccount,
    "RoleBinding",
    "cma-trigger-batchsandbox-reclaimer",
    "trigger-workloads",
  )
  binding.fetch("subjects").first["namespace"] = "other-control-plane"
  mutations << ["RoleBinding to wrong service-account namespace", wrong_subject_namespace]

  extra_trigger_role = deep_copy(adoption)
  extra_trigger_role.dig(:manifests, :serviceaccount) << {
    "apiVersion" => "rbac.authorization.k8s.io/v1",
    "kind" => "Role",
    "metadata" => {
      "name" => "cma-trigger-provider-admin",
      "namespace" => "trigger-workloads",
    },
    "rules" => [
      {
        "apiGroups" => ["sandbox.opensandbox.io"],
        "resources" => ["batchsandboxes"],
        "verbs" => ["*"],
      },
    ],
  }
  mutations << ["extra Trigger provider Role", extra_trigger_role]

  extra_trigger_binding = deep_copy(adoption)
  extra_trigger_binding.dig(:manifests, :serviceaccount) << {
    "apiVersion" => "rbac.authorization.k8s.io/v1",
    "kind" => "RoleBinding",
    "metadata" => {
      "name" => "cma-trigger-provider-duplicate",
      "namespace" => "trigger-workloads",
    },
    "subjects" => [
      {
        "kind" => "ServiceAccount",
        "name" => CONTROL_PLANE_SERVICE_ACCOUNT,
        "namespace" => CONTROL_PLANE_NAMESPACE,
      },
    ],
    "roleRef" => {
      "apiGroup" => "rbac.authorization.k8s.io",
      "kind" => "Role",
      "name" => "cma-trigger-batchsandbox-reclaimer",
    },
  }
  mutations << ["extra Trigger provider RoleBinding", extra_trigger_binding]

  trigger_group_binding = deep_copy(adoption)
  trigger_group_binding.dig(:manifests, :serviceaccount) << {
    "apiVersion" => "rbac.authorization.k8s.io/v1",
    "kind" => "RoleBinding",
    "metadata" => {
      "name" => "cma-trigger-provider-serviceaccount-group",
      "namespace" => "trigger-workloads",
    },
    "subjects" => [
      {
        "kind" => "Group",
        "name" => "system:serviceaccounts:#{CONTROL_PLANE_NAMESPACE}",
        "apiGroup" => "rbac.authorization.k8s.io",
      },
    ],
    "roleRef" => {
      "apiGroup" => "rbac.authorization.k8s.io",
      "kind" => "Role",
      "name" => "cma-trigger-batchsandbox-reclaimer",
    },
  }
  mutations << ["Trigger provider service-account Group binding", trigger_group_binding]

  trigger_user_binding = deep_copy(adoption)
  trigger_user_binding.dig(:manifests, :serviceaccount) << {
    "apiVersion" => "rbac.authorization.k8s.io/v1",
    "kind" => "RoleBinding",
    "metadata" => {
      "name" => "cma-trigger-provider-serviceaccount-user",
      "namespace" => "trigger-workloads",
    },
    "subjects" => [
      {
        "kind" => "User",
        "name" => "system:serviceaccount:#{CONTROL_PLANE_NAMESPACE}:#{CONTROL_PLANE_SERVICE_ACCOUNT}",
        "apiGroup" => "rbac.authorization.k8s.io",
      },
    ],
    "roleRef" => {
      "apiGroup" => "rbac.authorization.k8s.io",
      "kind" => "Role",
      "name" => "cma-trigger-batchsandbox-reclaimer",
    },
  }
  mutations << ["Trigger provider service-account User binding", trigger_user_binding]

  trigger_cluster_role = deep_copy(adoption)
  trigger_cluster_role.dig(:manifests, :serviceaccount) << {
    "apiVersion" => "rbac.authorization.k8s.io/v1",
    "kind" => "ClusterRole",
    "metadata" => {"name" => "cma-trigger-provider-cluster"},
    "rules" => [
      {
        "apiGroups" => ["sandbox.opensandbox.io"],
        "resources" => ["batchsandboxes"],
        "verbs" => ["get", "list", "delete"],
      },
    ],
  }
  mutations << ["Trigger provider ClusterRole", trigger_cluster_role]

  trigger_cluster_binding = deep_copy(adoption)
  trigger_cluster_binding.dig(:manifests, :serviceaccount) << {
    "apiVersion" => "rbac.authorization.k8s.io/v1",
    "kind" => "ClusterRoleBinding",
    "metadata" => {"name" => "cma-trigger-provider-cluster"},
    "subjects" => [
      {
        "kind" => "ServiceAccount",
        "name" => CONTROL_PLANE_SERVICE_ACCOUNT,
        "namespace" => CONTROL_PLANE_NAMESPACE,
      },
    ],
    "roleRef" => {
      "apiGroup" => "rbac.authorization.k8s.io",
      "kind" => "ClusterRole",
      "name" => "external-trigger-provider-role",
    },
  }
  mutations << ["Trigger provider ClusterRoleBinding", trigger_cluster_binding]

  token_disabled = deep_copy(adoption)
  deployment = find_document(
    token_disabled,
    :deployment,
    "Deployment",
    "cma-api",
    CONTROL_PLANE_NAMESPACE,
  )
  deployment.dig("spec", "template", "spec")["automountServiceAccountToken"] = false
  mutations << ["deployment token automount disabled", token_disabled]

  mutations.each { |name, inputs| expect_rejected(name, inputs) }
  puts "CMA Trigger provider contract self-test passed (#{mutations.length} rejected mutations)"
end

begin
  inputs = load_inputs
  case ARGV
  when []
    state = verify_contract(inputs)
    label = state == :unprovisioned ? "unprovisioned fail-closed" : "adoption-ready"
    puts "CMA Trigger provider smoke contract passed (provider lane: #{label})"
  when ["--self-test"]
    self_test(inputs)
  else
    warn "usage: #{File.basename($PROGRAM_NAME)} [--self-test]"
    exit 2
  end
rescue ContractError => error
  warn "CMA Trigger provider contract failed: #{error.message}"
  exit 1
end

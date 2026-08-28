#!/usr/bin/env ruby

require "digest"
require "json"
require "yaml"

ROOT = File.expand_path("../..", __dir__)
CI_WORKFLOW_PATH = File.join(ROOT, ".github/workflows/ci.yml")
WORKFLOW_PATH = File.join(ROOT, ".github/workflows/publish.yml")
RUNTIME_SMOKE_PATH = File.join(ROOT, ".github/scripts/cma-trigger-runtime-image-smoke.mjs")
RELEASE_DOC_PATH = File.join(ROOT, "docs/Release.md")
READINESS_DOC_PATH = File.join(ROOT, "docs/CMA_Trigger_Release_Readiness.md")

COMPONENTS = %w[api web runtime].freeze
METADATA_KEYS = %w[
  archive
  archiveReference
  archiveSha256
  component
  fullRevision
  platforms
  revision
  revisionReference
  schemaVersion
  sourceDigest
  sourceReference
  sourceRepository
  version
  versionReference
].freeze
ARTIFACT_KEYS = %w[
  acpHash
  acpVersion
  architecture
  bwrapHash
  bwrapPath
  bwrapVersion
  codexLauncherHash
  codexNativeHash
  codexVersion
  platform
  runtimeHash
  uid
].freeze
IMAGE_KEYS = %w[architecture id os reference repoDigests revision].freeze
PASS_EVIDENCE_KEYS = %w[
  artifacts
  buildCommit
  capability
  cleanup
  connect
  evidenceClass
  image
  isolationDenials
  listeners
  managedDenials
  outcomes
  permissionProfile
  providerFixture
  recordedAt
  redaction
  result
  schema
  startedAt
  startupRefusals
  traversal
  workspaceFence
].freeze
FALLBACK_EVIDENCE_KEYS = %w[
  artifacts
  buildCommit
  cleanup
  evidenceClass
  failure
  image
  recordedAt
  redaction
  result
  schema
  startedAt
].freeze
GATEWAY_PROBE_COMMAND = [
  "codex",
  "sandbox",
  "--permission-profile",
  "cma-trigger",
  "--",
  "/usr/local/bin/layer4-runtime",
  "runtime-gateway-probe-helper-v1",
].freeze
PERMISSION_PROFILE_SHA256 =
  "65ce3deca2d907481d999d1ec56a52ad3017644417e262a89cdf9ca1ad649be2"
PERMISSION_CONFIG = <<~'TOML'
  model = "gpt-5.6-sol"
  approval_policy = "never"
  default_permissions = "cma-trigger"

  [features]
  network_proxy = true

  [permissions.cma-trigger]
  extends = ":workspace"

  [permissions.cma-trigger.network]
  enabled = true
  enable_socks5 = false
  enable_socks5_udp = false
  allow_upstream_proxy = false
  allow_local_binding = false

  [permissions.cma-trigger.network.domains]
  "127.0.0.2" = "allow"
TOML

class ContractError < StandardError; end

def check(condition, message)
  raise ContractError, message unless condition
end

def required_hash(value, message)
  check(value.is_a?(Hash), message)
  value
end

def required_steps(job, job_name)
  steps = required_hash(job, "publish.yml must retain the #{job_name} job")["steps"]
  check(steps.is_a?(Array), "#{job_name} must define a step list")
  steps
end

def named_step(steps, name)
  steps.find { |step| step.is_a?(Hash) && step["name"] == name }
end

def required_step(steps, name)
  step = named_step(steps, name)
  check(step, "publish.yml must retain the #{name.inspect} step")
  step
end

def always_condition?(value)
  value.to_s.include?("always()")
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

def normalized_shell(source)
  source.to_s.gsub(/\\\s*\n/, " ").gsub(/\s+/, " ").strip
end

def jq_string_list_after(source, prefix)
  match = source.match(/#{Regexp.escape(prefix)}\s*\[(.*?)\]/m)
  match ? match[1].scan(/"([^"]+)"/).flatten : []
end

def require_jq_predicate(filter, predicate, message)
  check(
    normalized_shell(filter).include?(normalized_shell(predicate)),
    message,
  )
end

def extract_evidence_filter(close_run, scope)
  match = close_run.match(
    /'(?<filter>def sha256:.*?\n\s*end\))' \\\n\s*"\$D09_EVIDENCE_PATH"/m,
  )
  check(
    match,
    "#{scope} D-09 evidence closure must feed its evidence path through the jq validator",
  )
  match[:filter]
end

def verify_evidence_filter(close_run, scope)
  filter = extract_evidence_filter(close_run, scope)
  check(
    jq_string_list_after(filter, "then ((keys ==") == PASS_EVIDENCE_KEYS,
    "passing #{scope} D-09 evidence must enforce the exact top-level schema",
  )
  check(
    jq_string_list_after(filter, "else ((keys ==") == FALLBACK_EVIDENCE_KEYS,
    "closed #{scope} D-09 fallback evidence must enforce the exact top-level schema",
  )
  check(
    jq_string_list_after(filter, "(.image | keys) ==") == IMAGE_KEYS,
    "passing #{scope} D-09 evidence must enforce the exact image schema",
  )
  check(
    jq_string_list_after(filter, "(.artifacts | keys) ==") == ARTIFACT_KEYS,
    "passing #{scope} D-09 evidence must enforce the exact artifact schema",
  )

  contracts = [
    [
      <<~'JQ',
        def sha256:
          (type == "string") and test("^[0-9a-f]{64}$");
      JQ
      "#{scope} D-09 evidence must use the exact SHA-256 predicate",
    ],
    [
      <<~'JQ',
        def bounded_body:
          ((.bodyBytes | type) == "number")
          and (.bodyBytes >= 0 and .bodyBytes <= 65536)
          and ((.bodyBytes | floor) == .bodyBytes)
          and (.bodySha256 | sha256);
      JQ
      "#{scope} D-09 denial evidence must retain bounded hashed bodies",
    ],
    [
      <<~'JQ',
        type == "object"
          and (.schema == "cma-trigger-d09-v3")
          and (.evidenceClass == "D")
          and (.result == "pass" or .result == "fail" or .result == "inconclusive")
          and ((.startedAt | type) == "string" and (.startedAt | length) > 0)
          and ((.recordedAt | type) == "string" and (.recordedAt | length) > 0)
          and (.buildCommit == $build_commit)
          and (has("image"))
          and (has("artifacts"))
          and ((.cleanup | type) == "object")
          and ((.cleanup.status | type) == "string")
          and ((.cleanup.failureCodes | type) == "array")
          and (.redaction == "Only closed outcomes, status classes, artifact hashes, synthetic identities, and counts are retained; provider bodies and credential values are discarded.")
      JQ
      "#{scope} D-09 evidence must retain the exact common envelope",
    ],
    [
      <<~'JQ',
        and (.image.reference == $source_reference)
        and (.image.id | test("^sha256:[0-9a-f]{64}$"))
        and ((.image.repoDigests | type) == "array")
        and (all(.image.repoDigests[];
          (type == "string") and test("@sha256:[0-9a-f]{64}$")
        ))
        and (.image.os == "linux")
        and (.image.architecture == "amd64")
        and (.image.revision == $build_commit)
      JQ
      "passing #{scope} D-09 evidence must bind the exact image identity",
    ],
    [
      <<~'JQ',
        and (.artifacts.codexVersion == "codex-cli 0.147.0")
        and (.artifacts.acpVersion == "1.4.0")
        and (.artifacts.acpHash == "2c370f2337a5cc689d9a76b4bba1289d42afe6c17de824e2ca48004ba9670ab6")
        and (.artifacts.bwrapPath == "/usr/bin/bwrap")
        and (.artifacts.bwrapVersion | test("^bubblewrap [0-9]+\\.[0-9]+(\\.[0-9]+)?([-+~].*)?$"))
        and (.artifacts.bwrapHash | sha256)
        and (.artifacts.codexLauncherHash == "134063e133f0b4244fa3b251acf973d4fe4b4aeeacbdc135211bf480f59f1477")
        and (.artifacts.codexNativeHash == "cb0a15567e9a60a5820d54b0f6ae86d504dc3805c1eab21a47f70e3eb7b73a40")
        and (.artifacts.runtimeHash | sha256)
        and (.artifacts.uid == 10001)
        and (.artifacts.platform == "linux")
        and (.artifacts.architecture == "x64")
      JQ
      "passing #{scope} D-09 evidence must bind the exact runtime artifacts",
    ],
    [
      <<~JQ,
        and (.permissionProfile == {
          name: "cma-trigger",
          approvalPolicy: "never",
          sha256: "#{PERMISSION_PROFILE_SHA256}",
          allowedDomains: ["127.0.0.2"],
          socks5: false,
          upstreamProxy: false,
          localBinding: false
        })
      JQ
      "passing #{scope} D-09 evidence must bind the exact permission profile",
    ],
    [
      <<~'JQ',
        and ((.listeners | type) == "array")
        and ((.listeners | length) == 5)
        and (.listeners == (.listeners | sort))
        and ((.listeners | map(select(startswith("127.")))) == [
          "127.0.0.1:9091", "127.0.0.2:9090",
          "127.0.0.2:9091", "127.0.0.2:9092"
        ])
        and ((.listeners | map(select(startswith("127.") | not)) | length) == 1)
        and ((.listeners | map(select(startswith("127.") | not))[0]
          | test("^[0-9]{1,3}(\\.[0-9]{1,3}){3}:9090$")
            and (startswith("0.0.0.0:") | not)))
      JQ
      "passing #{scope} D-09 evidence must bind the exact listener inventory",
    ],
    [
      <<~'JQ',
        and (.outcomes == {
          missingPolicy: "policy_denied",
          wrongGeneration: "stale_generation",
          allowed: "allowed",
          providerDenied: "provider_denied",
          brokerTransient: "transient_broker_failure",
          wrongService: "policy_denied",
          wrongResource: "policy_denied"
        })
      JQ
      "passing #{scope} D-09 evidence must bind the exact provider outcomes",
    ],
    [
      <<~'JQ',
        and (.capability == {
          firstGeneration: 2,
          lastGeneration: 7,
          renewedBeforeEveryProviderProbe: true
        })
      JQ
      "passing #{scope} D-09 evidence must bind capability renewal and generations",
    ],
    [
      <<~'JQ',
        and ((.workspaceFence | keys) == [
          "adoptedBySupervisor", "boundary",
          "escapedOriginalSession", "fullyDaemonized",
          "quiescedWriters", "reapedAfterExit", "resumed",
          "stableWhileHeld"
        ])
        and (.workspaceFence.boundary == "linux_child_subreaper_v1")
        and (.workspaceFence.fullyDaemonized == true)
        and (.workspaceFence.adoptedBySupervisor == true)
        and (.workspaceFence.escapedOriginalSession == true)
        and ((.workspaceFence.quiescedWriters | type) == "number")
        and (.workspaceFence.quiescedWriters >= 2)
        and ((.workspaceFence.quiescedWriters | floor) == .workspaceFence.quiescedWriters)
        and (.workspaceFence.stableWhileHeld == true)
        and (.workspaceFence.resumed == true)
        and (.workspaceFence.reapedAfterExit == true)
      JQ
      "passing #{scope} D-09 evidence must prove coherent daemon-writer checkpointing",
    ],
    [
      <<~'JQ',
        and ((.managedDenials | map(.target)) == [
          "127.0.0.1", "localhost", "public-destination",
          "pod-address"
        ])
        and (all(.managedDenials[];
          (keys == ["bodyBytes", "bodySha256", "marker", "status", "target"])
          and (.status == 403)
          and (.marker == "blocked-by-allowlist")
          and bounded_body
        ))
      JQ
      "passing #{scope} D-09 evidence must bind the exact managed denials",
    ],
    [
      <<~'JQ',
        and ((.isolationDenials | map(.target)) == [
          "supervisor-front", "model-front"
        ])
        and (all(.isolationDenials[];
          (keys == ["bodyBytes", "bodySha256", "marker", "status", "target"])
          and (.status == 403)
          and (.marker == "denied")
          and (.bodyBytes == 0)
          and (.bodySha256 == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        ))
      JQ
      "passing #{scope} D-09 evidence must bind the exact isolation denials",
    ],
    [
      <<~'JQ',
        and ((.traversal | map(.form)) == [
          "literal-dot-dot", "encoded-dot-dot",
          "double-encoded-dot-dot", "mixed-dot-dot",
          "encoded-slash", "backslash"
        ])
        and (all(.traversal[];
          (keys == ["bodyBytes", "bodySha256", "form", "outcome", "status"])
          and (.status == 403)
          and (.outcome == "policy_denied")
          and bounded_body
        ))
      JQ
      "passing #{scope} D-09 evidence must bind the exact traversal denials",
    ],
    [
      <<~'JQ',
        and ((.connect | map([.path, .target])) == [
          ["direct-listener", "supervisor"],
          ["direct-listener", "model-proxy"],
          ["direct-listener", "platform-upstream"],
          ["direct-listener", "public-destination"],
          ["cma-trigger-sandbox", "supervisor"],
          ["cma-trigger-sandbox", "model-proxy"],
          ["cma-trigger-sandbox", "platform-upstream"],
          ["cma-trigger-sandbox", "public-via-gateway"],
          ["cma-trigger-sandbox", "public-direct"],
          ["cma-trigger-sandbox", "platform-direct"]
        ])
        and (all(.connect[];
          ((.status | type) == "number")
          and ((.status | floor) == .status)
          and (.status < 200 or .status >= 300)
          and bounded_body
          and (if .path == "direct-listener"
               then (keys == ["bodyBytes", "bodySha256", "path", "status", "target"])
               else (keys == ["bodyBytes", "bodySha256", "marker", "path", "status", "target"])
               end)
        ))
        and ([.connect[] | select(.path == "cma-trigger-sandbox") | [.target, .marker]] == [
          ["supervisor", "blocked-by-allowlist"],
          ["model-proxy", "blocked-by-allowlist"],
          ["platform-upstream", null],
          ["public-via-gateway", null],
          ["public-direct", "blocked-by-allowlist"],
          ["platform-direct", "blocked-by-allowlist"]
        ])
      JQ
      "passing #{scope} D-09 evidence must bind the exact CONNECT denials",
    ],
    [
      <<~'JQ',
        and ((.startupRefusals | map(.name)) == [
          "occupied-supervisor-ingress", "occupied-model-proxy",
          "occupied-supervisor-denial", "occupied-model-denial",
          "occupied-platform-gateway", "loopback-supervisor-ingress",
          "missing-model-upstream", "missing-platform-upstream",
          "missing-gateway-command", "missing-policy-keyring"
        ])
        and (all(.startupRefusals[];
          (keys == [
            "exitCode", "markerObserved", "name", "readyAnswered",
            "readyStatusCount", "signal"
          ])
          and (.exitCode == 2)
          and (.signal == null)
          and (.readyAnswered == false)
          and (.readyStatusCount == 0)
          and (.markerObserved == true)
        ))
      JQ
      "passing #{scope} D-09 evidence must bind the exact startup refusals",
    ],
    [
      <<~'JQ',
        and (.providerFixture == {
          githubCalls: 3,
          modelCalls: 0,
          connectCalls: 0,
          unexpectedCalls: 0,
          authorizationFailures: 0,
          probeHeaderLeaks: 0
        })
      JQ
      "passing #{scope} D-09 evidence must bind the exact provider fixture counts",
    ],
    [
      'and (.cleanup == {status: "verified", failureCodes: []})',
      "passing #{scope} D-09 evidence must prove exact cleanup",
    ],
    [
      <<~'JQ',
        and (.result == "fail" or .result == "inconclusive")
        and (.image == null or (.image | type) == "object")
        and (.artifacts == null or (.artifacts | type) == "object")
        and ((.failure | type) == "object")
        and ((.failure | keys) == ["code"])
        and ((.failure.code | type) == "string")
        and ((.failure.code | length) > 0)
        and ((.cleanup | keys) == ["failureCodes", "status"])
        and (.cleanup.status == "not_started"
             or .cleanup.status == "verified"
             or .cleanup.status == "failed")
        and (all(.cleanup.failureCodes[];
          (type == "string") and (length > 0)
        ))
      JQ
      "closed #{scope} D-09 fallback evidence must retain an honest failure and cleanup receipt",
    ],
  ]
  contracts.each do |predicate, message|
    require_jq_predicate(filter, predicate, message)
  end
  filter
end

def verify_smoke_sandbox_contract(smoke_source)
  gateway_match = smoke_source.match(
    /const GATEWAY_PROBE_COMMAND = JSON\.stringify\(\[(?<arguments>.*?)\]\);/m,
  )
  check(gateway_match, "D-09 smoke must define its gateway probe command as JSON")
  gateway_source = gateway_match[:arguments].sub(/,\s*\z/, "")
  gateway_command = begin
    JSON.parse("[#{gateway_source}]")
  rescue JSON::ParserError
    nil
  end
  check(
    gateway_command == GATEWAY_PROBE_COMMAND,
    "D-09 gateway probes must use the exact cma-trigger Codex sandbox command",
  )

  permission_match = smoke_source.match(/const PERMISSION_CONFIG = `(?<config>.*?)`;/m)
  check(permission_match, "D-09 smoke must define its Trigger permission profile")
  permission_config = permission_match[:config]
  check(
    permission_config == PERMISSION_CONFIG,
    "D-09 smoke must retain the exact cma-trigger permission profile",
  )
  check(
    Digest::SHA256.hexdigest(permission_config) == PERMISSION_PROFILE_SHA256,
    "D-09 smoke permission profile must match its retained SHA-256",
  )
  check(
    smoke_source.include?("CHRONO_RUNTIME_CONFIG: PERMISSION_CONFIG,") &&
      smoke_source.include?(
        "CHRONO_RUNTIME_GATEWAY_PROBE_COMMAND: GATEWAY_PROBE_COMMAND,",
      ),
    "D-09 runtime environment must install the exact profile and sandboxed gateway command",
  )
  check(
    smoke_source.include?("assert.equal(configResult.stdout, PERMISSION_CONFIG);"),
    "D-09 smoke must verify the installed permission profile byte for byte",
  )

  workspace_restore = "await restoreWorkspace(baseUrl, workspace);"
  sandbox_probe = "await sandboxObservation("
  gateway_probe = "await gatewayProbe("
  restore_index = smoke_source.index(workspace_restore)
  sandbox_index = smoke_source.index(sandbox_probe)
  gateway_index = smoke_source.index(gateway_probe)
  check(
    smoke_source.scan(workspace_restore).length == 1,
    "D-09 smoke must restore its repository exactly once",
  )
  check(
    restore_index && sandbox_index && gateway_index &&
      restore_index < sandbox_index && restore_index < gateway_index,
    "D-09 smoke must restore its repository before the first sandboxed command",
  )
  check(
    smoke_source.include?(
      "\"--workdir\",\n      WORKSPACE_ROOT,\n      container,",
    ),
    "D-09 raw probes must execute from the restored repository root",
  )

  sandbox_wrapper =
    '? ["codex", "sandbox", "--permission-profile", "cma-trigger", "--"]'
  check(
    smoke_source.scan(sandbox_wrapper).length == 1,
    "D-09 direct probes must use the exact cma-trigger Codex sandbox wrapper",
  )
  initialization_failure = <<~'JAVASCRIPT'
    if (receipt?.outcome === "command_sandbox_initialization_failed") {
      throw new CampaignFailure(
        "command_sandbox_initialization_failed",
        "inconclusive",
      );
    }
  JAVASCRIPT
  check(
    normalized_shell(smoke_source).include?(normalized_shell(initialization_failure)),
    "D-09 command-sandbox initialization failure must remain inconclusive",
  )

  prohibited_options = [
    [/['"]--privileged(?:=[^'"]*)?['"]/, "--privileged"],
    [/['"]--cap-add(?:=[^'"]*)?['"]/, "--cap-add"],
    [
      /['"]--network=host['"]|['"]--network['"]\s*,\s*['"]host['"]/m,
      "host networking",
    ],
    [/['"]--userns(?:=[^'"]*)?['"]/, "--userns"],
    [/['"]--pid(?:=[^'"]*)?['"]/, "--pid"],
  ]
  prohibited_options.each do |pattern, label|
    check(
      !smoke_source.match?(pattern),
      "D-09 smoke must not use #{label} on any container",
    )
  end
end

def workflow_run_source(workflow)
  jobs = workflow["jobs"]
  return "" unless jobs.is_a?(Hash)

  jobs.values.flat_map do |job|
    next [] unless job.is_a?(Hash) && job["steps"].is_a?(Array)

    job["steps"].map { |step| step.is_a?(Hash) ? step["run"].to_s : "" }
  end.join("\n")
end

def verify_bwrap_host_preparation(step, scope)
  run = normalized_shell(step["run"])
  expected = normalized_shell(<<~SHELL)
    set -euo pipefail
    sudo apt-get update
    sudo apt-get install -y --no-install-recommends \
      apparmor-profiles \
      apparmor-utils
    sudo install -m 0644 \
      /usr/share/apparmor/extra-profiles/bwrap-userns-restrict \
      /etc/apparmor.d/bwrap-userns-restrict
    sudo apparmor_parser -r /etc/apparmor.d/bwrap-userns-restrict
  SHELL
  check(
    run == expected,
    "#{scope} D-09 host preparation must exactly install and load the targeted bubblewrap AppArmor profile",
  )
  check(
    !step.key?("continue-on-error"),
    "#{scope} D-09 host preparation must fail closed",
  )
end

def verify_d09_host_contract(ci_workflow, publish_workflow, smoke_source)
  {
    "CI" => ci_workflow,
    "release" => publish_workflow,
  }.each do |scope, workflow|
    run_source = workflow_run_source(workflow)
    check(
      !run_source.match?(/\bsysctl\b/) &&
        !run_source.include?("apparmor_restrict_unprivileged_userns"),
      "#{scope} D-09 workflow must not disable the global AppArmor unprivileged user-namespace restriction",
    )
  end

  ci_jobs = required_hash(ci_workflow, "ci.yml must contain a workflow mapping")["jobs"]
  ci_jobs = required_hash(ci_jobs, "ci.yml must contain jobs")
  images = required_hash(ci_jobs["images"], "ci.yml must retain the images job")
  check(
    images["runs-on"] == "ubuntu-24.04",
    "CI D-09 runner must stay pinned to Ubuntu 24.04",
  )
  image_steps = required_steps(images, "CI images")
  ci_initialize = required_step(
    image_steps,
    "Initialize closed Trigger runtime evidence",
  )
  check(
    ci_initialize["if"] == "${{ always() && matrix.component == 'runtime' }}",
    "CI D-09 fallback initialization must always run for the runtime image",
  )
  initialize_run = ci_initialize["run"].to_s
  [
    'schema: "cma-trigger-d09-v3"',
    'result: "inconclusive"',
    'failure: {code: "ci_runtime_gate_incomplete"}',
    '"$D09_FALLBACK_PATH"',
    'cp "$D09_FALLBACK_PATH" "$D09_EVIDENCE_PATH"',
  ].each do |contract|
    check(
      initialize_run.include?(contract),
      "CI D-09 fallback initialization is missing #{contract}",
    )
  end
  ci_prepare = required_step(image_steps, "Prepare Codex bubblewrap AppArmor profile")
  check(
    ci_prepare["if"] == "matrix.component == 'runtime'",
    "CI D-09 host preparation must remain scoped to the runtime image",
  )
  verify_bwrap_host_preparation(ci_prepare, "CI")
  ci_campaign = required_step(image_steps, "Pinned Trigger runtime network boundary")
  ci_close = required_step(image_steps, "Close Trigger runtime evidence")
  check(
    ci_close["if"] == "${{ always() && matrix.component == 'runtime' }}",
    "CI D-09 evidence closure must always run for the runtime image",
  )
  ci_close_run = ci_close["run"].to_s
  ci_filter = verify_evidence_filter(ci_close_run, "CI")
  [
    "end)' \\",
    '.failure = {code: "evidence_record_invalid"}',
    '"$D09_FALLBACK_PATH" > "$D09_EVIDENCE_PATH.tmp"',
    'mv "$D09_EVIDENCE_PATH.tmp" "$D09_EVIDENCE_PATH"',
  ].each do |contract|
    check(ci_close_run.include?(contract), "CI D-09 evidence closure is missing #{contract}")
  end
  ci_upload = required_step(image_steps, "Retain Trigger runtime boundary evidence")
  check(
    always_condition?(ci_upload["if"]),
    "CI D-09 evidence upload must always run",
  )
  ci_require_pass = required_step(
    image_steps,
    "Require passing Trigger runtime evidence",
  )
  check(
    ci_require_pass["if"] == "${{ always() && matrix.component == 'runtime' }}",
    "CI D-09 pass assertion must always run for the runtime image",
  )
  check(
    ci_require_pass["run"].to_s.include?('.result == "pass"'),
    "CI image success must require passing D-09 evidence",
  )
  check(
    image_steps.index(ci_initialize) < image_steps.index(ci_prepare) &&
      image_steps.index(ci_prepare) < image_steps.index(ci_campaign) &&
      image_steps.index(ci_campaign) < image_steps.index(ci_close) &&
      image_steps.index(ci_close) < image_steps.index(ci_upload) &&
      image_steps.index(ci_upload) < image_steps.index(ci_require_pass),
    "CI D-09 must initialize evidence, prepare the host, run, close, upload, then require pass",
  )

  publish_jobs = required_hash(
    publish_workflow,
    "publish.yml must contain a workflow mapping",
  )["jobs"]
  publish_jobs = required_hash(publish_jobs, "publish.yml must contain jobs")
  gate = required_hash(
    publish_jobs["runtime-image-gate"],
    "publish.yml must retain the runtime-image-gate job",
  )
  check(
    gate["runs-on"] == "ubuntu-24.04",
    "release D-09 runner must stay pinned to Ubuntu 24.04",
  )
  gate_steps = required_steps(gate, "runtime-image-gate")
  release_initialize = required_step(
    gate_steps,
    "Initialize closed release-image evidence",
  )
  pull = required_step(gate_steps, "Pull the immutable runtime image")
  release_prepare = required_step(
    gate_steps,
    "Prepare Codex bubblewrap AppArmor profile",
  )
  check(
    !release_prepare.key?("if"),
    "release D-09 host preparation must not be optional",
  )
  verify_bwrap_host_preparation(release_prepare, "release")
  check(
    normalized_shell(ci_prepare["run"]) == normalized_shell(release_prepare["run"]),
    "CI and release D-09 host preparation must remain identical",
  )
  campaign = required_step(gate_steps, "Prove the release runtime network boundary")
  release_close = required_step(gate_steps, "Close release-image evidence")
  release_filter = verify_evidence_filter(release_close["run"].to_s, "release")
  check(
    release_filter == ci_filter,
    "CI and release D-09 evidence closures must use the identical jq validator",
  )
  check(
    gate_steps.index(release_initialize) < gate_steps.index(pull) &&
      gate_steps.index(pull) < gate_steps.index(release_prepare) &&
      gate_steps.index(release_prepare) < gate_steps.index(campaign),
    "release D-09 must initialize evidence, pull the image, prepare the host, then run the campaign",
  )

  launch_match = smoke_source.match(
    /campaignStage = "runtime-startup";(?<launch>.*?)"start D-09 runtime"/m,
  )
  check(launch_match, "D-09 smoke must retain its reviewed runtime launch block")
  launch = launch_match[:launch]
  security_options = launch.scan(/"--security-opt",\s*"([^"]+)"/m).flatten
  check(
    security_options == %w[
      seccomp=unconfined
      apparmor=unconfined
      systempaths=unconfined
    ],
    "D-09 runtime must retain exactly its three reviewed outer security options",
  )
  check(
    smoke_source.scan(/"--security-opt(?:=[^"]*)?"/).length == 3,
    "D-09 outer security options must remain scoped to its main runtime",
  )
  verify_smoke_sandbox_contract(smoke_source)
end

def verify_contract(workflow, release_doc, readiness_doc)
  jobs = required_hash(workflow, "publish.yml must contain a workflow mapping")["jobs"]
  jobs = required_hash(jobs, "publish.yml must contain jobs")

  images = required_hash(jobs["images"], "publish.yml must retain the images job")
  check(
    images.dig("strategy", "matrix", "component") == COMPONENTS,
    "images matrix must cover exactly api, web, runtime",
  )
  image_steps = required_steps(images, "images")
  publish = required_step(image_steps, "Publish")
  publish_with = required_hash(publish["with"], "Publish must retain its build-push inputs")
  check(
    publish_with["outputs"] == "type=registry,oci-mediatypes=true",
    "Publish must retain digest-preserving OCI registry output",
  )
  check(publish_with["provenance"] == false, "Publish must keep provenance disabled")

  exports = required_hash(
    jobs["image-exports"],
    "publish.yml must retain the image-exports job",
  )
  check(exports["name"] == "Image export", "image-exports must remain named Image export")
  check(
    Array(exports["needs"]) == %w[plan images],
    "image-exports must depend exactly on plan and images",
  )
  check(
    exports.dig("strategy", "matrix", "component") == COMPONENTS,
    "image-exports matrix must cover exactly api, web, runtime",
  )
  export_steps = required_steps(exports, "image-exports")
  export = required_step(export_steps, "Export the published image")
  export_run = export["run"].to_s
  check(
    contains_in_order?(
      export_run,
      [
        ".github/export-release-image.sh",
        '"${{ matrix.component }}"',
        '"${{ needs.plan.outputs.version }}"',
        '"$GITHUB_SHA"',
        '"${{ steps.image.outputs.repository }}"',
        '"${{ steps.image.outputs.output }}"',
      ],
    ),
    "image-exports must invoke the exporter with all five landed arguments",
  )

  transfer = required_step(export_steps, "Upload the OCI transfer artifact")
  transfer_with = required_hash(
    transfer["with"],
    "OCI transfer upload must retain artifact inputs",
  )
  expected_artifact = 'cma-${{ matrix.component }}-${{ needs.plan.outputs.version }}-${{ steps.image.outputs.revision }}-oci'
  check(
    transfer_with["name"] == expected_artifact,
    "OCI transfer artifact name must retain component, version, and revision",
  )
  expected_paths = [
    '${{ steps.image.outputs.output }}/cma-${{ matrix.component }}-${{ needs.plan.outputs.version }}-${{ steps.image.outputs.revision }}.oci.tar',
    '${{ steps.image.outputs.output }}/cma-${{ matrix.component }}-${{ needs.plan.outputs.version }}-${{ steps.image.outputs.revision }}.json',
  ]
  actual_paths = transfer_with["path"].to_s.lines.map(&:strip).reject(&:empty?)
  check(actual_paths == expected_paths, "OCI transfer must contain exactly its archive and metadata")
  check(transfer_with["retention-days"] == 1, "OCI transfer retention must remain one day")
  check(transfer_with["compression-level"] == 0, "OCI transfer upload must disable recompression")
  check(transfer_with["overwrite"] == true, "OCI transfer retry must replace its same-named artifact")
  check(
    transfer_with["if-no-files-found"] == "error",
    "OCI transfer must fail when either exported file is missing",
  )

  gate = required_hash(
    jobs["runtime-image-gate"],
    "publish.yml must retain the runtime-image-gate job",
  )
  check(
    Array(gate["needs"]) == %w[plan images image-exports],
    "runtime-image-gate must depend exactly on plan, images, and image-exports",
  )
  check(always_condition?(gate["if"]), "runtime-image-gate must run after dependency failure")
  gate_steps = required_steps(gate, "runtime-image-gate")

  initialize = gate_steps.first
  check(
    initialize.is_a?(Hash) && initialize["name"] == "Initialize closed release-image evidence",
    "runtime-image-gate must initialize closed fallback evidence in its first step",
  )
  check(always_condition?(initialize["if"]), "closed fallback initialization must always run")
  initialize_run = initialize["run"].to_s
  [
    'schema: "cma-trigger-d09-v3"',
    'result: "inconclusive"',
    'failure: {code: "release_image_gate_incomplete"}',
    '"$D09_FALLBACK_PATH"',
    'cp "$D09_FALLBACK_PATH" "$D09_EVIDENCE_PATH"',
  ].each do |contract|
    check(
      initialize_run.include?(contract),
      "closed fallback initialization is missing #{contract}",
    )
  end

  dependency = required_step(gate_steps, "Require successful image publication and export")
  dependency_env = required_hash(
    dependency["env"],
    "runtime dependency check must bind dependency results",
  )
  expected_dependency_env = {
    "PLAN_RESULT" => '${{ needs.plan.result }}',
    "IMAGES_RESULT" => '${{ needs.images.result }}',
    "EXPORTS_RESULT" => "${{ needs['image-exports'].result }}",
  }
  check(
    expected_dependency_env.all? { |name, value| dependency_env[name] == value },
    "runtime dependency check must consume plan, images, and image-exports results",
  )
  dependency_run = dependency["run"].to_s
  %w[plan:$PLAN_RESULT images:$IMAGES_RESULT image-exports:$EXPORTS_RESULT].each do |result|
    check(
      dependency_run.include?(result),
      "runtime dependency check must fail closed for #{result.split(':').first}",
    )
  end

  resolve = required_step(gate_steps, "Resolve the exact runtime OCI artifact")
  resolve_run = resolve["run"].to_s
  [
    'revision=${GITHUB_SHA:0:7}',
    'base="cma-runtime-${VERSION}-${revision}"',
    'artifact_name=${base}-oci',
    'metadata_path=$RUNNER_TEMP/runtime-export/${base}.json',
    'archive_path=$RUNNER_TEMP/runtime-export/${base}.oci.tar',
  ].each do |contract|
    check(resolve_run.include?(contract), "runtime artifact resolution is missing #{contract}")
  end

  download = required_step(gate_steps, "Download the exact runtime OCI artifact")
  download_with = required_hash(
    download["with"],
    "runtime artifact download must retain exact inputs",
  )
  check(
    download["uses"] == "actions/download-artifact@v4",
    "runtime artifact download must use actions/download-artifact@v4",
  )
  check(
    download_with["name"] == '${{ steps.runtime_artifact.outputs.artifact_name }}',
    "runtime artifact download must use the resolved exact artifact name",
  )
  check(
    download_with["path"] == '${{ runner.temp }}/runtime-export',
    "runtime artifact download must use its isolated export directory",
  )

  validation = required_step(gate_steps, "Validate the runtime OCI metadata and archive")
  validation_run = validation["run"].to_s
  schema_match = validation_run.match(/keys == \[(.*?)\]\)/m)
  schema_keys = schema_match ? schema_match[1].scan(/"([^"]+)"/).flatten : []
  check(schema_keys == METADATA_KEYS, "runtime metadata must enforce the exact 14-field schema")
  [
    ['.schemaVersion == 1', "runtime metadata must bind schema version 1"],
    ['.component == "runtime"', "runtime metadata must bind the runtime component"],
    ['.version == $version', "runtime metadata must bind the release version"],
    ['.revision == $revision', "runtime metadata must bind the short revision"],
    ['.fullRevision == $full_revision', "runtime metadata must bind the full revision"],
    ['.sourceRepository == $repository', "runtime metadata must bind the lowercase runtime repository"],
    ['.versionReference == ($repository + ":" + $version)', "runtime metadata must bind the version reference"],
    ['.revisionReference == ($repository + ":" + $revision)', "runtime metadata must bind the revision reference"],
    ['.sourceDigest | test("^sha256:[0-9a-f]{64}$")', "runtime metadata must validate the source digest"],
    ['.sourceReference == ($repository + "@" + .sourceDigest)', "runtime metadata must bind the immutable source reference"],
    ['.archive == $archive', "runtime metadata must bind the exact archive filename"],
    ['.archiveReference == $version', "runtime metadata must bind the archive version"],
    ['.archiveSha256 | test("^sha256:[0-9a-f]{64}$")', "runtime metadata must validate the archive digest"],
    ['.platforms == ["linux/amd64"]', "runtime metadata must permit only linux/amd64"],
  ].each do |contract, message|
    check(validation_run.include?(contract), message)
  end
  check(
    validation_run.include?("tr '[:upper:]' '[:lower:]'") &&
      validation_run.include?('expected_repository="ghcr.io/${owner}/cma-runtime"'),
    "runtime metadata must derive a lowercase GHCR runtime repository",
  )
  check(
    validation_run.include?('find "$export_dir" -mindepth 1 -maxdepth 1') &&
      validation_run.include?('[ "$entry_count" -ne 2 ]'),
    "runtime artifact must contain exactly two entries",
  )
  check(
    validation_run.include?('sha256sum "$ARCHIVE_PATH"') &&
      validation_run.include?('[ "$actual_archive_sha" != "$expected_archive_sha" ]'),
    "runtime archive checksum must be verified against metadata",
  )

  pull = required_step(gate_steps, "Pull the immutable runtime image")
  check(
    pull["run"].to_s.include?('docker pull "$SOURCE_REFERENCE"'),
    "D-09 must pull the validated immutable sourceReference",
  )

  campaign = required_step(gate_steps, "Prove the release runtime network boundary")
  campaign_run = campaign["run"].to_s
  [
    ['--image "$SOURCE_REFERENCE"', "release D-09 must use the immutable sourceReference"],
    ['--build-commit "$GITHUB_SHA"', "release D-09 must bind the full release commit"],
    ["--expected-architecture amd64", "release D-09 must require amd64"],
    ['--output "$D09_EVIDENCE_PATH"', "release D-09 must write the retained evidence path"],
  ].each do |argument, message|
    check(campaign_run.include?(argument), message)
  end

  close = required_step(gate_steps, "Close release-image evidence")
  check(always_condition?(close["if"]), "D-09 evidence closure must always run")
  close_run = close["run"].to_s
  verify_evidence_filter(close_run, "release")
  [
    ['.failure.code = "evidence_record_invalid"', "malformed D-09 evidence must close inconclusive"],
    ['"$D09_FALLBACK_PATH" > "$D09_EVIDENCE_PATH"', "malformed D-09 evidence must replace output with the closed fallback"],
  ].each do |contract, message|
    check(close_run.include?(contract), message)
  end

  upload = required_step(gate_steps, "Retain release runtime boundary evidence")
  check(always_condition?(upload["if"]), "D-09 evidence upload must always run")
  upload_with = required_hash(upload["with"], "D-09 evidence upload must retain artifact inputs")
  check(
    upload_with["name"] == 'cma-trigger-d09-release-${{ github.run_id }}-${{ github.run_attempt }}',
    "D-09 evidence artifact must bind the workflow run and attempt",
  )
  check(
    upload_with["path"] == '${{ runner.temp }}/cma-trigger-d09-release.json',
    "D-09 upload must retain the closed evidence path",
  )
  check(upload_with["retention-days"] == 90, "D-09 evidence must be retained for 90 days")
  check(
    upload_with["if-no-files-found"] == "error",
    "a missing D-09 evidence record must fail closed",
  )

  require_pass = required_step(gate_steps, "Require passing release runtime evidence")
  check(always_condition?(require_pass["if"]), "the D-09 pass assertion must always run")
  check(
    require_pass["run"].to_s.include?('.result == "pass"'),
    "GitHub Release creation must require a passing D-09 result",
  )
  close_index = gate_steps.index(close)
  upload_index = gate_steps.index(upload)
  pass_index = gate_steps.index(require_pass)
  check(
    close_index < upload_index && upload_index < pass_index,
    "D-09 steps must close evidence, upload it, then require pass",
  )

  release = required_hash(jobs["release"], "publish.yml must retain the release job")
  check(
    Array(release["needs"]) == %w[plan images image-exports runtime-image-gate],
    "release must depend directly on image-exports and runtime-image-gate",
  )
  release_steps = required_steps(release, "release")
  preflight = required_step(release_steps, "Verify the current run's image artifacts")
  preflight_run = preflight["run"].to_s
  [
    ["for component in api web runtime; do", "release artifact preflight must cover api, web, and runtime"],
    ['expected="cma-${component}-${VERSION}-${revision}-oci"', "release preflight must resolve exact OCI artifact names"],
    ['[ "$matches" -ne 1 ]', "release preflight must require exactly one artifact per component"],
    ['.expired == false', "release preflight must reject expired artifacts"],
    ['.size_in_bytes | type == "number" and . > 0', "release preflight must reject empty artifacts"],
  ].each do |contract, message|
    check(preflight_run.include?(contract), message)
  end

  release_prose = release_doc.gsub(/\s+/, " ")
  check(
    release_prose.include?("cannot prevent or retract those image tags"),
    "Release documentation must state that D-09 cannot prevent or retract GHCR image tags",
  )
  check(
    release_prose.include?("requires a pass before GitHub Release creation"),
    "Release documentation must state that D-09 gates GitHub Release creation",
  )

  readiness_prose = readiness_doc.gsub(/\s+/, " ")
  check(
    readiness_prose.include?("GitHub Release creation must depend on that run"),
    "release-readiness must name the exact GitHub Release gate",
  )
  check(
    readiness_prose.include?("D-09 does not gate that initial GHCR tag publication"),
    "release-readiness must not claim D-09 gates initial GHCR publication",
  )
end

def load_inputs
  workflow = YAML.safe_load(File.read(WORKFLOW_PATH), aliases: true)
  [workflow, File.read(RELEASE_DOC_PATH), File.read(READINESS_DOC_PATH)]
rescue Psych::Exception => error
  raise ContractError, "publish.yml must remain valid YAML: #{error.message}"
end

def load_d09_host_inputs
  ci_workflow = YAML.safe_load(File.read(CI_WORKFLOW_PATH), aliases: true)
  [ci_workflow, File.read(RUNTIME_SMOKE_PATH)]
rescue Psych::Exception => error
  raise ContractError, "ci.yml must remain valid YAML: #{error.message}"
end

def cloned(value)
  Marshal.load(Marshal.dump(value))
end

def mutation_step(workflow, job, name)
  named_step(workflow.fetch("jobs").fetch(job).fetch("steps"), name).tap do |step|
    raise "self-test fixture is missing #{job}/#{name}" unless step
  end
end

def replace_once!(source, before, after)
  changed = source.sub!(before, after)
  raise "self-test fixture is missing #{before.inspect}" unless changed
end

def ci_close_filter_mutation(before, after)
  lambda do |ci, _publish, _smoke|
    run = mutation_step(ci, "images", "Close Trigger runtime evidence").fetch("run")
    replace_once!(run, before, after)
  end
end

def release_close_filter_mutation(before, after)
  lambda do |_ci, publish, _smoke|
    run = mutation_step(
      publish,
      "runtime-image-gate",
      "Close release-image evidence",
    ).fetch("run")
    replace_once!(run, before, after)
  end
end

def smoke_source_mutation(before, after)
  lambda do |_ci, _publish, smoke|
    replace_once!(smoke, before, after)
  end
end

def run_self_tests(workflow, release_doc, readiness_doc)
  tests = [
    [
      "collapsed OCI export matrix",
      "image-exports matrix must cover exactly api, web, runtime",
      lambda do |candidate, _release, _readiness|
        candidate.fetch("jobs").fetch("image-exports").fetch("strategy").fetch("matrix")["component"] = ["runtime"]
      end,
    ],
    [
      "extended OCI transfer retention",
      "OCI transfer retention must remain one day",
      lambda do |candidate, _release, _readiness|
        mutation_step(candidate, "image-exports", "Upload the OCI transfer artifact").fetch("with")["retention-days"] = 90
      end,
    ],
    [
      "fallback moved away from the first step",
      "runtime-image-gate must initialize closed fallback evidence in its first step",
      lambda do |candidate, _release, _readiness|
        steps = candidate.fetch("jobs").fetch("runtime-image-gate").fetch("steps")
        steps.insert(1, steps.shift)
      end,
    ],
    [
      "full revision validation removed",
      "runtime metadata must bind the full revision",
      lambda do |candidate, _release, _readiness|
        run = mutation_step(candidate, "runtime-image-gate", "Validate the runtime OCI metadata and archive").fetch("run")
        replace_once!(run, ".fullRevision == $full_revision", ".fullRevision != null")
      end,
    ],
    [
      "D-09 changed to a floating image",
      "release D-09 must use the immutable sourceReference",
      lambda do |candidate, _release, _readiness|
        run = mutation_step(candidate, "runtime-image-gate", "Prove the release runtime network boundary").fetch("run")
        replace_once!(run, '--image "$SOURCE_REFERENCE"', '--image "ghcr.io/example/cma-runtime:latest"')
      end,
    ],
    [
      "runtime gate removed from release dependencies",
      "release must depend directly on image-exports and runtime-image-gate",
      lambda do |candidate, _release, _readiness|
        candidate.fetch("jobs").fetch("release")["needs"] = %w[plan images image-exports]
      end,
    ],
    [
      "pass assertion moved before evidence upload",
      "D-09 steps must close evidence, upload it, then require pass",
      lambda do |candidate, _release, _readiness|
        steps = candidate.fetch("jobs").fetch("runtime-image-gate").fetch("steps")
        pass_index = steps.index { |step| step["name"] == "Require passing release runtime evidence" }
        upload_index = steps.index { |step| step["name"] == "Retain release runtime boundary evidence" }
        pass_step = steps.delete_at(pass_index)
        steps.insert(upload_index, pass_step)
      end,
    ],
    [
      "system bubblewrap path evidence removed",
      "passing release D-09 evidence must bind the exact runtime artifacts",
      lambda do |candidate, _release, _readiness|
        run = mutation_step(candidate, "runtime-image-gate", "Close release-image evidence").fetch("run")
        replace_once!(
          run,
          '.artifacts.bwrapPath == "/usr/bin/bwrap"',
          '(.artifacts.bwrapPath | type) == "string"',
        )
      end,
    ],
    [
      "honest GHCR limitation removed from release documentation",
      "Release documentation must state that D-09 cannot prevent or retract GHCR image tags",
      lambda do |_candidate, release, _readiness|
        replace_once!(release, "cannot prevent or retract those image tags", "prevents those image tags")
      end,
    ],
  ]

  tests.each do |name, expected_error, mutation|
    candidate = cloned(workflow)
    candidate_release = release_doc.dup
    candidate_readiness = readiness_doc.dup
    mutation.call(candidate, candidate_release, candidate_readiness)
    begin
      verify_contract(candidate, candidate_release, candidate_readiness)
    rescue ContractError => error
      unless error.message.include?(expected_error)
        raise ContractError,
              "self-test #{name.inspect} failed for the wrong reason: #{error.message}"
      end
      next
    end
    raise ContractError, "self-test #{name.inspect} was not rejected"
  end

  puts "CMA Trigger publish contract self-tests passed (#{tests.length} mutations)"
end

def run_d09_host_self_tests(ci_workflow, publish_workflow, smoke_source)
  tests = [
    [
      "CI D-09 runner floated",
      "CI D-09 runner must stay pinned to Ubuntu 24.04",
      lambda do |ci, _publish, _smoke|
        ci.fetch("jobs").fetch("images")["runs-on"] = "ubuntu-latest"
      end,
    ],
    [
      "release D-09 runner floated",
      "release D-09 runner must stay pinned to Ubuntu 24.04",
      lambda do |_ci, publish, _smoke|
        publish.fetch("jobs").fetch("runtime-image-gate")["runs-on"] = "ubuntu-latest"
      end,
    ],
    [
      "CI bubblewrap profile load removed",
      "CI D-09 host preparation must exactly install and load",
      lambda do |ci, _publish, _smoke|
        run = mutation_step(ci, "images", "Prepare Codex bubblewrap AppArmor profile").fetch("run")
        replace_once!(
          run,
          "sudo apparmor_parser -r /etc/apparmor.d/bwrap-userns-restrict",
          "true # profile load removed",
        )
      end,
    ],
    [
      "release bubblewrap profile load removed",
      "release D-09 host preparation must exactly install and load",
      lambda do |_ci, publish, _smoke|
        run = mutation_step(
          publish,
          "runtime-image-gate",
          "Prepare Codex bubblewrap AppArmor profile",
        ).fetch("run")
        replace_once!(
          run,
          "sudo apparmor_parser -r /etc/apparmor.d/bwrap-userns-restrict",
          "true # profile load removed",
        )
      end,
    ],
    [
      "CI fallback moved after host preparation",
      "CI D-09 must initialize evidence, prepare the host, run, close, upload, then require pass",
      lambda do |ci, _publish, _smoke|
        steps = ci.fetch("jobs").fetch("images").fetch("steps")
        initialize_index = steps.index do |step|
          step["name"] == "Initialize closed Trigger runtime evidence"
        end
        initialize = steps.delete_at(initialize_index)
        prepare_index = steps.index do |step|
          step["name"] == "Prepare Codex bubblewrap AppArmor profile"
        end
        steps.insert(prepare_index + 1, initialize)
      end,
    ],
    [
      "CI fallback stopped running after failure",
      "CI D-09 fallback initialization must always run for the runtime image",
      lambda do |ci, _publish, _smoke|
        mutation_step(
          ci,
          "images",
          "Initialize closed Trigger runtime evidence",
        )["if"] = "matrix.component == 'runtime'"
      end,
    ],
    [
      "CI evidence closure moved before the campaign",
      "CI D-09 must initialize evidence, prepare the host, run, close, upload, then require pass",
      lambda do |ci, _publish, _smoke|
        steps = ci.fetch("jobs").fetch("images").fetch("steps")
        close_index = steps.index { |step| step["name"] == "Close Trigger runtime evidence" }
        campaign_index = steps.index do |step|
          step["name"] == "Pinned Trigger runtime network boundary"
        end
        close = steps.delete_at(close_index)
        campaign_index = steps.index do |step|
          step["name"] == "Pinned Trigger runtime network boundary"
        end
        steps.insert(campaign_index, close)
      end,
    ],
    [
      "CI malformed pass cleanup accepted",
      "passing CI D-09 evidence must prove exact cleanup",
      lambda do |ci, _publish, _smoke|
        run = mutation_step(ci, "images", "Close Trigger runtime evidence").fetch("run")
        replace_once!(
          run,
          'and (.cleanup == {status: "verified", failureCodes: []})',
          '(.cleanup.status | type) == "string"',
        )
      end,
    ],
    [
      "CI evidence input detached from jq validation",
      "CI D-09 evidence closure must feed its evidence path through the jq validator",
      lambda do |ci, _publish, _smoke|
        run = mutation_step(ci, "images", "Close Trigger runtime evidence").fetch("run")
        replace_once!(
          run,
          "end)' \\" + "\n",
          "end)'\n",
        )
      end,
    ],
    [
      "CI pass assertion weakened",
      "CI image success must require passing D-09 evidence",
      lambda do |ci, _publish, _smoke|
        run = mutation_step(
          ci,
          "images",
          "Require passing Trigger runtime evidence",
        ).fetch("run")
        replace_once!(run, '.result == "pass"', '.result != null')
      end,
    ],
    [
      "CI pass assertion moved before evidence upload",
      "CI D-09 must initialize evidence, prepare the host, run, close, upload, then require pass",
      lambda do |ci, _publish, _smoke|
        steps = ci.fetch("jobs").fetch("images").fetch("steps")
        pass_index = steps.index do |step|
          step["name"] == "Require passing Trigger runtime evidence"
        end
        pass = steps.delete_at(pass_index)
        upload_index = steps.index do |step|
          step["name"] == "Retain Trigger runtime boundary evidence"
        end
        steps.insert(upload_index, pass)
      end,
    ],
    [
      "CI host preparation allowed to fail",
      "CI D-09 host preparation must fail closed",
      lambda do |ci, _publish, _smoke|
        mutation_step(
          ci,
          "images",
          "Prepare Codex bubblewrap AppArmor profile",
        )["continue-on-error"] = true
      end,
    ],
    [
      "CI host preparation made permissive",
      "CI D-09 host preparation must remain scoped to the runtime image",
      lambda do |ci, _publish, _smoke|
        mutation_step(
          ci,
          "images",
          "Prepare Codex bubblewrap AppArmor profile",
        )["if"] = "always()"
      end,
    ],
    [
      "release host preparation made optional",
      "release D-09 host preparation must not be optional",
      lambda do |_ci, publish, _smoke|
        mutation_step(
          publish,
          "runtime-image-gate",
          "Prepare Codex bubblewrap AppArmor profile",
        )["if"] = "success()"
      end,
    ],
    [
      "global AppArmor restriction weakened",
      "CI D-09 workflow must not disable the global AppArmor unprivileged user-namespace restriction",
      lambda do |ci, _publish, _smoke|
        steps = ci.fetch("jobs").fetch("images").fetch("steps")
        prepare_index = steps.index do |step|
          step["name"] == "Prepare Codex bubblewrap AppArmor profile"
        end
        steps.insert(
          prepare_index,
          {
            "name" => "Disable the global AppArmor restriction",
            "run" => "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0\n",
          },
        )
      end,
    ],
    [
      "AppArmor outer option removed",
      "D-09 runtime must retain exactly its three reviewed outer security options",
      lambda do |_ci, _publish, smoke|
        replace_once!(smoke, "        \"apparmor=unconfined\",\n", "")
      end,
    ],
    [
      "capability added to D-09 runtime",
      "D-09 smoke must not use --cap-add on any container",
      lambda do |_ci, _publish, smoke|
        replace_once!(
          smoke,
          "        \"--publish\",\n",
          "        \"--cap-add=SYS_ADMIN\",\n        \"--publish\",\n",
        )
      end,
    ],
    [
      "host network added to D-09 runtime",
      "D-09 smoke must not use host networking on any container",
      lambda do |_ci, _publish, smoke|
        replace_once!(
          smoke,
          "        runtime,\n        \"--label\",\n        resourceLabel,\n        \"--network\",\n        network,\n",
          "        runtime,\n        \"--label\",\n        resourceLabel,\n        \"--network=host\",\n",
        )
      end,
    ],
    [
      "outer security option leaked to startup refusal",
      "D-09 outer security options must remain scoped to its main runtime",
      lambda do |_ci, _publish, smoke|
        marker = "      \"--entrypoint\",\n      \"node\",\n      ...environmentArguments(environment),"
        replacement = "      \"--security-opt\",\n      \"seccomp=unconfined\",\n      \"--entrypoint\",\n      \"node\",\n      ...environmentArguments(environment),"
        replace_once!(smoke, marker, replacement)
      end,
    ],
    [
      "system-path outer option replaced",
      "D-09 runtime must retain exactly its three reviewed outer security options",
      lambda do |_ci, _publish, smoke|
        replace_once!(
          smoke,
          '"systempaths=unconfined"',
          '"no-new-privileges=true"',
        )
      end,
    ],
    [
      "privileged D-09 runtime added",
      "D-09 smoke must not use --privileged on any container",
      lambda do |_ci, _publish, smoke|
        replace_once!(
          smoke,
          "        \"--publish\",\n",
          "        \"--privileged=true\",\n        \"--publish\",\n",
        )
      end,
    ],
  ]

  tests.concat(
    [
      [
        "CI pass top-level schema weakened",
        "passing CI D-09 evidence must enforce the exact top-level schema",
        ci_close_filter_mutation(
          '"artifacts", "buildCommit", "capability", "cleanup",',
          '"artifacts", "buildCommit", "cleanup",',
        ),
      ],
      [
        "CI permission-profile name changed",
        "passing CI D-09 evidence must bind the exact permission profile",
        ci_close_filter_mutation(
          'name: "cma-trigger",',
          'name: "workspace",',
        ),
      ],
      [
        "CI permission-profile domain broadened",
        "passing CI D-09 evidence must bind the exact permission profile",
        ci_close_filter_mutation(
          'allowedDomains: ["127.0.0.2"],',
          'allowedDomains: ["127.0.0.2", "localhost"],',
        ),
      ],
      [
        "CI permission-profile digest changed",
        "passing CI D-09 evidence must bind the exact permission profile",
        ci_close_filter_mutation(
          PERMISSION_PROFILE_SHA256,
          "05ce3deca2d907481d999d1ec56a52ad3017644417e262a89cdf9ca1ad649be2",
        ),
      ],
      [
        "CI listener count weakened",
        "passing CI D-09 evidence must bind the exact listener inventory",
        ci_close_filter_mutation(
          "((.listeners | length) == 5)",
          "((.listeners | length) >= 4)",
        ),
      ],
      [
        "CI known listener changed",
        "passing CI D-09 evidence must bind the exact listener inventory",
        ci_close_filter_mutation(
          '"127.0.0.1:9091", "127.0.0.2:9090",',
          '"127.0.0.1:9090", "127.0.0.2:9090",',
        ),
      ],
      [
        "CI provider outcome changed",
        "passing CI D-09 evidence must bind the exact provider outcomes",
        ci_close_filter_mutation(
          'wrongGeneration: "stale_generation",',
          'wrongGeneration: "policy_denied",',
        ),
      ],
      [
        "CI first capability generation weakened",
        "passing CI D-09 evidence must bind capability renewal and generations",
        ci_close_filter_mutation("firstGeneration: 2,", "firstGeneration: 1,"),
      ],
      [
        "CI capability renewal fact weakened",
        "passing CI D-09 evidence must bind capability renewal and generations",
        ci_close_filter_mutation(
          "renewedBeforeEveryProviderProbe: true",
          "renewedBeforeEveryProviderProbe: false",
        ),
      ],
      [
        "CI managed-denial targets weakened",
        "passing CI D-09 evidence must bind the exact managed denials",
        ci_close_filter_mutation(
          '"127.0.0.1", "localhost", "public-destination",',
          '"127.0.0.1", "public-destination",',
        ),
      ],
      [
        "CI managed-denial marker weakened",
        "passing CI D-09 evidence must bind the exact managed denials",
        ci_close_filter_mutation(
          '(.marker == "blocked-by-allowlist")',
          '((.marker | type) == "string")',
        ),
      ],
      [
        "CI denial body bound broadened",
        "CI D-09 denial evidence must retain bounded hashed bodies",
        ci_close_filter_mutation(
          ".bodyBytes <= 65536",
          ".bodyBytes <= 1048576",
        ),
      ],
      [
        "CI isolation-denial marker weakened",
        "passing CI D-09 evidence must bind the exact isolation denials",
        ci_close_filter_mutation(
          '(.marker == "denied")',
          '((.marker | type) == "string")',
        ),
      ],
      [
        "CI isolation-denial empty body weakened",
        "passing CI D-09 evidence must bind the exact isolation denials",
        ci_close_filter_mutation("(.bodyBytes == 0)", "(.bodyBytes >= 0)"),
      ],
      [
        "CI traversal form changed",
        "passing CI D-09 evidence must bind the exact traversal denials",
        ci_close_filter_mutation(
          '"literal-dot-dot", "encoded-dot-dot",',
          '"literal-dot-dot", "plain-path",',
        ),
      ],
      [
        "CI traversal outcome weakened",
        "passing CI D-09 evidence must bind the exact traversal denials",
        ci_close_filter_mutation(
          '(.outcome == "policy_denied")',
          '((.outcome | type) == "string")',
        ),
      ],
      [
        "CI CONNECT path changed",
        "passing CI D-09 evidence must bind the exact CONNECT denials",
        ci_close_filter_mutation(
          '["direct-listener", "supervisor"],',
          '["direct", "supervisor"],',
        ),
      ],
      [
        "CI CONNECT target changed",
        "passing CI D-09 evidence must bind the exact CONNECT denials",
        ci_close_filter_mutation(
          '["cma-trigger-sandbox", "public-direct"],',
          '["cma-trigger-sandbox", "public-destination"],',
        ),
      ],
      [
        "CI CONNECT status weakened",
        "passing CI D-09 evidence must bind the exact CONNECT denials",
        ci_close_filter_mutation(
          "(.status < 200 or .status >= 300)",
          "(.status != 200)",
        ),
      ],
      [
        "CI CONNECT marker weakened",
        "passing CI D-09 evidence must bind the exact CONNECT denials",
        ci_close_filter_mutation(
          '["supervisor", "blocked-by-allowlist"],',
          '["supervisor", null],',
        ),
      ],
      [
        "CI startup-refusal name changed",
        "passing CI D-09 evidence must bind the exact startup refusals",
        ci_close_filter_mutation(
          '"missing-gateway-command", "missing-policy-keyring"',
          '"missing-gateway-command", "missing-permission-profile"',
        ),
      ],
      [
        "CI startup-refusal exit code weakened",
        "passing CI D-09 evidence must bind the exact startup refusals",
        ci_close_filter_mutation(
          "(.exitCode == 2)",
          "((.exitCode | type) == \"number\")",
        ),
      ],
      [
        "CI startup-refusal readiness answer weakened",
        "passing CI D-09 evidence must bind the exact startup refusals",
        ci_close_filter_mutation(
          "(.readyAnswered == false)",
          "((.readyAnswered | type) == \"boolean\")",
        ),
      ],
      [
        "CI startup-refusal readiness count weakened",
        "passing CI D-09 evidence must bind the exact startup refusals",
        ci_close_filter_mutation(
          "(.readyStatusCount == 0)",
          "(.readyStatusCount >= 0)",
        ),
      ],
      [
        "CI startup-refusal marker weakened",
        "passing CI D-09 evidence must bind the exact startup refusals",
        ci_close_filter_mutation(
          "(.markerObserved == true)",
          "((.markerObserved | type) == \"boolean\")",
        ),
      ],
      [
        "CI provider fixture GitHub count changed",
        "passing CI D-09 evidence must bind the exact provider fixture counts",
        ci_close_filter_mutation("githubCalls: 3,", "githubCalls: 2,"),
      ],
      [
        "CI provider fixture zero-count fact changed",
        "passing CI D-09 evidence must bind the exact provider fixture counts",
        ci_close_filter_mutation("modelCalls: 0,", "modelCalls: 1,"),
      ],
      [
        "CI fallback top-level schema weakened",
        "closed CI D-09 fallback evidence must enforce the exact top-level schema",
        ci_close_filter_mutation(
          '"artifacts", "buildCommit", "cleanup", "evidenceClass",',
          '"artifacts", "buildCommit", "evidenceClass",',
        ),
      ],
      [
        "CI fallback result admits pass",
        "closed CI D-09 fallback evidence must retain an honest failure and cleanup receipt",
        ci_close_filter_mutation(
          'and (.result == "fail" or .result == "inconclusive")',
          'and (.result == "pass" or .result == "fail" or .result == "inconclusive")',
        ),
      ],
      [
        "CI fallback failure code weakened",
        "closed CI D-09 fallback evidence must retain an honest failure and cleanup receipt",
        ci_close_filter_mutation(
          '((.failure | keys) == ["code"])',
          '(.failure | has("code"))',
        ),
      ],
      [
        "release evidence validator drifted from CI",
        "CI and release D-09 evidence closures must use the identical jq validator",
        release_close_filter_mutation(
          "     (type == \"string\") and test(\"^[0-9a-f]{64}$\");",
          "      (type == \"string\") and test(\"^[0-9a-f]{64}$\");",
        ),
      ],
      [
        "gateway probe command made unsandboxed",
        "D-09 gateway probes must use the exact cma-trigger Codex sandbox command",
        smoke_source_mutation(
          "  \"codex\",\n  \"sandbox\",\n  \"--permission-profile\",\n  \"cma-trigger\",\n  \"--\",\n",
          "",
        ),
      ],
      [
        "gateway permission-profile name changed",
        "D-09 gateway probes must use the exact cma-trigger Codex sandbox command",
        smoke_source_mutation(
          "  \"cma-trigger\",\n  \"--\",\n",
          "  \"workspace\",\n  \"--\",\n",
        ),
      ],
      [
        "permission profile domain broadened",
        "D-09 smoke must retain the exact cma-trigger permission profile",
        smoke_source_mutation(
          "\"127.0.0.2\" = \"allow\"\n",
          "\"127.0.0.2\" = \"allow\"\n\"localhost\" = \"allow\"\n",
        ),
      ],
      [
        "installed permission-profile equality removed",
        "D-09 smoke must verify the installed permission profile byte for byte",
        smoke_source_mutation(
          "assert.equal(configResult.stdout, PERMISSION_CONFIG);",
          "assert.equal(typeof configResult.stdout, \"string\");",
        ),
      ],
      [
        "workspace restore moved after the first sandboxed command",
        "D-09 smoke must restore its repository before the first sandboxed command",
        lambda do |_ci, _publish, smoke|
          restore =
            "    campaignStage = \"workspace-restore\";\n" \
            "    const workspace = workspaceFixture();\n" \
            "    await restoreWorkspace(baseUrl, workspace);\n\n"
          replace_once!(smoke, restore, "")
          first_probe = "    requireManagedDenial(sandboxPreflight);\n"
          replace_once!(smoke, first_probe, "#{first_probe}\n#{restore}")
        end,
      ],
      [
        "sandbox initialization failure made a test failure",
        "D-09 command-sandbox initialization failure must remain inconclusive",
        smoke_source_mutation(
          "\"command_sandbox_initialization_failed\",\n      \"inconclusive\",",
          "\"command_sandbox_initialization_failed\",\n      \"fail\",",
        ),
      ],
      [
        "direct raw-probe sandbox wrapper changed",
        "D-09 direct probes must use the exact cma-trigger Codex sandbox wrapper",
        smoke_source_mutation(
          '? ["codex", "sandbox", "--permission-profile", "cma-trigger", "--"]',
          '? ["codex", "sandbox", "--permission-profile", "workspace", "--"]',
        ),
      ],
      [
        "provider fixture made privileged",
        "D-09 smoke must not use --privileged on any container",
        smoke_source_mutation(
          "        fixture,\n        \"--label\",\n",
          "        fixture,\n        \"--privileged\",\n        \"--label\",\n",
        ),
      ],
      [
        "startup-refusal container gained SYS_ADMIN",
        "D-09 smoke must not use --cap-add on any container",
        smoke_source_mutation(
          "      \"--rm\",\n      \"--network\",\n",
          "      \"--rm\",\n      \"--cap-add=SYS_ADMIN\",\n      \"--network\",\n",
        ),
      ],
      [
        "provider fixture gained host networking",
        "D-09 smoke must not use host networking on any container",
        smoke_source_mutation(
          "        fixture,\n        \"--label\",\n        resourceLabel,\n        \"--network\",\n        network,\n",
          "        fixture,\n        \"--label\",\n        resourceLabel,\n        \"--network=host\",\n",
        ),
      ],
      [
        "provider fixture shared the host user namespace",
        "D-09 smoke must not use --userns on any container",
        smoke_source_mutation(
          "        fixture,\n        \"--label\",\n",
          "        fixture,\n        \"--userns=host\",\n        \"--label\",\n",
        ),
      ],
      [
        "startup-refusal container shared the host PID namespace",
        "D-09 smoke must not use --pid on any container",
        smoke_source_mutation(
          "      \"--rm\",\n      \"--network\",\n",
          "      \"--rm\",\n      \"--pid=host\",\n      \"--network\",\n",
        ),
      ],
    ],
  )

  tests.each do |name, expected_error, mutation|
    candidate_ci = cloned(ci_workflow)
    candidate_publish = cloned(publish_workflow)
    candidate_smoke = smoke_source.dup
    mutation.call(candidate_ci, candidate_publish, candidate_smoke)
    begin
      verify_d09_host_contract(candidate_ci, candidate_publish, candidate_smoke)
    rescue ContractError => error
      unless error.message.include?(expected_error)
        raise ContractError,
              "self-test #{name.inspect} failed for the wrong reason: #{error.message}"
      end
      next
    end
    raise ContractError, "self-test #{name.inspect} was not rejected"
  end

  puts "CMA Trigger D-09 host contract self-tests passed (#{tests.length} mutations)"
end

unless ARGV.empty? || ARGV == ["--self-test"]
  warn "usage: #{File.basename($PROGRAM_NAME)} [--self-test]"
  exit 2
end

workflow, release_doc, readiness_doc = load_inputs
ci_workflow, smoke_source = load_d09_host_inputs
verify_contract(workflow, release_doc, readiness_doc)
verify_d09_host_contract(ci_workflow, workflow, smoke_source)
if ARGV == ["--self-test"]
  run_self_tests(workflow, release_doc, readiness_doc)
  run_d09_host_self_tests(ci_workflow, workflow, smoke_source)
else
  puts "CMA Trigger publish contract passed"
end

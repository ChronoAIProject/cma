#!/usr/bin/env ruby

require "digest"
require "json"
require "open3"
require "yaml"

ROOT = File.expand_path("../..", __dir__)
WORKFLOW_DIRECTORY = File.join(ROOT, ".github/workflows")
CI_WORKFLOW_PATH = File.join(ROOT, ".github/workflows/ci.yml")
WORKFLOW_PATH = File.join(ROOT, ".github/workflows/publish.yml")
RUNTIME_SMOKE_PATH = File.join(ROOT, ".github/scripts/cma-trigger-runtime-image-smoke.mjs")
GATEWAY_STATE_PATH = File.join(ROOT, ".github/scripts/cma-trigger-gateway-state.sh")
GATEWAY_ADOPTION_PATH = File.join(ROOT, ".github/scripts/cma-trigger-gateway-adoption.sh")
GATEWAY_SOURCE_CONTRACT_PATH = File.join(
  ROOT,
  ".github/scripts/cma-trigger-gateway-source-contract.rb",
)
GATEWAY_ADOPTION_TEST_PATH = File.join(
  ROOT,
  ".github/scripts/cma-trigger-gateway-adoption-tests.sh",
)
GATEWAY_RESTORE_TRAFFIC_PATH = File.join(
  ROOT,
  ".github/scripts/cma-trigger-gateway-restore-traffic.sh",
)
GATEWAY_RESTORE_TRAFFIC_TEST_PATH = File.join(
  ROOT,
  ".github/scripts/cma-trigger-gateway-restore-traffic-tests.sh",
)
RELEASE_DOC_PATH = File.join(ROOT, "docs/Release.md")
READINESS_DOC_PATH = File.join(ROOT, "docs/CMA_Trigger_Release_Readiness.md")

COMPONENTS = %w[api web runtime].freeze
PACKAGE_WRITER_ACTION_REFS = [
  "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09",
  "docker/login-action@c94ce9fb468520275223c153574b00df6fe4bcc9",
  "docker/setup-qemu-action@c7c53464625b32c7a7e944ae62b3e17d2b600130",
  "docker/setup-buildx-action@8d2750c68a42422c14e847fe6c8ac0403b4cbd6f",
  "docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8",
  "aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25",
  "docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8",
].freeze
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
  "runtime-gateway-probe-helper-v2",
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

def package_write_permission?(permissions)
  permissions == "write-all" ||
    (permissions.is_a?(Hash) && permissions["packages"] == "write")
end

def package_write_grants(publish_workflow)
  Dir.glob(File.join(WORKFLOW_DIRECTORY, "*.{yml,yaml}")).sort.each_with_object([]) do |path, grants|
    workflow = if path == WORKFLOW_PATH
                 publish_workflow
               else
                 YAML.safe_load(File.read(path), aliases: true)
               end
    next unless workflow.is_a?(Hash)

    basename = File.basename(path)
    grants << "#{basename}:workflow" if package_write_permission?(workflow["permissions"])
    workflow_jobs = workflow["jobs"]
    next unless workflow_jobs.is_a?(Hash)

    workflow_jobs.each do |name, job|
      next unless job.is_a?(Hash) && package_write_permission?(job["permissions"])

      grants << "#{basename}:#{name}"
    end
  end
rescue Psych::Exception => error
  raise ContractError, "workflow package-write audit requires valid YAML: #{error.message}"
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

def shell_readonly_words(source, variable, message)
  assignment = source.match(
    /^readonly #{Regexp.escape(variable)}="(?<words>[^"\n]*)"$/,
  )
  check(assignment, message)
  words = assignment[:words].split
  check(words.length == words.uniq.length, message)
  words
end

def normalized_shell(source)
  source.to_s.gsub(/\\\s*\n/, " ").gsub(/\s+/, " ").strip
end

def shell_function_body(source, function_name, message)
  match = source.match(
    /^#{Regexp.escape(function_name)}\(\) \{\n(?<body>.*?)^\}\n/m,
  )
  check(match, message)
  match[:body]
end

def bash_syntax_valid?(source)
  _stdout, _stderr, status = Open3.capture3(
    "/bin/bash",
    "--noprofile",
    "--norc",
    "-n",
    stdin_data: source,
  )
  status.success?
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
          wrongCatalogService: "policy_denied",
          wrongUserServiceSlug: "policy_denied",
          wrongResource: "policy_denied"
        })
      JQ
      "passing #{scope} D-09 evidence must bind the exact provider outcomes",
    ],
    [
      <<~'JQ',
        and (.capability == {
          firstGeneration: 2,
          lastGeneration: 9,
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
          "encoded-slash", "backslash",
          "canonical-github-slug-route", "suffixed-github-slug-route",
          "pat-lookalike-slug-route", "user-service-id-as-slug-route",
          "catalog-service-id-as-slug-route"
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

def verify_contract(workflow, release_doc, readiness_doc, restore_source = nil)
  restore_source ||= File.read(GATEWAY_RESTORE_TRAFFIC_PATH)
  jobs = required_hash(workflow, "publish.yml must contain a workflow mapping")["jobs"]
  jobs = required_hash(jobs, "publish.yml must contain jobs")

  main_checks_start = release_doc.index("### Required checks on `main`")
  main_checks_end = main_checks_start && release_doc.index("### ", main_checks_start + 4)
  check(
    main_checks_start && main_checks_end,
    "Release documentation must retain its bounded main-checks section",
  )
  main_checks = release_doc[main_checks_start...main_checks_end]
  ["Trigger release contract", "Trigger provider smoke contract"].each do |name|
    check(
      main_checks.include?(name),
      "main branch protection documentation must require #{name}",
    )
  end
  [
    "same non-null GitHub Actions App ID required on `develop`",
    "Legacy context-only entries accept a status from any app",
    "`required_status_checks.checks`",
  ].each do |contract|
    check(
      main_checks.include?(contract),
      "main branch protection documentation must retain exact app binding",
    )
  end

  images = required_hash(jobs["images"], "publish.yml must retain the images job")
  check(
    package_write_grants(workflow) == ["publish.yml:images"],
    "only the publish images job may receive package-write authority",
  )
  check(
    images["permissions"] == { "contents" => "read", "packages" => "write" },
    "images permissions must remain exactly contents-read and packages-write",
  )
  check(
    images.dig("strategy", "matrix", "component") == COMPONENTS,
    "images matrix must cover exactly api, web, runtime",
  )
  image_steps = required_steps(images, "images")
  package_writer_action_refs = image_steps.map do |step|
    step["uses"] if step.is_a?(Hash) && step["uses"]
  end.compact
  check(
    package_writer_action_refs == PACKAGE_WRITER_ACTION_REFS,
    "every action with package-write authority must retain its exact reviewed SHA",
  )
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
  ghcr_writer_contracts = [
    "one audited ordinary-operation package writer",
    "Repository permission inheritance is disabled on all three",
    "`repository_id=1330597394`) at `Write`",
    "zero direct package members or teams",
    "GitHub automatically grants package `Admin` to every organization owner",
    "audited break-glass exceptions",
    "all by `github-actions[bot]`",
    "every action executed in that job is pinned to a full commit SHA",
    "Literal zero-human package administration is not a capability GHCR offers",
  ]
  check(
    ghcr_writer_contracts.all? { |contract| release_prose.include?(contract) },
    "Release documentation must retain the audited GHCR writer and owner-exception contract",
  )
  check(
    release_prose.include?("cannot prevent or retract those image tags"),
    "Release documentation must state that D-09 cannot prevent or retract GHCR image tags",
  )
  check(
    release_prose.include?("requires a pass before GitHub Release creation"),
    "Release documentation must state that D-09 gates GitHub Release creation",
  )
  check(
    contains_in_order?(
      release_doc,
      [
        "### App-origin route and health-policy adoption",
        "cma_gateway_release_tag_record=\"$(git --no-replace-objects for-each-ref",
        'CMA_GATEWAY_CONFIRM="adopt-${cma_gateway_release_tag}-route"',
        ".github/scripts/cma-trigger-gateway-launch.sh adoption --adopt",
        "apply -f infra/api/configmap.yaml",
        "## 5. First production MongoDB cutover",
      ],
    ),
    "Release documentation must adopt the health policy and route before the first-production cutover",
  )
  check(
    contains_in_order?(
      restore_source,
      [
        "if ! cma_route_and_policies_ready; then",
        "cma_traffic_open=1",
        "https://bot.chrono-ai.fun/openapi.json",
        '(.paths["/api/v1/sessions"].get | type) == "object"',
      ],
    ),
    "Release documentation must prove the direct route and OpenAPI body around traffic restoration",
  )
  check(
    restore_source.scan("if ! cma_route_and_policies_ready; then").length == 2,
    "Release documentation must reassert the direct route before and after traffic restoration",
  )
  check(
    release_prose.include?(
      "Every advertised zonal NEG must be empty across two coherent sweeps under unchanged Service UID, resourceVersion, NEG identity, and EndpointSlice identities",
    ) && release_prose.include?(
      "zero-writer boundary is reached only after this external drain proof, the Deployment is at zero, every old API pod is confirmed deleted",
    ),
    "Release documentation must not treat EndpointSlice drain as the zero-writer boundary",
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
  readiness_routing_freeze_contracts = [
    "entire checked routing graph",
    "three owned `HTTPRoute`s",
    "`HealthCheckPolicy/cma-api`",
    "shared `gateway-infra/chronoai-gateway` Gateway",
    "`cma-api` and `cma-web` Services and GCPBackendPolicies",
    "generated Compute global address, forwarding rules, target proxies, URL maps, certificates/SSL policy, backend services, health checks, and NEGs",
    "normal GKE Gateway controller status and effective-resource reconciliation remains allowed",
    "Failure handling masks `HUP`, `INT`, `QUIT`, and `TERM` as its first state change",
    "successful adoption remains signal-masked while it disarms rollback",
    "`compute.networkEndpointGroups.listNetworkEndpoints`",
    "Zero EndpointSlice entries alone are insufficient",
    "two consecutive empty NEG sweeps",
    "complete EndpointSlice name/UID/resourceVersion records",
    "current checker is an open partial predicate",
    "shared Gateway's exact spec",
    "desired `cloud.google.com/neg` annotation",
  ]
  check(
    readiness_routing_freeze_contracts.all? do |contract|
      readiness_prose.include?(contract)
    end,
    "release-readiness must retain the routing-graph freeze and signal contract",
  )
  readiness_edge_contracts = [
    "both CMA hostnames proxied through Cloudflare",
    "Public DNS is not origin proof",
    "HTTP/HTTPS `curl --resolve` probes",
    "`%{remote_ip}` required to equal it",
    "release candidate selects and implements the conditional chain-aware",
    "approved GFE peer",
    "build-pinned complete official Cloudflare CIDR",
    "Startup accepts only an empty Cloudflare set or",
    "official-CIDR comparison in both directions",
    "hard-disabled by a reviewed source constant",
    "put `--disable` first",
    "set `--noproxy '*'`",
    "`SSLKEYLOGFILE`",
    "completes after its wall-clock deadline is a failure",
  ]
  check(
    readiness_edge_contracts.all? { |contract| readiness_prose.include?(contract) },
    "release-readiness must retain the Cloudflare, origin, and client-IP production gates",
  )
end

def load_inputs
  workflow = YAML.safe_load(File.read(WORKFLOW_PATH), aliases: true)
  [workflow, File.read(RELEASE_DOC_PATH), File.read(READINESS_DOC_PATH)]
rescue Psych::Exception => error
  raise ContractError, "publish.yml must remain valid YAML: #{error.message}"
end

def release_step10_invocation(release_doc)
  marker = "10. Restore ordinary traffic only after every final check has passed."
  marker_index = release_doc.index(marker)
  check(marker_index, "Release documentation must retain traffic-restoration step 10")
  block_open = release_doc.index("```bash", marker_index)
  block_close = block_open && release_doc.index("```", block_open + 7)
  check(
    block_open && block_close,
    "Release traffic-restoration step must retain an executable Bash invocation",
  )
  release_doc[(block_open + 7)...block_close]
end

def release_manual_restore_invocation(release_doc)
  marker = "Manual restore has its own explicit confirmation"
  marker_index = release_doc.index(marker)
  check(marker_index, "Release documentation must retain the manual restore procedure")
  block_open = release_doc.index("```sh", marker_index)
  block_close = block_open && release_doc.index("```", block_open + 5)
  check(
    block_open && block_close,
    "Release manual restore must retain an executable shell invocation",
  )
  release_doc[(block_open + 5)...block_close]
end

def release_step10_script(_release_doc = nil, restore_source = nil)
  restore_source || File.read(GATEWAY_RESTORE_TRAFFIC_PATH)
end

def verify_gateway_release_contract_legacy_unused(
  ci_workflow,
  release_doc,
  state_source,
  adoption_source,
  source_contract_source,
  restore_source = nil
)
  restore_source ||= File.read(GATEWAY_RESTORE_TRAFFIC_PATH)
  contract_job = required_hash(
    ci_workflow.fetch("jobs")["trigger-release-contract"],
    "ci.yml must retain the Trigger release contract job",
  )
  contract_step = required_step(
    required_steps(contract_job, "trigger-release-contract"),
    "Runtime provenance and release evidence stay mandatory",
  )
  contract_run = contract_step.fetch("run", "").to_s
  [
    "set -euo pipefail",
    "ruby -c .github/scripts/cma-trigger-gateway-source-contract.rb",
    "ruby .github/scripts/cma-trigger-gateway-source-contract.rb",
    "ruby .github/scripts/cma-trigger-gateway-source-contract.rb --self-test",
    "bash -n .github/scripts/cma-trigger-gateway-*.sh",
    ".github/scripts/cma-trigger-gateway-state.sh --self-test",
    ".github/scripts/cma-trigger-gateway-adoption.sh --self-test",
    ".github/scripts/cma-trigger-gateway-adoption-tests.sh",
    ".github/scripts/cma-trigger-gateway-restore-traffic-tests.sh",
    "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10 | bash -n",
    "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10 | shellcheck -s bash -",
    "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10-invocation | bash -n",
    "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10-invocation | shellcheck -s bash -",
  ].each do |command|
    check(
      contract_run.include?(command),
      "CI must syntax-check and self-test every Gateway release contract",
    )
  end
  full_shellcheck_command = "shellcheck -x .github/scripts/cma-trigger-gateway-*.sh"
  check(
    contract_run.include?(full_shellcheck_command),
    "CI must run full ShellCheck over every checked-in Gateway shell program",
  )

  check(File.executable?(GATEWAY_STATE_PATH), "Gateway state checker must be executable")
  check(File.executable?(GATEWAY_ADOPTION_PATH), "Gateway adoption program must be executable")
  check(
    File.executable?(GATEWAY_SOURCE_CONTRACT_PATH),
    "Gateway source contract must be executable",
  )
  check(
    File.executable?(GATEWAY_ADOPTION_TEST_PATH),
    "Gateway adoption behavioral tests must be executable",
  )
  check(
    File.executable?(GATEWAY_RESTORE_TRAFFIC_PATH),
    "Gateway traffic-restoration program must be executable",
  )
  check(
    File.executable?(GATEWAY_RESTORE_TRAFFIC_TEST_PATH) &&
      contract_run.include?("bash -n .github/scripts/cma-trigger-gateway-*.sh") &&
      contract_run.include?("shellcheck -x .github/scripts/cma-trigger-gateway-*.sh") &&
      contract_run.include?(".github/scripts/cma-trigger-gateway-restore-traffic-tests.sh"),
    "CI must syntax-check, shellcheck, and run the traffic-restoration behavioral tests",
  )
  source_contracts = [
    "routes == expected_routes",
    "health == [expected_health]",
    "httproute.yaml must contain exactly the three owned routes",
    "healthcheckpolicy.yaml must contain exactly the owned /ready policy",
    'ARGV == ["--self-test"]',
    'ARGV.length == 2 && %w[--emit --seal-bundle].include?(ARGV[0])',
    "emit directory must be private, owned, and not a symlink",
  ]
  check(
    source_contracts.all? { |contract| source_contract_source.include?(contract) },
    "Gateway source contract must validate and emit only the reviewed object bundle",
  )
  durable_bundle_contracts = [
    "def seal_bundle(directory)",
    "File.absolute_path(directory) == directory",
    "Dir.children(directory).sort == BUNDLE_FILENAMES",
    "(stat.mode & 0o777) == 0o600",
    "File.chmod(0o400, path)",
    "file.fsync",
    "fsync_directory(directory)",
    "fsync_directory(File.dirname(directory))",
  ]
  check(
    durable_bundle_contracts.all? { |contract| source_contract_source.include?(contract) },
    "Gateway source contract must durably seal exactly four custody files",
  )
  check(
    !state_source.match?(/\bkubectl\b[^\n]*\b(?:apply|patch|delete|replace|scale)\b/),
    "Gateway state checker must remain read-only",
  )
  state_contracts = [
    '((keys - ["apiVersion","kind","metadata","spec","status"]) | length) == 0',
    '(.metadata.finalizers // []) == []',
    '(.metadata.ownerReferences // []) == []',
    ".observedGeneration == $generation",
    "gcloud compute health-checks describe",
    ".httpHealthCheck.requestPath == $expected_path",
    "gcloud compute backend-services describe",
    ".healthChecks == [$health_link]",
    "([.backends[]?.group] | sort) == ($groups | sort)",
    "cma_gateway_assert_resource_unchanged",
  ]
  check(
    state_contracts.all? { |contract| state_source.include?(contract) } &&
      state_source.scan(
        "([.backends[]?.group] | sort) == ($groups | sort)",
      ).length == 2 &&
      state_source.scan(
        'cma_gateway_validate_api_route "${state_dir}/cma-api-route.json" || return 1',
      ).length == 2 &&
      contains_in_order?(
        state_source,
        [
          'cma_gateway_validate_api_route "${state_dir}/cma-api-route.json" || return 1',
          "cma_gateway_validate_web_route",
          "cma_gateway_validate_redirect_route",
          "cma_gateway_validate_health_policy",
          "cma_gateway_validate_backend_policy",
          "cma_gateway_validate_effective_state",
        ],
      ),
    "Gateway state checker must bind every route and effective GCP policy",
  )
  check(
    contains_in_order?(
      state_source,
      [
        "cma_gateway_kubectl() {",
        "timeout --signal=TERM --kill-after=5s 25s",
        "kubectl --request-timeout=15s",
      ],
    ),
    "Gateway state checker must bound every Kubernetes request",
  )
  check(
    contains_in_order?(
      state_source,
      [
        "cma_gateway_gcloud() {",
        "timeout --signal=TERM --kill-after=5s 25s",
        "env -i",
        'HOME="${HOME}"',
        'PATH="${PATH}"',
        "LC_ALL=C",
        "CLOUDSDK_CORE_HTTP_TIMEOUT=15",
        'gcloud "$@"',
      ],
    ) && state_source.include?(
      "self-test observed a poisoned gcloud child environment",
    ),
    "Gateway state checker must isolate every Compute truth read from ambient overrides",
  )
  check(
    state_source.include?(".timeoutSec == 660") &&
      state_source.scan("cma_gateway_validate_backend_policy").length >= 3,
    "Gateway state checker must prove both Kubernetes and effective backend timeouts",
  )
  [
    "self-test accepted a direct route with the wrong backend",
    "self-test accepted stale HTTPRoute conditions",
    "self-test accepted a stale effective health-check path",
    "self-test accepted a stale effective backend timeout",
    "self-test accepted a backend detached from the Service NEG",
    "self-test accepted unowned route metadata",
  ].each do |failure|
    check(
      state_source.include?(failure),
      "Gateway state checker must exercise its fail-closed semantic fixtures",
    )
  end
  check(
    contains_in_order?(
      state_source,
      [
        "cma_gateway_check_ready_transition() {",
        "cma_gateway_validate_snapshot",
        "cma_gateway_capture_live_state",
        'cma_gateway_validate_local_state "${state_dir}" "${snapshot_mode}" /ready',
        "for resource_name in cma-api-route cma-web-route cma-http-redirect-route; do",
        "cma_gateway_compare_snapshot_metadata",
        "cma_gateway_compare_snapshot_identity",
      ],
    ) && contains_in_order?(
      state_source,
      [
        "cma_gateway_validate_rollback_local_state() {",
        '"${policy_path}" == /livez || "${policy_path}" == /ready',
        '"${effective_path}" == /livez || "${effective_path}" == /ready',
        "cma_gateway_check_rollback_routes() {",
        "cma_gateway_validate_rollback_local_state",
        "cma_gateway_compare_snapshot_metadata",
        "cma_gateway_compare_snapshot_identity",
      ],
    ) && state_source.include?("--check-ready-transition") &&
      state_source.include?("--check-rollback-routes") &&
      state_source.include?("--check-adopted-from-snapshot"),
    "Gateway state checker must bind transition, rollback, and adopted state to the sealed snapshot",
  )

  adoption_preflight_contracts = [
    '[[ "${CMA_GATEWAY_RELEASE_REVISION:-}" =~ ^[0-9a-f]{40}$ ]]',
    '[[ "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:-}" == sole-spec-writer ]]',
    "git --no-replace-objects",
    "rev-parse --verify 'HEAD^{commit}'",
    "-c core.fsmonitor=false -c core.hooksPath=/dev/null",
    "status --porcelain=v1 --untracked-files=all",
    "cma_gateway_reviewed_source_digest",
    '[[ "${live_source_sha256}" == "${reviewed_source_sha256}" ]]',
    'CMA_GATEWAY_SOURCE_SHA256="${reviewed_source_sha256}"',
    "cma_gateway_source_contract --emit",
    "--seal-bundle",
    "cma_gateway_source_digest",
    "cma_gateway_named_files_digest",
    "cma_gateway_validate_snapshot_seal",
    "cma_gateway_capture_allowed_current_set",
    "--dry-run=server",
  ]
  check(
    adoption_preflight_contracts.all? { |contract| adoption_source.include?(contract) } &&
      contains_in_order?(
        adoption_source,
        [
          "cma_gateway_adopt() {",
          "cma_gateway_require_checkout",
          'trap \'cma_gateway_on_exit "$?"\' EXIT',
          "cma_gateway_prepare_desired",
          "cma_gateway_capture_snapshot",
          'cma_gateway_probe_old_images "${expected_status}"',
          "cma_gateway_capture_allowed_current_set",
          "cma_gateway_preflight_replacements",
          "CMA_GATEWAY_MUTATED=1",
          "cma_gateway_forward_one cma-api-health",
          'cma_gateway_wait_for_state ready-transition "${CMA_GATEWAY_ROLLBACK_DIR}"',
          'cma_gateway_probe_old_images "${expected_status}"',
          "for object_name in cma-api-route cma-web-route cma-http-redirect-route; do",
          'cma_gateway_forward_one "${object_name}"',
          'cma_gateway_wait_for_state adopted "${CMA_GATEWAY_ROLLBACK_DIR}"',
          "cma_gateway_probe_old_images 404",
          "--check-adopted-from-snapshot",
          "CMA_GATEWAY_MUTATED=0",
        ],
      ),
    "Gateway adoption must seal and preflight source state before ordered CAS writes",
  )
  check(
    adoption_source.scan("cma_gateway_validate_response").length >= 3 &&
      contains_in_order?(
        adoption_source,
        [
          "cma_gateway_forward_one() {",
          "cma_gateway_get_current_object",
          "cma_gateway_replace_cas",
        ],
      ),
    "Gateway adoption must validate write responses and re-read before every forward CAS",
  )
  check(
    adoption_source.scan(
      'cma_gateway_deadline_reached "${deadline}"',
    ).length == 7 &&
      contains_in_order?(
        adoption_source,
        [
          "cma_gateway_wait_for_state() {",
          "while ! cma_gateway_deadline_reached",
          "--check-adopted-from-snapshot",
          'cma_gateway_deadline_reached "${deadline}" && return 1',
          "--check-snapshot",
          'cma_gateway_deadline_reached "${deadline}" && return 1',
          "--check-ready-transition",
          'cma_gateway_deadline_reached "${deadline}" && return 1',
          "--check-rollback-routes",
          'cma_gateway_deadline_reached "${deadline}" && return 1',
          "sleep 5 || return 1",
          'cma_gateway_deadline_reached "${deadline}" && return 1',
        ],
      ),
    "Gateway adoption convergence must reject checker results that complete after the wall-clock deadline",
  )
  check(
    adoption_source.scan("cma_gateway_preflight_replacements").length >= 4 &&
      contains_in_order?(
        adoption_source,
        [
          "cma_gateway_adopt() {",
          "cma_gateway_preflight_replacements",
          '"${CMA_GATEWAY_DESIRED_DIR}"',
          "cma_gateway_preflight_replacements",
          '"${CMA_GATEWAY_ROLLBACK_DIR}"',
          "CMA_GATEWAY_MUTATED=1",
        ],
      ) && contains_in_order?(
        adoption_source,
        [
          "cma_gateway_restore() {",
          'if [[ "${require_preflight}" == 1 ]]',
          "cma_gateway_preflight_replacements",
          '"${restore_dir}" "${snapshot_dir}" "${snapshot_dir}"',
          "for object_name in cma-api-route cma-web-route cma-http-redirect-route; do",
          "cma_gateway_manual_restore() {",
          'cma_gateway_restore "${snapshot_dir}" 1',
        ],
      ),
    "Gateway adoption and manual restore must preflight every forward and rollback replacement",
  )
  rollback_failure_guards = [
    'CMA_GATEWAY_PROBE_DIR="$(mktemp -d)" || return 1',
    '[[ -n "${CMA_GATEWAY_PROBE_DIR}" ]] || return 1',
    'restore_dir="$(mktemp -d)" || return 1',
    '[[ -n "${restore_dir}" ]] || return 1',
    'chmod 700 "${restore_dir}" || {',
    '> "${output_file}" || return 1',
    '> "${response_file}" || \\',
    'cp "${current_file}" "${response_file}" || return 1',
    'chmod 600 "${response_file}" || return 1',
  ]
  check(
    rollback_failure_guards.all? { |contract| adoption_source.include?(contract) },
    "Gateway rollback must preserve helper failures while running under set +e",
  )
  rollback_custody_contracts = [
    '[[ "${CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM:-}" == durable-off-host ]]',
    'custody_root="${CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT:-}"',
    '"${custody_root}" == /*',
    '"${custody_root}" != /',
    '-d "${custody_root}"',
    '! -L "${custody_root}"',
    '-O "${custody_root}"',
    'custody_mode="$(cma_gateway_file_mode "${custody_root}")"',
    '[[ "${custody_mode}" == 700 ]]',
    'mktemp -d "${custody_root%/}/cma-gateway-route.XXXXXXXXXX"',
    '--seal-bundle "${CMA_GATEWAY_ROLLBACK_DIR}"',
  ]
  check(
    rollback_custody_contracts.all? { |contract| adoption_source.include?(contract) },
    "Gateway adoption must capture rollback material in attested durable custody",
  )
  check(
    adoption_source.include?("cma_gateway_kubectl replace -f") &&
      adoption_source.include?('.metadata.resourceVersion | type) == "string"') &&
      contains_in_order?(
        adoption_source,
        [
          "cma_gateway_restore() {",
          "cma_gateway_validate_snapshot_seal",
          "cma_gateway_check --validate-snapshot",
          "cma_gateway_capture_allowed_current_set",
          "for object_name in cma-api-route cma-web-route cma-http-redirect-route; do",
          "cma_gateway_restore_one",
          "cma_gateway_wait_for_state rollback-routes",
          'cma_gateway_probe_old_images "${expected_status}"',
          "health_current=",
          "cma_gateway_object_matches_target",
          "cma_gateway_restore_one cma-api-health",
          "cma_gateway_wait_for_state snapshot",
          "cma_gateway_probe_old_images",
          "cma_gateway_check --check-snapshot",
        ],
      ) && contains_in_order?(
        adoption_source,
        [
          "cma_gateway_on_exit() {",
          "trap '' HUP INT QUIT TERM",
          'local original_status="$1"',
          "local rollback_status=0",
          "trap - EXIT",
          "set +e",
          'cma_gateway_restore "${CMA_GATEWAY_ROLLBACK_DIR}"',
          "trap - HUP INT QUIT TERM",
          'exit "${original_status}"',
        ],
      ),
    "Gateway rollback must mask signals and restore every sealed object through CAS",
  )
  check(
    contains_in_order?(
      adoption_source,
      [
        "cma_gateway_adopt() {",
          '--check-adopted-from-snapshot "${CMA_GATEWAY_ROLLBACK_DIR}"',
          "trap '' HUP INT QUIT TERM",
          '"CMA Gateway route and /ready health check are adopted and effective;',
          "CMA_GATEWAY_MUTATED=0",
          "trap - EXIT",
          'rm -rf "${CMA_GATEWAY_WORK_DIR}" "${CMA_GATEWAY_DESIRED_DIR}"',
          'CMA_GATEWAY_WORK_DIR=""',
          'CMA_GATEWAY_DESIRED_DIR=""',
          "trap - HUP INT QUIT TERM",
      ],
    ),
    "Gateway adoption success must remain signal-masked through disarm, cleanup, and output",
  )
  check(
    contains_in_order?(
      adoption_source,
      [
        "cma_gateway_restore() {",
        "for object_name in cma-api-route cma-web-route cma-http-redirect-route; do",
        "cma_gateway_wait_for_state rollback-routes",
        'cma_gateway_probe_old_images "${expected_status}"',
        "health_current=",
        "cma_gateway_object_matches_target",
        "else",
        "cma_gateway_restore_one cma-api-health",
      ],
    ) && adoption_source.include?(
      '"route rollback is unproved; refusing to change the API health policy"',
    ),
    "Gateway rollback must prove routes and source probes before changing health",
  )
  adoption_release_tag_contracts = [
    "cma_gateway_resolve_release_tag() {",
    "git --no-replace-objects for-each-ref",
    '--points-at "${release_revision}"',
    "--format='%(refname:strip=2)|%(objecttype)|%(*objecttype)|%(*objectname)'",
    "'refs/tags/v*'",
    '"${tag_record}" != *$\'\\n\'*',
    '"${tag_name}" =~ ^v(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$',
    '"${tag_object_type}" == tag',
    '"${peeled_object_type}" == commit',
    '"${peeled_revision}" == "${release_revision}"',
    'local expected_confirmation="${action}-${CMA_GATEWAY_RELEASE_TAG}-${scope}"',
    '"${CMA_GATEWAY_CONFIRM:-}" == "${expected_confirmation}"',
    "cma_gateway_require_confirmation adopt route",
    "cma_gateway_require_confirmation restore route",
  ]
  check(
    adoption_release_tag_contracts.all? { |contract| adoption_source.include?(contract) } &&
      adoption_source.include?(
        '[[ "${CMA_GATEWAY_ROLLBACK_SHA256:-}" =~ ^[0-9a-f]{64}$ ]]',
      ),
    "Gateway adoption and sealed manual restore must require separate confirmations",
  )
  check(
    adoption_source.include?("http://bot.chrono-ai.fun/") &&
      adoption_source.include?("http://bot-api.chrono-ai.fun/") &&
      adoption_source.include?("/.well-known/agent-card.json") &&
      adoption_source.include?("https://bot-api.chrono-ai.fun/ready") &&
      adoption_source.include?('. == {"status":"ready"}'),
    "Gateway adoption must probe redirect and both discovery ownership paths",
  )

  release_gateway_prose = release_doc.gsub(/\s+/, " ")
  check(
    contains_in_order?(
      adoption_source,
      [
        'adopted_projection_sha256="$(awk',
        '"${adopted_projection_dir}/projection.sha256")"',
        '"${adopted_projection_sha256}" ==',
        '"$(cma_gateway_sha256_file "${adopted_projection_dir}/projection.json")"',
        '"CMA Gateway route and /ready health check are adopted and effective;',
        'adopted projection ${adopted_projection_dir}; SHA-256 ${adopted_projection_sha256};',
        'predecessor SHA-256 ${CMA_GATEWAY_PROJECTION_SHA256}"',
      ],
    ) && release_gateway_prose.include?(
      "Carry the adopted digest as `CMA_GATEWAY_SOURCE_PROJECTION_SHA256` and the predecessor digest as `CMA_GATEWAY_SOURCE_PROJECTION_PREDECESSOR_SHA256`",
    ),
    "Gateway adoption must externally acknowledge adopted projection custody",
  )
  release_gateway_contracts = [
    "compute.globalAddresses.get",
    "compute.globalForwardingRules.get",
    "compute.targetHttpProxies.get",
    "compute.targetHttpsProxies.get",
    "compute.urlMaps.get",
    "compute.sslCertificates.get",
    "compute.healthChecks.get",
    "compute.backendServices.get",
    "compute.networkEndpointGroups.get",
    "compute.networkEndpointGroups.listNetworkEndpoints",
    "compute.sslPolicies.get",
    "CMA_GATEWAY_RELEASE_REVISION='<reviewed-full-SHA>'",
    "single annotated SemVer tag",
    'CMA_GATEWAY_CONFIRM="adopt-${cma_gateway_release_tag}-route"',
    "CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer",
    "CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT='<durable-absolute-directory>'",
    "CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host",
    'CMA_GATEWAY_CONFIRM="restore-${cma_gateway_release_tag}-route"',
    "CMA_GATEWAY_ROLLBACK_SHA256='<externally-recorded-rollback-SHA-256>'",
    ".github/scripts/cma-trigger-gateway-launch.sh adoption --adopt",
    "--restore '<retained-sealed-rollback-directory>'",
    "server-side dry-runs all four desired replacements plus all four sealed rollback replacements",
    "Forward writes use `kubectl replace`, not apply",
    "freshly read UID and `resourceVersion`",
    "masks ordinary termination signals",
    "digest alone cannot reconstruct a rollback",
    "mode `0700`",
    "file-fsyncs each object",
    "effective GCP health check has changed to `/ready`",
    "closed legacy transition set `{/livez,/ready}`",
    "derives its source seal from the reviewed commit blobs",
    "operator attestation, not an admission lock",
    "server-side dry-runs all four sealed snapshot replacements",
    "entire checked routing graph",
    "every `HTTPRoute` in any namespace allowed to attach to either the HTTP or HTTPS listener",
    "shared `gateway-infra/chronoai-gateway` Gateway",
    "`Service/cma-api`, `Service/cma-web`, `GCPBackendPolicy/cma-api`, `GCPBackendPolicy/cma-web`",
    "generated Compute global address, forwarding rules, target HTTP/HTTPS proxies, URL maps, certificates, SSL policy, backend services, health checks, and network endpoint groups",
    "Normal GKE Gateway controller status and effective-resource reconciliation remains allowed",
    "masks `HUP`, `INT`, `QUIT`, and `TERM` as its first state-changing action",
    "marks the mutation closed or clears the `EXIT` rollback trap",
    "default dispositions are restored only after that output completes",
  ]
  check(
    release_gateway_contracts.all? { |contract| release_gateway_prose.include?(contract) },
    "Release documentation must state the sealed CAS adoption and recovery contract",
  )
  effective_projection_gate_contracts = [
    "current Gateway checker has an open partial effective-Compute predicate",
    "two live backend-service JSON documents",
    "complete effective routing trace",
    "exact Gateway UID, `resourceVersion`, annotations, and status IP",
    "port 80 and 443 forwarding rules",
    "re-read every Compute fingerprint plus the Gateway identity",
    "Backend projections must include CDN",
    "every per-NEG balancing/capacity/rate field",
    "every API and web health-check link",
    "Health projections must include interval, timeout, healthy/unhealthy thresholds",
    "URL-map/proxy projection must close redirects",
    "current-generation `Programmed` status and listener conditions",
    "desired `cloud.google.com/neg` annotation",
    "complete accepted `cloud.google.com/neg-status` shape",
    "Fail-closed evidence-collection and checker tooling may merge",
    "production adoption must remain disabled",
    "`--adopt` must not be run",
    "Public DNS is not proof of the origin attachment graph",
    "Ordinary public-chain probes",
    "origin-pinned HTTP and HTTPS probes",
    "`curl --resolve`",
    "`%{remote_ip}` to equal that captured address",
    "Cloudflare DNS, origin/TLS, redirect, cache, transform, header, and routing setting",
    "Production is also blocked on live validation of the selected conditional",
    "GFE-observed upstream token",
    "build-pinned complete official Cloudflare CIDR",
    "official Cloudflare IPv4 and IPv6 sets",
    "production adoption, `--adopt`, and step 10 traffic restoration remain disabled",
  ]
  check(
    effective_projection_gate_contracts.all? do |contract|
      release_gateway_prose.include?(contract)
    end,
    "Release documentation must keep effective routing evidence as a production pre-mutation gate",
  )
  check(
    adoption_source.include?("readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0") &&
      adoption_source.include?("cma_gateway_require_production_mutation_enabled") &&
      contains_in_order?(
        adoption_source,
        [
          "cma_gateway_adopt() {",
          "cma_gateway_require_production_mutation_enabled",
          "cma_gateway_require_checkout",
        ],
      ) &&
      contains_in_order?(
        adoption_source,
        [
          "cma_gateway_manual_restore() {",
          "cma_gateway_require_production_mutation_enabled",
          "cma_gateway_require_checkout",
        ],
      ) &&
      restore_source.include?("readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0") &&
      contains_in_order?(
        restore_source,
        [
          "cma_step10_main() {",
          "cma_step10_require_production_mutation_enabled",
          ': "${CMA_FINAL_POD:',
        ],
      ) &&
      release_gateway_prose.include?(
        "hard-disabled by a reviewed source constant in this evidence-tooling revision",
      ),
    "Gateway forward mutation entry points must remain hard-disabled while production evidence is open",
  )
  curl_invocation = /(?:^\s*curl |\$\(curl )/
  adoption_curl_lines = adoption_source.lines.select { |line| line.match?(curl_invocation) }
  restore_curl_lines = restore_source.lines.select { |line| line.match?(curl_invocation) }
  check(
    !adoption_curl_lines.empty? && !restore_curl_lines.empty? &&
      adoption_curl_lines.all? { |line| line.include?("curl --disable --noproxy '*'") } &&
      restore_curl_lines.all? { |line| line.include?("curl --disable --noproxy '*'") } &&
      contains_in_order?(
        restore_source,
        [
          "cma_step10_main() {",
          "cma_step10_require_trusted_launcher",
          "ulimit -c 0",
          "unset CURL_CA_BUNDLE CURL_SSL_BACKEND SSL_CERT_DIR SSL_CERT_FILE SSLKEYLOGFILE",
          ': "${CMA_GATEWAY_TOKEN_FD:',
          'if [ "${CMA_GATEWAY_TOKEN_FD}" != 8 ]',
          'IFS= read -r cma_release_access_token <&8',
          'exec 8<&-',
          "unset CMA_GATEWAY_TOKEN_FD CHRONO_RELEASE_ACCESS_TOKEN",
          "*$'\\r'*|*$'\\n'*)",
          "the protected release bearer must not contain CR or LF",
        ],
      ) &&
      adoption_source.scan(/unset .*SSLKEYLOGFILE/).length == 2 &&
      release_gateway_prose.include?("Every release curl must put `--disable` first") &&
      release_gateway_prose.include?("set `--noproxy '*'`") &&
      release_gateway_prose.include?("`SSLKEYLOGFILE` removed") &&
      release_gateway_prose.include?("`CURL_CA_BUNDLE`, `SSL_CERT_FILE`, `SSL_CERT_DIR`") &&
      release_gateway_prose.include?("rejects CR or LF in the bearer") &&
      release_gateway_prose.include?(
        "disables core dumps at process startup, before the inherited bearer can be handled",
      ),
    "Gateway release probes must ignore ambient curl, proxy, and TLS-keylog configuration",
  )
  check(
    release_doc.scan("CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer").length == 5 &&
      contains_in_order?(
        release_doc,
        [
          'CMA_GATEWAY_CONFIRM="adopt-${cma_gateway_release_tag}-route"',
          "CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer",
          "CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT=",
          'CMA_GATEWAY_CONFIRM="restore-${cma_gateway_release_tag}-route"',
          "CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer",
          "CMA_GATEWAY_ROLLBACK_SHA256=",
          'CMA_GATEWAY_CONFIRM="fence-${cma_gateway_release_tag}-traffic"',
          'CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM="${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM}"',
          'CMA_GATEWAY_CONFIRM="restore-${cma_gateway_release_tag}-traffic"',
        ],
      ),
    "Release adoption and restore must require the exclusive spec-writer attestation",
  )
  check(
    restore_source.include?(
      '(.paths["/api/v1/sessions"].get | type) == "object"',
    ),
    "Release traffic restoration must identify the CMA OpenAPI document",
  )
  step10_invocation = release_step10_invocation(release_doc)
  check(
    bash_syntax_valid?(step10_invocation),
    "Release traffic-restoration step must remain valid Bash",
  )
  check(
    contains_in_order?(
      step10_invocation,
      [
        "set +x",
        ': "${CHRONO_RELEASE_ACCESS_TOKEN:',
        'cma_gateway_release_tag_record="$(git --no-replace-objects for-each-ref',
        'CMA_GATEWAY_CONFIRM="restore-${cma_gateway_release_tag}-traffic"',
        'CMA_FINAL_DEPLOYMENT_UID="${cma_final_deployment_uid}"',
        'CMA_FINAL_REPLICA_SET="${cma_final_replica_set}"',
        'CMA_FINAL_REPLICA_SET_UID="${cma_final_replica_set_uid}"',
        'CMA_FINAL_POD="${cma_final_pod}"',
        'CMA_FINAL_POD_UID="${cma_final_pod_uid}"',
        'CMA_FINAL_POD_IP="${cma_final_pod_ip}"',
        'CMA_FINAL_POD_IMAGE="${cma_final_pod_image}"',
        'CMA_FINAL_POD_IMAGE_ID="${cma_final_pod_image_id}"',
        ".github/scripts/cma-trigger-gateway-launch.sh traffic",
        "unset CHRONO_RELEASE_ACCESS_TOKEN",
      ],
    ) && release_gateway_prose.include?(
      "Invoke the checked-in program only through `cma-trigger-gateway-launch.sh` from the clean checkout root; do not paste, source, or execute the traffic implementation directly",
    ),
    "Release documentation must invoke traffic restoration only through the reviewed launcher",
  )
  step10_custody_contracts = [
    "CMA_GATEWAY_ADMISSION_RECEIPT='<absolute-admission-receipt>'",
    "CMA_GATEWAY_ADMISSION_RECEIPT_SHA256='<externally-recorded-admission-receipt-SHA-256>'",
    "CMA_GATEWAY_FENCE_LABEL_RESERVATION_CONFIRM=live-denial-drill-v1",
    "CMA_GATEWAY_FENCE_RECEIPT_DIR='<absolute-off-host-fence-receipt-directory>'",
    "CMA_GATEWAY_FENCE_RECEIPT_SHA256='<externally-recorded-fence-receipt-SHA-256>'",
    "CMA_GATEWAY_FENCED_PROJECTION_PARENT='<absolute-off-host-fenced-projection-directory>'",
    "CMA_GATEWAY_FINAL_PROJECTION_PARENT='<absolute-off-host-final-projection-directory>'",
  ]
  check(
    step10_custody_contracts.all? { |contract| step10_invocation.include?(contract) },
    "Release traffic restoration must carry durable admission, fence-root, and projection custody evidence",
  )
  step10_script = release_step10_script(release_doc, restore_source)
  fence_body = shell_function_body(
    step10_script,
    "cma_step10_fence_main",
    "Dedicated traffic fencing must retain its checked entry point",
  )
  selector_body = shell_function_body(
    step10_script,
    "cma_read_selector_state",
    "Dedicated traffic fencing must retain its selector verifier",
  )
  drain_body = shell_function_body(
    step10_script,
    "cma_wait_for_fenced_endpoints",
    "Dedicated traffic fencing must retain its external drain verifier",
  )
  check(
    step10_script.scan(
      '(keys | sort) == ["apiVersion","kind","metadata","spec","status"]',
    ).length == 2 &&
      step10_script.scan(
        '"clusterIP","clusterIPs","internalTrafficPolicy","ipFamilies",',
      ).length == 2 &&
      step10_script.scan(
        '"cloud.google.com/neg","cloud.google.com/neg-status",',
      ).length == 2 &&
      step10_script.scan(
        '(.metadata.annotations["cloud.google.com/neg"] | fromjson) ==',
      ).length == 2 &&
      step10_script.include?(
        '(keys | sort) == ["apiVersion","items","kind","metadata"]',
      ) &&
      step10_script.include?('(.metadata | keys) == ["resourceVersion"]') &&
      step10_script.include?('jq -cer --arg service_uid "$2"') &&
      step10_script.include?('uid:$service_uid'),
    "Release traffic gate must close the Service and EndpointSlice roots",
  )
  check(
    contains_in_order?(
      step10_script,
      [
        "#!/bin/bash -p",
        "{ set +x; } 2>/dev/null",
        "umask 077",
        "ulimit -c 0",
        "set -euo pipefail",
        "cma_step10_sha256_stream() {",
        "cma_step10_sha256_file() {",
        "cma_step10_require_checkout() {",
        '"${#CMA_GATEWAY_RELEASE_REVISION}" -ne 40',
        "git --no-replace-objects",
        "rev-parse --verify 'HEAD^{commit}'",
        '"${cma_step10_head_revision}" != "${CMA_GATEWAY_RELEASE_REVISION}"',
        "-c core.fsmonitor=false",
        "-c core.hooksPath=/dev/null status",
        "--porcelain=v1 --untracked-files=all",
        'if [ -n "${cma_step10_dirty_paths}" ]',
        'if [ ! -f "${cma_step10_checker_path}" ]',
        '[ -L "${cma_step10_checker_path}" ]',
        'git --no-replace-objects cat-file blob \\',
        '"${CMA_GATEWAY_RELEASE_REVISION}:${cma_step10_checker_path}"',
        'cma_step10_sha256_file "${cma_step10_checker_path}"',
        '"${cma_step10_live_checker_sha256}" !=',
        '"${cma_step10_committed_checker_sha256}"',
        "cma_route_and_policies_ready() {",
        "cma_step10_require_checkout || return 1",
        "cma_step10_main() {",
        ': "${CMA_GATEWAY_RELEASE_REVISION:',
        "cma_step10_require_checkout",
      ],
    ),
    "Release traffic restoration must execute only from the clean reviewed commit",
  )
  check(
    contains_in_order?(
      step10_script,
      [
        "cma_step10_restore_path=.github/scripts/cma-trigger-gateway-restore-traffic.sh",
        'if [ ! -f "${cma_step10_restore_path}" ]',
        '[ -L "${cma_step10_restore_path}" ]',
        '"${CMA_GATEWAY_RELEASE_REVISION}:${cma_step10_restore_path}"',
        'cma_step10_sha256_file "${cma_step10_restore_path}"',
        '"${cma_step10_live_restore_sha256}" !=',
        '"${cma_step10_committed_restore_sha256}"',
      ],
    ) && release_gateway_prose.include?(
      "verifies the tag-derived confirmation, exact freeze attestation, clean reviewed commit, and its own executable Git blob",
    ),
    "Traffic-restoration program must byte-bind its own reviewed executable",
  )
  check(
    contains_in_order?(
      step10_script,
      [
        "cma_step10_resolve_release_tag() {",
        "git --no-replace-objects for-each-ref",
        '--points-at "${cma_release_revision}"',
        '"${cma_tag_object_type}" == tag',
        '"${cma_peeled_object_type}" == commit',
        '"${cma_peeled_revision}" == "${cma_release_revision}"',
        "cma_step10_require_checkout() {",
        'CMA_GATEWAY_RELEASE_TAG="$(cma_step10_resolve_release_tag',
        "cma_step10_fence_main() {",
        "cma_step10_require_confirmation fence traffic",
        "cma_step10_main() {",
        ': "${CMA_GATEWAY_CONFIRM:',
        "cma_step10_require_checkout",
        "cma_step10_require_confirmation restore traffic",
      ],
    ) && release_gateway_prose.include?(
      "derive the distinct destructive-intent guard as `restore-<exact-release-tag>-traffic` from the single annotated SemVer tag",
    ),
    "Traffic restoration must require its distinct destructive confirmation",
  )
  check(
    contains_in_order?(
      fence_body,
      [
        'if [ "$#" -ne 0 ]',
        "cma_step10_require_production_mutation_enabled || return 1",
        "cma_step10_require_privileged_bash || return 1",
        ': "${CMA_GATEWAY_CONFIRM:',
        "cma_step10_require_checkout || return 1",
        "cma_step10_require_confirmation fence traffic || return 1",
      ],
    ) && contains_in_order?(
      step10_script,
      [
        "--fence)",
        "shift",
        'cma_step10_fence_main "$@"',
      ],
    ) && step10_script.include?(
      "readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0",
    ),
    "Dedicated traffic fencing must remain hard-disabled before every boundary",
  )
  fence_cas =
    '-p=\'[{"op":"test","path":"/spec/selector","value":{"app.kubernetes.io/name":"cma-api"}},{"op":"replace","path":"/spec/selector","value":{"release.chrono-ai.fun/cutover-fence":"mongodb-715"}}]\''
  check(
    fence_body.include?(fence_cas) && contains_in_order?(
      fence_body,
      [
        'if .spec.selector == {"app.kubernetes.io/name":"cma-api"} then "source"',
        'elif .spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"} then "fenced"',
        'case "${cma_fence_initial_state}" in',
        "source)",
        "cma_parse_source_api_neg",
        "fenced)",
        "cma_parse_fenced_api_neg",
        'if [ "${cma_fence_initial_state}" = source ]; then',
        "cma_step10_kubectl patch service cma-api --type=json",
        fence_cas,
      ],
    ) && release_gateway_prose.include?(
      "rerun the reviewed `--fence` entrypoint",
    ),
    "Dedicated traffic fencing must remain an idempotent source-to-fence CAS",
  )
  check(
    contains_in_order?(
      fence_body,
      [
        "cma_fence_patch_status=0",
        "cma_step10_kubectl patch service cma-api --type=json",
        'cma_fence_patch_status=$?',
        'cma_fence_observed_state="$(cma_read_selector_state)"',
        'case "${cma_fence_observed_state}" in',
        "fenced) ;;",
        "if ! cma_wait_for_fenced_endpoints; then",
        'if [ "${cma_fence_patch_status}" -ne 0 ]; then',
        "the fence patch reported failure, but the exact applied fence and external drain were independently proved",
        '"CMA traffic is fenced and externally drained; receipt',
      ],
    ) && release_gateway_prose.include?(
      "A nonzero patch result is ambiguous rather than proof of failure",
    ),
    "Dedicated traffic fencing must reconcile ambiguous applied writes before success",
  )
  check(
    contains_in_order?(
      fence_body,
      [
        "cma_fence_label_has_no_pods",
        'cma_fence_initial_service="$(cma_step10_kubectl',
        "cma_parse_source_api_neg",
        "cma_parse_fenced_api_neg",
        'cma_pin_or_check_api_root "${cma_fence_initial_record}"',
        'cma_fence_observed_state="$(cma_read_selector_state)"',
        "if ! cma_wait_for_fenced_endpoints; then",
        '"CMA traffic is fenced and externally drained; receipt',
      ],
    ) && selector_body.include?(
      'cma_api_root_matches_pinned "${cma_selector_record}"',
    ) && contains_in_order?(
      drain_body,
      [
        "cma_fence_label_has_no_pods",
        "cma_parse_fenced_api_neg",
        "compute network-endpoint-groups list-network-endpoints",
        '"${cma_fence_neg_record_after}" != "${cma_fence_neg_record_before}"',
        '"${cma_fence_slice_record_after}" != "${cma_fence_slice_record_before}"',
        'cma_pin_or_check_api_root "${cma_fence_neg_record_before}"',
        "cma_fence_label_has_no_pods",
        "return 0",
      ],
    ),
    "Dedicated traffic fencing must pin Service/NEG identity and prove no-Pod external drain",
  )
  check(
    contains_in_order?(
      step10_script,
      [
        "{ set +x; } 2>/dev/null",
        "set -euo pipefail",
        "cma_step10_authenticated_curl() {",
        "curl --disable --noproxy '*' --fail --show-error --silent --max-time 15",
        "--header @/dev/fd/9",
        '9<<<"Authorization: Bearer ${cma_release_access_token}"',
        "cma_step10_main() {",
        ': "${CMA_GATEWAY_TOKEN_FD:',
        "unset cma_release_access_token cma_direct_livez cma_direct_ready cma_app_openapi",
        "unset cma_direct_sessions cma_app_sessions",
        "exec 9<&-",
        'IFS= read -r cma_release_access_token <&8',
        "exec 8<&-",
        "export -n cma_release_access_token",
        "unset CMA_GATEWAY_TOKEN_FD CHRONO_RELEASE_ACCESS_TOKEN",
        "cma_step10_require_checkout",
        "cma_step10_authenticated_curl",
        "trap '' HUP INT QUIT TERM",
        "unset cma_release_access_token cma_direct_livez cma_direct_ready cma_app_openapi",
        "unset cma_direct_sessions cma_app_sessions",
        '"CMA traffic restoration is complete and verified; final projection',
      ],
    ) && step10_script.scan(
      "unset cma_release_access_token cma_direct_livez cma_direct_ready cma_app_openapi",
    ).length == 2 &&
      !step10_script.include?("exec {cma_auth_fd}") &&
      !step10_script.include?(
      '--header "Authorization: Bearer ${CHRONO_RELEASE_ACCESS_TOKEN}"',
    ) && release_gateway_prose.include?(
      "only `curl` receives it through a separate protected descriptor rather than an argument",
    ),
    "Traffic restoration must keep its bearer credential out of traces, argv, and child environments",
  )
  check(
    contains_in_order?(
      step10_script,
      [
        "cma_step10_adoption_path=.github/scripts/cma-trigger-gateway-adoption.sh",
        'if [ ! -f "${cma_step10_adoption_path}" ]',
        '[ -L "${cma_step10_adoption_path}" ]',
        'git --no-replace-objects cat-file blob \\',
        '"${CMA_GATEWAY_RELEASE_REVISION}:${cma_step10_adoption_path}"',
        'cma_step10_sha256_file "${cma_step10_adoption_path}"',
        '"${cma_step10_live_adoption_sha256}" !=',
        '"${cma_step10_committed_adoption_sha256}"',
        "cma_step10_require_checkout",
        "cma_step10_validate_snapshot_seal",
      ],
    ) && release_gateway_prose.include?(
      "byte-binds the Gateway state checker, sourced adoption helper, API/web Deployment manifests, and API ConfigMap",
    ),
    "Release traffic restoration must byte-bind every sourced Gateway helper",
  )
  check(
    contains_in_order?(
      step10_script,
      [
        "cma_step10_require_checkout() {",
        'if [ "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM}" != sole-spec-writer ]',
        "cma_step10_main() {",
        ': "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:',
        "cma_step10_require_checkout",
        "cma_step10_validate_snapshot_seal",
        "if ! cma_final_workload_is_exact; then",
      ],
    ) && release_gateway_prose.include?(
      "Re-establish the full checked-routing-graph freeze",
    ) && release_gateway_prose.include?(
      "keep that window through the final post-open route/policy proof, success output, and re-fence trap removal",
    ),
    "Release traffic restoration must require and retain the checked-routing-graph freeze",
  )
  check(
    contains_in_order?(
      step10_script,
      [
        "cma_step10_validate_snapshot_seal() {",
        "source .github/scripts/cma-trigger-gateway-adoption.sh",
        'cma_gateway_validate_snapshot_seal "$1" "$2"',
        "cma_route_and_policies_ready() {",
        "cma_step10_require_checkout || return 1",
        "cma_step10_validate_snapshot_seal || return 1",
        "--check-adopted-from-snapshot",
        "cma_step10_main() {",
        ': "${CMA_GATEWAY_ROLLBACK_DIR:',
        ': "${CMA_GATEWAY_ROLLBACK_SHA256:',
        "cma_step10_require_checkout",
        "cma_step10_validate_snapshot_seal",
      ],
    ),
    "Release traffic restoration must authenticate and bind the retained adoption snapshot",
  )
  check(
    step10_script.scan("kubectl --request-timeout=15s").length >= 1 &&
      step10_script.scan("cma_step10_kubectl").length >= 7 &&
      step10_script.include?("kubectl --request-timeout=15s") &&
      step10_script.include?("timeout --signal=TERM --kill-after=5s 25s") &&
      contains_in_order?(
        step10_script,
        [
          "cma_step10_kubectl() {",
          "timeout --signal=TERM --kill-after=5s 25s",
          "kubectl --request-timeout=15s",
        ],
      ) && contains_in_order?(
        step10_script,
        [
          "cma_route_and_policies_ready() {",
          "timeout --signal=TERM --kill-after=5s 90s",
          ".github/scripts/cma-trigger-gateway-state.sh",
          "--check-adopted-from-snapshot",
        ],
      ),
    "Release traffic restoration must bound every cluster and Gateway check",
  )
  check(
    contains_in_order?(
      step10_script,
      [
        "cma_step10_gcloud() {",
        "timeout --signal=TERM --kill-after=5s 25s",
        "env -i",
        'HOME="${HOME}"',
        'PATH="${PATH}"',
        "LC_ALL=C",
        "CLOUDSDK_CORE_HTTP_TIMEOUT=15",
        'gcloud --quiet "$@"',
        "cma_parse_fenced_api_neg() {",
        '(.metadata.annotations["cloud.google.com/neg-status"] | fromjson)',
        'uid:.metadata.uid',
        'resourceVersion:.metadata.resourceVersion',
        "cma_parse_api_endpoint_slices() {",
        "cma_wait_for_fenced_endpoints() {",
        'cma_fence_deadline=$((SECONDS + 300))',
        'cma_fence_previous_empty_observation=""',
        'cma_step10_deadline_reached "${cma_fence_deadline}"',
        'cma_fence_service_before="$(cma_step10_kubectl get service cma-api -o json)"',
        "cma_fence_neg_record_before",
        "cma_fence_service_uid",
        "cma_fence_endpoints_json",
        "cma_fence_slice_record_before",
        "compute network-endpoint-groups list-network-endpoints",
        'if type == "array" then length',
        'cma_fence_service_after="$(cma_step10_kubectl get service cma-api -o json)"',
        "cma_fence_slice_record_after",
        '"${cma_fence_neg_record_after}" != "${cma_fence_neg_record_before}"',
        '"${cma_fence_slice_record_after}" != "${cma_fence_slice_record_before}"',
        'if [ "${cma_fence_negs_empty}" -eq 1 ]',
        '"${cma_fence_empty_observation}" =',
        '"${cma_fence_previous_empty_observation}"',
        "return 0",
        "cma_traffic_open=1",
        'trap \'cma_refence_on_exit "$?"\' EXIT',
        'if ! cma_initial_selector_state="$(cma_read_selector_state)"',
        'if [ "${cma_initial_selector_state}" != fenced ]',
        "if ! cma_wait_for_fenced_endpoints; then",
        "cma_traffic_open=0",
        "if ! cma_route_and_policies_ready; then",
      ],
    ) &&
      step10_script.scan(
        'cma_step10_deadline_reached "${cma_fence_deadline}"',
      ).length == 11 &&
      step10_script.include?(
        "the coherent empty NEG proof completed after the five-minute drain deadline",
      ) &&
      !step10_script.include?("cma_fence_endpoint_attempt") &&
      release_gateway_prose.include?(
      "Two consecutive all-empty sweeps under the same Service and EndpointSlice records are required",
      ) && release_gateway_prose.include?(
        "each EndpointSlice/NEG convergence operation has an explicit five-minute `SECONDS` deadline",
      ) && release_gateway_prose.include?(
        "Every Compute truth read starts `gcloud` under `env -i`",
      ),
    "Release traffic re-fence must prove coherent empty zonal NEGs",
  )
  workload_identity_contracts = [
    "cma_manifest_api_image() {",
    "YAML.safe_load(",
    "cma_final_workload_is_exact() {",
    'kind:"ReplicaSet",name:$replica_set,uid:$replica_set_uid',
    'kind:"Deployment",name:"cma-api",uid:$deployment_uid',
    '.spec.template.spec.containers[0].image == $image',
    '.status.containerStatuses[0].imageID == $image_id',
    'cma_manifest_image="$(cma_manifest_api_image)"',
    '"${cma_final_pod_image}" != "${cma_manifest_image}"',
    'cma_final_pod_image_id}" =~ ^(docker-pullable://)?us-central1-docker',
  ]
  check(
    step10_script.scan("if ! cma_wait_for_reopened_endpoint; then").length == 2 &&
      step10_script.scan("if ! cma_final_workload_is_exact; then").length == 2 &&
      workload_identity_contracts.all? { |contract| step10_script.include?(contract) } &&
      contains_in_order?(
        step10_script,
        [
          "cma_parse_source_api_neg() {",
          "cma_wait_for_reopened_endpoint() {",
          'cma_reopen_deadline=$((SECONDS + 300))',
          'cma_step10_deadline_reached "${cma_reopen_deadline}"',
          'addresses:[$ip]',
          'targetRef:{kind:"Pod",name:$pod,namespace:"chrono-bot",uid:$uid}',
          "compute network-endpoint-groups list-network-endpoints",
          '"${cma_reopen_neg_record_after}" !=',
          '"${cma_reopen_neg_record_before}"',
          '"${cma_reopen_slice_record_after}" !=',
          '"${cma_reopen_slice_record_before}"',
          'length == 1 and .[0].ipAddress == $ip and .[0].port == 8080',
          'cma_reopen_projection="$(jq -cn',
          "cma_reopen_committed_projection",
          "if ! cma_wait_for_reopened_endpoint; then",
          "https://bot-api.chrono-ai.fun/livez",
          'cma_abort_after_reopen "the app-origin route or Gateway policy drifted during traffic restoration"',
          "if ! cma_wait_for_reopened_endpoint; then",
          "if ! cma_final_workload_is_exact; then",
        ],
      ) &&
        step10_script.scan(
          'cma_step10_deadline_reached "${cma_reopen_deadline}"',
        ).length == 9 &&
        step10_script.include?(
          "the exact reopened projection completed after the five-minute deadline",
        ) &&
        !step10_script.include?("cma_reopen_endpoint_attempt") &&
        release_gateway_prose.include?(
        "NEG population must contain exactly one endpoint with that same pod IP and port `8080`",
        ) && release_gateway_prose.include?(
          "byte-identical to the first successful normalized Service, EndpointSlice, and NEG projection",
        ) && release_gateway_prose.include?(
          "re-reads the complete Deployment-to-ReplicaSet-to-Pod chain, full workload specs, immutable imageID, Pod IP, readiness, started state, and zero restart count immediately before commit",
        ) && contains_in_order?(
          step10_script,
          [
            'cma_pinned_api_root_identity=""',
            "cma_pin_or_check_api_root() {",
            'cma_pinned_api_root_identity="${cma_observed_api_root}"',
            "cma_wait_for_fenced_endpoints() {",
            'cma_pin_or_check_api_root "${cma_fence_neg_record_before}"',
            "cma_wait_for_reopened_endpoint() {",
            'cma_api_root_matches_pinned "${cma_reopen_neg_record_before}"',
          ],
        ) && release_gateway_prose.include?(
          "pins the `cma-api` Service UID and its exact NEG name and zone set for the whole transaction",
        ),
    "Release traffic reopen must bind the exact EndpointSlice and zonal NEG population",
  )
  check(
    contains_in_order?(
      step10_script,
      [
        "cma_refence_on_exit() {",
        "trap '' HUP INT QUIT TERM",
        "exec 9<&-",
        'cma_exit_status="$1"',
        "trap - EXIT",
        "cma_refence_api",
        "cma_refence_on_signal() {",
        "trap '' HUP INT QUIT TERM",
        "exec 9<&-",
        "exit 1",
        'trap \'cma_refence_on_exit "$?"\' EXIT',
      ],
    ) && contains_in_order?(
      step10_script,
      [
        "cma_step10_validate_snapshot_seal",
        "trap '' HUP INT QUIT TERM",
        "cma_traffic_open=1",
        'trap \'cma_refence_on_exit "$?"\' EXIT',
        "trap cma_refence_on_signal HUP INT QUIT TERM",
        "if ! cma_final_workload_is_exact; then",
      ],
    ) && contains_in_order?(
      step10_script,
      [
        'cma_abort_after_reopen "the app-origin route or Gateway policy drifted during traffic restoration"',
        "trap '' HUP INT QUIT TERM",
        "unset cma_release_access_token cma_direct_livez cma_direct_ready cma_app_openapi",
        "unset cma_direct_sessions cma_app_sessions",
        '"CMA traffic restoration is complete and verified; final projection',
        "cma_traffic_open=0",
        "trap - EXIT",
        "trap - HUP INT QUIT TERM",
      ],
    ),
    "Release traffic restoration must mask signals during re-fencing and success disarm",
  )
end

def load_gateway_support_sources
  {
    projection: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-projection.rb")),
    capture: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-projection.sh")),
    admission_bootstrap: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-admission-bootstrap.sh")),
    cloudflare: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-cloudflare-capture.sh")),
    tls_sanitizer: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-tls-sanitize.rb")),
    launcher: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-launch.sh")),
    origin: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-origin-evidence.sh")),
    live_policy: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-live-policy.json")),
    cloudflare_policy: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-cloudflare-policy.json")),
    admission: File.read(File.join(ROOT, ".github/scripts/cma-trigger-gateway-cutover-fence-policy.yaml")),
  }
end

def verify_gateway_release_contract(
  ci_workflow,
  release_doc,
  state_source,
  adoption_source,
  source_contract_source,
  restore_source = nil,
  support_sources = nil
)
  restore_source ||= File.read(GATEWAY_RESTORE_TRAFFIC_PATH)
  files = support_sources || load_gateway_support_sources
  executable_paths = %w[
    .github/scripts/cma-trigger-gateway-state.sh
    .github/scripts/cma-trigger-gateway-adoption.sh
    .github/scripts/cma-trigger-gateway-adoption-tests.sh
    .github/scripts/cma-trigger-gateway-restore-traffic.sh
    .github/scripts/cma-trigger-gateway-restore-traffic-tests.sh
    .github/scripts/cma-trigger-gateway-projection.rb
    .github/scripts/cma-trigger-gateway-projection.sh
    .github/scripts/cma-trigger-gateway-projection-tests.sh
    .github/scripts/cma-trigger-gateway-tls-sanitize.rb
    .github/scripts/cma-trigger-gateway-tls-sanitize-tests.sh
    .github/scripts/cma-trigger-gateway-admission-bootstrap.sh
    .github/scripts/cma-trigger-gateway-admission-bootstrap-tests.sh
    .github/scripts/cma-trigger-gateway-cloudflare-capture.sh
    .github/scripts/cma-trigger-gateway-cloudflare-capture-tests.sh
    .github/scripts/cma-trigger-gateway-launch.sh
    .github/scripts/cma-trigger-gateway-launch-tests.sh
    .github/scripts/cma-trigger-gateway-origin-evidence.sh
    .github/scripts/cma-trigger-gateway-origin-evidence-tests.sh
  ]
  executable_paths.each do |path|
    check(File.executable?(File.join(ROOT, path)), "Gateway release program must be executable: #{path}")
  end

  mutation_sources = [
    adoption_source, restore_source, files[:capture], files[:admission_bootstrap],
    files[:cloudflare], files[:launcher], files[:origin],
  ]
  check(
    mutation_sources.all? { |source| source.include?("readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0") },
    "every Gateway release entrypoint must retain the hard-disabled production mutation constant",
  )
  check(
    JSON.parse(files[:live_policy]).fetch("status") == "blocked-pending-reviewed-live-capture" &&
      JSON.parse(files[:cloudflare_policy]).fetch("status") == "blocked-pending-reviewed-cloudflare-capture" &&
      JSON.parse(files[:cloudflare_policy]).fetch("sourceEvidenceSha256").nil? &&
      JSON.parse(files[:cloudflare_policy]).fetch("accountId").nil?,
    "reviewed live-routing and Cloudflare policies must remain blocked until evidence is approved",
  )

  reviewed_paths = %w[
    infra/api/deployment.yaml
    infra/web/deployment.yaml
    infra/api/configmap.yaml
    .github/scripts/cma-trigger-gateway-cutover-fence-policy.yaml
    .github/scripts/cma-trigger-gateway-live-policy.json
    .github/scripts/cma-trigger-gateway-cloudflare-capture.sh
    .github/scripts/cma-trigger-gateway-cloudflare-policy.json
    .github/scripts/cma-trigger-gateway-projection.rb
    .github/scripts/cma-trigger-gateway-projection.sh
    .github/scripts/cma-trigger-gateway-origin-evidence.sh
    .github/scripts/cma-trigger-gateway-launch.sh
    .github/scripts/cma-trigger-gateway-restore-traffic.sh
  ]
  reviewed_source_sets = [
    shell_readonly_words(
      files[:admission_bootstrap],
      "CMA_ADMISSION_SOURCE_PATHS",
      "admission source seal must be one closed checked-in path list",
    ),
    shell_readonly_words(
      adoption_source,
      "CMA_GATEWAY_SOURCE_PATHS",
      "adoption source seal must be one closed checked-in path list",
    ),
    shell_readonly_words(
      restore_source,
      "CMA_GATEWAY_REVIEWED_SOURCE_PATHS",
      "traffic source seal must be one closed checked-in path list",
    ),
  ]
  check(
    reviewed_source_sets.all? do |source_paths|
      reviewed_paths.all? { |path| source_paths.include?(path) }
    end,
    "admission, adoption, and traffic restoration must byte-seal every behavior-bearing release source",
  )

  release_tag_common_contracts = [
    "git --no-replace-objects for-each-ref",
    "--format='%(refname:strip=2)|%(objecttype)|%(*objecttype)|%(*objectname)'",
    "'refs/tags/v*'",
    "the reviewed release revision must have exactly one annotated SemVer release tag",
  ]
  tag_sources = [
    [
      files[:admission_bootstrap],
      [
        '"${tag_record}" != *$\'\\n\'*', '"${tag_name}" =~ ^v(0|[1-9][0-9]*)',
        '"${tag_object_type}" == tag', '"${peeled_object_type}" == commit',
        '"${peeled_revision}" == "${release_revision}"',
      ],
    ],
    [
      adoption_source,
      [
        '"${tag_record}" != *$\'\\n\'*', '"${tag_name}" =~ ^v(0|[1-9][0-9]*)',
        '"${tag_object_type}" == tag', '"${peeled_object_type}" == commit',
        '"${peeled_revision}" == "${release_revision}"',
      ],
    ],
    [
      restore_source,
      [
        '"${cma_tag_record}" != *$\'\\n\'*', '"${cma_tag_name}" =~ ^v(0|[1-9][0-9]*)',
        '"${cma_tag_object_type}" == tag', '"${cma_peeled_object_type}" == commit',
        '"${cma_peeled_revision}" == "${cma_release_revision}"',
      ],
    ],
  ]
  check(
    tag_sources.all? do |source, specific_contracts|
      release_tag_common_contracts.all? { |contract| source.include?(contract) } &&
        specific_contracts.all? { |contract| source.include?(contract) }
    end &&
      files[:admission_bootstrap].include?(
        'local expected_confirmation="bootstrap-${CMA_GATEWAY_RELEASE_TAG}-admission"',
      ) &&
      adoption_source.include?(
        'local expected_confirmation="${action}-${CMA_GATEWAY_RELEASE_TAG}-${scope}"',
      ) &&
      restore_source.include?(
        'local cma_expected_confirmation="${cma_action}-${CMA_GATEWAY_RELEASE_TAG}-${cma_scope}"',
      ),
    "every mutating Gateway entrypoint must derive confirmation from one annotated exact SemVer release tag",
  )

  check(
    release_doc.include?(".github/scripts/cma-trigger-gateway-launch.sh adoption --adopt") &&
      contains_in_order?(
        release_doc,
        [
          ".github/scripts/cma-trigger-gateway-launch.sh adoption",
          "--restore '<retained-sealed-rollback-directory>'",
          ".github/scripts/cma-trigger-gateway-launch.sh traffic --fence",
          ".github/scripts/cma-trigger-gateway-launch.sh traffic ||",
        ],
      ) &&
      !release_doc.include?(".github/scripts/cma-trigger-gateway-adoption.sh") &&
      !release_doc.include?(".github/scripts/cma-trigger-gateway-restore-traffic.sh"),
    "Release runbook must invoke Gateway mutations only through the trusted launcher",
  )
  check(
    release_doc.include?(
      ".github/scripts/cma-trigger-gateway-launch.sh traffic --prove-fence",
    ) && release_doc.include?(
      'CMA_GATEWAY_CONFIRM="prove-${cma_gateway_release_tag}-fence"',
    ) && release_doc.include?(
      ': "${CMA_GATEWAY_FENCE_RECEIPT_SHA256:?export the independently recorded fence-receipt SHA-256}"',
    ),
    "Release runbook must retain the independently authenticated post-scale fence proof",
  )

  cloudflare_contracts = [
    "CMA_CF_EVIDENCE_FILES", "completedAt", "status=all",
    "/url_normalization", "/cache/variants", "/argo/tiered_caching",
    "/workers/domains?hostname=", "/access/apps", "/waiting_rooms",
    "/firewall/rules?page=1&per_page=100", "/rate_limits?page=1&per_page=1000",
    "accountRulesets", "originTlsClientAuthHostnames", "--derive-policy",
    "sourceEvidenceSha256", "behaviorProjection:.behaviorProjection",
  ]
  check(
    cloudflare_contracts.all? { |contract| files[:cloudflare].include?(contract) },
    "Cloudflare capture must close behavior roots and derive policy only from exact evidence",
  )

  projection_contracts = [
    "policySha256", "sourceEvidenceSha256", "usedBy",
    "route_attaches_to_shared_gateway?", "hostname-less HTTPRoute",
    '"ready" => true, "serving" => true, "terminating" => false',
    "EndpointSlice nodeName does not bind its Pod", "containerStartedAt",
    "validate_health_policy", "validate_backend_policy", "Attached at its current generation",
    "validate_fence_admission", "publicSpkiSha256",
    "private key does not match its leaf certificate", "validate_backend_health",
    "unhealthy or unknown load-balancer endpoint", "firewall capture",
    "unsupported graph annotation", "stable unannotated forwarding rule",
    "stable mismatched certificate private key", "--derive-policy",
  ]
  check(
    projection_contracts.all? { |contract| files[:projection].include?(contract) },
    "closed Gateway projection must bind desired, controller, TLS, endpoint, provider, and Compute behavior roots",
  )
  capture_contracts = [
    "get secret", "CMA_GATEWAY_TLS_SANITIZER", "get validatingadmissionpolicy",
    "forwarding-rules list --global", "networking.gke.io/firewalls",
    "backend-services get-health", "list-network-endpoints", "--check-live",
  ]
  check(
    capture_contracts.all? { |contract| files[:capture].include?(contract) } &&
      files[:tls_sanitizer].include?("publicSpkiSha256") &&
      !files[:tls_sanitizer].include?("privateKeySha256"),
    "live projection capture must enumerate the complete Gateway graph and backend health",
  )
  check(
    contains_in_order?(
      files[:tls_sanitizer],
      [
        "certificate_remainder = certificate_source",
        "certificate_pattern = /\\A[[:space:]]*",
        "certificate_blocks << match[1]",
        'raise ArgumentError unless certificate_remainder.match?(/\A[[:space:]]*\z/)',
        "certificates = certificate_blocks.map { |block| OpenSSL::X509::Certificate.new(block) }",
        "certificate_pem = certificates.map(&:to_pem).join",
        "leaf = certificates.fetch(0)",
        "raise ArgumentError unless public_key.verify",
        '"certificatePem" => certificate_pem',
        "File.open(output_path",
      ],
    ) && !files[:tls_sanitizer].include?('"certificatePem" => certificate_source'),
    "Gateway TLS sanitizer must reject residual PEM and publish only parsed public certificates",
  )

  launcher_contracts = [
    "#!/bin/bash -p", "/usr/bin/env -i", "/bin/bash --noprofile --norc -p",
    "reviewed-host-toolchain", "CMA_GATEWAY_TOKEN_FD=8",
    "printf '%s\\n' \"${cma_launcher_token}\" | (", "exec 8<&0", "exec 0</dev/null",
    "CMA_GATEWAY_CLOUDFLARE_TOKEN_FD=7", "CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID",
    "projection)", "cloudflare)", "origin)", "adoption)", "traffic)",
  ]
  check(
    launcher_contracts.all? { |contract| files[:launcher].include?(contract) } &&
      !files[:launcher].include?("exec 8<<<") &&
      restore_source.include?("cma_step10_require_trusted_launcher") &&
      restore_source.include?('IFS= read -r cma_release_access_token <&8') &&
      restore_source.include?("exec 8<&-") &&
      !restore_source.include?(': "${CHRONO_RELEASE_ACCESS_TOKEN:'),
    "trusted env-i launcher must own one-read FD bearer custody for traffic restoration",
  )

  lifecycle_contracts = [
    "CMA_GATEWAY_ADOPTED_PROJECTION_PARENT", "adopted projection capture",
    "cma_step10_write_fence_receipt", "durable-prepatch-root-v1",
    "cma_step10_load_fence_receipt", "already-fenced Service requires its existing",
    "cma_step10_validate_admission_reservation", "live-denial-drill-v1",
    "fenced-pre-reopen", "cma_step10_validate_fence_projection",
    "CMA_GATEWAY_FINAL_PROJECTION_PARENT", "the stable final closed projection could not be captured",
  ]
  lifecycle_source = adoption_source + restore_source
  check(
    lifecycle_contracts.all? { |contract| lifecycle_source.include?(contract) },
    "release tooling must seal adopted, durable fence-root, fenced, pre-reopen, and final projections",
  )
  check(
    contains_in_order?(
      adoption_source,
      [
        "cma_gateway_validate_restored_projection() {",
        "cma_gateway_validate_projection_receipt 0",
        "CMA_GATEWAY_PROJECTION_STAGE=rollback-source",
        'CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256="${CMA_GATEWAY_PROJECTION_SHA256}"',
        "--capture \"${rollback_projection_dir}\" source",
        "cma_gateway_assert_source_seal",
        "cma_gateway_compare_projection_behavior",
        '"${source_projection}" "${rollback_projection_dir}/projection.json"',
      ],
    ) &&
      files[:projection].include?("def compare_projection_behavior(source, candidate)") &&
      files[:projection].include?('compute_behavior_policy(source["fullCapture"])') &&
      files[:projection].include?('compute_behavior_policy(candidate["fullCapture"])') &&
      files[:projection].include?(
        'canonical_json(source_behavior) == canonical_json(candidate_behavior)',
      ) &&
      files[:capture].include?('rollback-source:source'),
    "Gateway rollback success must compare a fresh full closed projection with sealed source behavior",
  )

  origin_contracts = [
    "curl --disable --noproxy '*'", "--resolve", "%{remote_ip}", "%{ssl_verify_result}",
    "pseudoIPv4 == \"Disabled\"", "removeVisitorIpHeaders == false",
    "identityHeaderWorkers == []", "identityHeaderTransforms == []",
    "CMA_CLOUDFLARE_IPV4", "CMA_CLOUDFLARE_IPV6", "behaviorProjection ==",
    "cma_dns_exact", "cloudflare-pre", "cloudflare-post", "cma_probe=",
    "cacheStatus: \"DYNAMIC\"", "writers-disabled-through-origin-probes",
    "behaviorPolicyFileSha256", "receipt.sha256", "cma_origin_fsync",
  ]
  check(
    origin_contracts.all? { |contract| files[:origin].include?(contract) },
    "origin/public evidence must bind TLS, remote IP, Cloudflare policy/CIDRs, and a durable receipt",
  )
  check(
    files[:admission].include?("kind: ValidatingAdmissionPolicy") &&
      files[:admission].include?("kind: ValidatingAdmissionPolicyBinding") &&
      files[:admission].include?("failurePolicy: Fail") &&
      files[:admission].include?("validationActions:") &&
      files[:admission].include?("- Deny") &&
      files[:admission].include?("release.chrono-ai.fun/cutover-fence"),
    "the reserved cutover-fence label must be enforced by fail-closed admission policy",
  )

  contract_job = required_hash(
    ci_workflow.fetch("jobs")["trigger-release-contract"],
    "ci.yml must retain the Trigger release contract job",
  )
  contract_run = required_step(
    required_steps(contract_job, "trigger-release-contract"),
    "Runtime provenance and release evidence stay mandatory",
  ).fetch("run", "").to_s
  core_ci_commands = [
    "set -euo pipefail",
    "ruby -c .github/scripts/cma-trigger-gateway-source-contract.rb",
    "ruby .github/scripts/cma-trigger-gateway-source-contract.rb",
    "ruby .github/scripts/cma-trigger-gateway-source-contract.rb --self-test",
    ".github/scripts/cma-trigger-gateway-state.sh --self-test",
    ".github/scripts/cma-trigger-gateway-adoption.sh --self-test",
    ".github/scripts/cma-trigger-gateway-adoption-tests.sh",
    ".github/scripts/cma-trigger-gateway-restore-traffic-tests.sh",
    "ruby .github/scripts/cma-trigger-publish-contract.rb",
    "ruby .github/scripts/cma-trigger-publish-contract.rb --self-test",
    "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10 | bash -n",
    "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10 | shellcheck -s bash -",
    "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10-invocation | bash -n",
    "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10-invocation | shellcheck -s bash -",
  ]
  check(
    core_ci_commands.all? { |command| contract_run.include?(command) },
    "CI must syntax-check and self-test every Gateway release contract",
  )
  check(
    contract_run.include?("shellcheck -x .github/scripts/cma-trigger-gateway-*.sh"),
    "CI must run full ShellCheck over every checked-in Gateway shell program",
  )
  behavior_ci_commands = [
    "ruby -c .github/scripts/cma-trigger-gateway-tls-sanitize.rb",
    "ruby -c .github/scripts/cma-trigger-gateway-projection.rb",
    "ruby .github/scripts/cma-trigger-gateway-projection.rb --self-test",
    "bash -n .github/scripts/cma-trigger-gateway-*.sh",
    "bash .github/scripts/cma-trigger-gateway-tls-sanitize-tests.sh",
    "bash .github/scripts/cma-trigger-gateway-projection-tests.sh",
    "bash .github/scripts/cma-trigger-gateway-admission-bootstrap-tests.sh",
    "bash .github/scripts/cma-trigger-gateway-cloudflare-capture-tests.sh",
    "bash .github/scripts/cma-trigger-gateway-launch-tests.sh",
    "bash .github/scripts/cma-trigger-gateway-origin-evidence-tests.sh",
  ]
  check(
    behavior_ci_commands.all? { |command| contract_run.include?(command) },
    "CI must syntax-check, shellcheck, and behavior-test all Gateway release tooling",
  )

  release_prose = release_doc.gsub(/\s+/, " ")
  tag_record_command = 'cma_gateway_release_tag_record="$(git --no-replace-objects for-each-ref'
  check(
    release_doc.scan(tag_record_command).length == 5 &&
      release_doc.scan('--points-at "${CMA_GATEWAY_RELEASE_REVISION}"').length == 5 &&
      release_doc.scan("%(objecttype)|%(*objecttype)|%(*objectname)").length == 5 &&
      release_doc.scan('cma_gateway_release_tag="${BASH_REMATCH[1]}"').length == 5 &&
      !release_doc.include?("git describe --tags"),
    "Release runbook must derive every confirmation from one annotated exact SemVer tag",
  )

  manual_restore_body = shell_function_body(
    adoption_source,
    "cma_gateway_manual_restore",
    "manual restore must retain its closed pre-write entrypoint",
  )
  projection_receipt_body = shell_function_body(
    adoption_source,
    "cma_gateway_validate_projection_receipt",
    "projection receipt validation must retain its closed lifecycle validator",
  )
  manual_restore_invocation = release_manual_restore_invocation(release_doc)
  check(
    contains_in_order?(
      manual_restore_body,
      [
        '${CMA_GATEWAY_ADMISSION_RECEIPT:-}" == /*',
        '${CMA_GATEWAY_ADMISSION_RECEIPT_SHA256:-}" =~ ^[0-9a-f]{64}$',
        "cma_gateway_verify_admission_receipt",
        "cma_gateway_prepare_desired",
        'cma_gateway_restore "${snapshot_dir}"',
      ],
    ) && [
      "CMA_GATEWAY_ADMISSION_RECEIPT='<absolute-admission-receipt>'",
      "CMA_GATEWAY_ADMISSION_RECEIPT_SHA256='<externally-recorded-admission-receipt-SHA-256>'",
    ].all? { |contract| manual_restore_invocation.include?(contract) } &&
      release_prose.include?(
        "stops before any restore write if that custody or enforcement proof is unavailable",
      ),
    "Gateway manual restore must authenticate admission custody before every write",
  )
  check(
    contains_in_order?(
      manual_restore_body,
      [
        "cma_gateway_validate_projection_receipt 0",
        "cma_gateway_prepare_desired",
        'cma_gateway_restore "${snapshot_dir}"',
      ],
    ) && contains_in_order?(
      projection_receipt_body,
      [
        'cmp "${candidate_file}" "${projection_file}"',
        "jq -se '",
        ".schemaVersion == 1 and .mode == \"source\"",
        '.lifecycle == {stage:"source",predecessorSha256:"genesis"}',
        '"${projection_dir}/capture.json" "${projection_file}"',
      ],
    ) && release_prose.include?(
      "capture-to-projection regeneration before its first restore write",
    ) && release_prose.include?(
      'the exact pre-adoption lifecycle root `{stage:"source",predecessorSha256:"genesis"}`',
    ),
    "Gateway manual restore must validate source projection custody before every write",
  )

  workload_contracts = [
    "def validate_api_workload", "default_pod_template_spec",
    "expected_service_account_projection", '"command"', '"env"',
    '"securityContext"', '"readinessProbe"', '"volumes"',
    'ARGV[0] == "--validate-workload"',
  ]
  check(
    workload_contracts.all? { |contract| files[:projection].include?(contract) } &&
      contains_in_order?(
        restore_source,
        [
          "cma_step10_validate_workload_contract() {",
          "--validate-workload infra/api/deployment.yaml",
          "cma_final_workload_is_exact() {",
          'cma_deployment_json="$(cma_step10_kubectl get deployment cma-api -o json)"',
          "cma_step10_validate_workload_contract",
        ],
      ) && release_doc.include?(
        "--validate-workload infra/api/deployment.yaml",
      ) && release_prose.include?(
        "Command, args, environment, `envFrom`, service account, security context, probes, resources, affinity, volumes, mounts, container count",
      ),
    "Release traffic workload proof must bind every live spec to the reviewed Deployment manifest",
  )

  prove_fence_body = shell_function_body(
    restore_source,
    "cma_step10_prove_fence_main",
    "post-scale fence proof must retain its closed entrypoint",
  )
  scaled_workload_body = shell_function_body(
    restore_source,
    "cma_step10_validate_scaled_down_workload",
    "post-scale fence proof must retain its zero-workload verifier",
  )
  check(
    contains_in_order?(
      prove_fence_body,
      [
        "cma_step10_load_fence_receipt",
        'cma_predecessor_projection="${CMA_GATEWAY_FENCED_PROJECTION_PARENT:-}/fenced-pre-scale-',
        "cma_step10_validate_fence_projection_receipt",
        "cma_step10_validate_scaled_down_workload",
        "cma_wait_for_fenced_endpoints",
        "cma_step10_projection_capture",
      ],
    ) && [
      ".spec.replicas == 0", '(.status.replicas // 0) == 0',
      '.items == []', "--ignore-not-found -o json",
    ].all? { |contract| scaled_workload_body.include?(contract) } &&
      files[:capture].include?("get deployment cma-api -o json") &&
      files[:capture].include?("apiDeploymentScale") &&
      files[:projection].include?("api_workload_scale_projection") &&
      files[:projection].include?('lifecycle_stage == "fenced-post-scale" ? 0 : 1') &&
      release_prose.include?("independently reads every recorded predecessor Pod name"),
    "Post-scale fence proof must bind predecessor Pod deletion and stable zero replicas before publication",
  )

  fence_body = shell_function_body(
    restore_source,
    "cma_step10_fence_main",
    "traffic fence must retain its closed entrypoint",
  )
  check(
    contains_in_order?(
      fence_body,
      [
        "cma_step10_write_fence_receipt",
        "CMA durable fence root committed before Service mutation",
        "cma_step10_kubectl patch service cma-api --type=json",
      ],
    ) && release_prose.include?(
      "receipt acknowledgement is printed before the Service mutation",
    ),
    "Traffic fence must surface durable receipt custody before its Service mutation",
  )

  receipt_writer_body = shell_function_body(
    restore_source,
    "cma_step10_write_fence_receipt",
    "fence receipt must retain its durable writer",
  )
  projection_publisher_body = shell_function_body(
    files[:capture],
    "cma_projection_publish_directory",
    "projection capture must retain its atomic publisher",
  )
  check(
    receipt_writer_body.include?("File.rename(source, destination)") &&
      !receipt_writer_body.include?("File.open(File.dirname(destination)") &&
      restore_source.scan("cma_step10_sync_published_fence_bundle").length == 4 &&
      projection_publisher_body.include?("File.rename(source, destination)") &&
      !projection_publisher_body.include?("fsync") &&
      files[:capture].scan("cma_projection_sync_published_directory").length == 5 &&
      files[:capture].include?("concurrent projection publication could not be durably re-synced"),
    "Fence receipt and projection publication must prove durability after atomic rename",
  )

  external_projection_digests = %w[
    CMA_GATEWAY_SOURCE_PROJECTION_SHA256
    CMA_GATEWAY_SOURCE_PROJECTION_PREDECESSOR_SHA256
    CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256
    CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256
  ]
  check(
    external_projection_digests.all? { |name| files[:launcher].include?(name) } &&
      files[:launcher].include?('"${cma_launcher_variable}=${!cma_launcher_variable}"'),
    "Gateway launcher must preserve every external projection digest",
  )
  step10_invocation = release_step10_invocation(release_doc)
  check(
    [
      "CMA_GATEWAY_SOURCE_PROJECTION_SHA256='<independently-recorded-adopted-projection-SHA-256>'",
      "CMA_GATEWAY_SOURCE_PROJECTION_PREDECESSOR_SHA256='<independently-recorded-source-projection-SHA-256>'",
    ].all? { |contract| release_doc.include?(contract) } && [
      ': "${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256:',
      'CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256}"',
      ': "${CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256:',
      'CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256="${CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256}"',
    ].all? { |contract| step10_invocation.include?(contract) } &&
      release_prose.include?(
        "the restore process will not trust a digest recovered from the colocated projection receipt",
      ) && release_prose.include?(
        "Every successor validator receives and enforces the exact expected lifecycle stage and predecessor digest again after capture publication",
      ),
    "Gateway runbook must carry every external projection digest between processes",
  )

  fence_projection_validator = shell_function_body(
    restore_source,
    "cma_step10_validate_fence_projection_receipt",
    "fenced projection must retain its exact lifecycle validator",
  )
  projection_capture_body = shell_function_body(
    restore_source,
    "cma_step10_projection_capture",
    "projection transition must retain its external predecessor validator",
  )
  projection_file_body = shell_function_body(
    restore_source,
    "cma_step10_require_projection_file",
    "adopted projection must retain its external digest validator",
  )
  check(
    contains_in_order?(
      projection_capture_body,
      [
        'local cma_expected_predecessor_digest="$7"',
        'local cma_expected_predecessor_predecessor_digest="$8"',
        'cma_predecessor_digest="$(cma_step10_sha256_file "${cma_predecessor_file}")"',
        'cma_step10_sha256_valid "${cma_expected_predecessor_digest}"',
        '[ "${cma_predecessor_digest}" = "${cma_expected_predecessor_digest}" ]',
        'cma_recorded_digest="$(awk',
        '[ "${cma_recorded_digest}" = "${cma_predecessor_digest}" ]',
        "cma_step10_validate_fence_projection_receipt",
        '"${cma_expected_predecessor_stage}"',
        '"${cma_expected_predecessor_predecessor_digest}"',
      ],
    ) && contains_in_order?(
      projection_file_body,
      [
        'local cma_expected_digest="$2"',
        'local cma_expected_predecessor_digest="$3"',
        'cma_step10_sha256_valid "${cma_expected_digest}"',
        'cma_step10_sha256_valid "${cma_expected_predecessor_digest}"',
        '[ "${cma_live_digest}" = "${cma_expected_digest}" ]',
        '.lifecycle == {stage:"adopted",predecessorSha256:$predecessor}',
      ],
    ) && contains_in_order?(
      fence_body,
      [
        'cma_source_projection_sha256="${CMA_GATEWAY_SOURCE_PROJECTION_SHA256:-}"',
        'cma_source_projection_predecessor_sha256="${CMA_GATEWAY_SOURCE_PROJECTION_PREDECESSOR_SHA256:-}"',
        "cma_step10_require_projection_file",
        '"${cma_source_projection_sha256}"',
        '"${cma_source_projection_predecessor_sha256}"',
        "cma_step10_write_fence_receipt",
        "fenced-pre-scale \"${cma_source_projection}\" adopted",
        '"${cma_source_projection_sha256}"',
      ],
    ) && contains_in_order?(
      prove_fence_body,
      [
        'cma_predecessor_projection_sha256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256:-}"',
        'cma_step10_sha256_valid "${cma_predecessor_projection_sha256}"',
        "fenced-post-scale \"${cma_predecessor_projection}\" fenced-pre-scale",
        '"${cma_predecessor_projection_sha256}"',
      ],
    ) && contains_in_order?(
      restore_source,
      [
        'cma_pre_scale_projection_sha256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256:-}"',
        'cma_post_scale_projection_sha256="${CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256:-}"',
        'cma_step10_sha256_valid "${cma_pre_scale_projection_sha256}"',
        'cma_step10_sha256_valid "${cma_post_scale_projection_sha256}"',
        "fenced-pre-reopen \"${cma_post_scale_projection}\" fenced-post-scale",
        '"${cma_post_scale_projection_sha256}"',
        "final-reopen \"${cma_fenced_projection_dir}/projection.json\" fenced-pre-reopen",
        '"${cma_fenced_projection_sha256}"',
      ],
    ),
    "Every Gateway projection transition must bind an externally retained predecessor digest",
  )
  check(
    contains_in_order?(
      fence_projection_validator,
      [
        'local cma_expected_stage="$2"',
        'local cma_expected_predecessor_digest="$3"',
        '[ "$(wc -l < "${cma_projection_digest_file}" | tr -d \' \')" = 1 ]',
        'cma_step10_sha256_valid "${cma_expected_predecessor_digest}"',
        'case "${cma_expected_stage}" in',
        'jq -e --arg expected_stage "${cma_expected_stage}"',
        '--arg expected_predecessor "${cma_expected_predecessor_digest}"',
        '.lifecycle == {',
        'stage:$expected_stage,predecessorSha256:$expected_predecessor',
      ],
    ),
    "Fenced projection validation must bind exact externally expected lifecycle ancestry",
  )

  check(
    receipt_writer_body.scan("CMA_GATEWAY_FENCE_RECEIPT_SHA256=").length == 1 &&
      contains_in_order?(
        receipt_writer_body,
        [
          'if [ -d "${cma_bundle_dir}" ]',
          'cma_step10_sha256_valid "${CMA_GATEWAY_FENCE_RECEIPT_SHA256:-}"',
          "an existing fence receipt requires its independently recorded SHA-256",
          "cma_step10_load_fence_receipt",
          '"${cma_receipt_file}" "${CMA_GATEWAY_FENCE_RECEIPT_SHA256}"',
          "CMA fence receipt candidate before publication:",
          '[ "${cma_published_receipt_digest}" = "${cma_receipt_digest}" ]',
          'CMA_GATEWAY_FENCE_RECEIPT_SHA256="${cma_receipt_digest}"',
        ],
      ) && release_prose.include?(
        "That candidate line is not a success acknowledgement",
      ) && release_prose.include?(
        "never replace the externally held digest with `receipt.sha256` from the same local bundle",
      ),
    "Fence receipt retry must retain the caller's external digest",
  )

  projection_workload_body = shell_function_body(
    restore_source,
    "cma_step10_projection_matches_final_workload",
    "sealed projection must retain its exact workload validator",
  )
  source_projection_validator = shell_function_body(
    restore_source,
    "cma_step10_validate_source_projection",
    "final source projection must retain its sealed workload validator",
  )
  fence_projection_digest_validator = shell_function_body(
    restore_source,
    "cma_step10_validate_fence_projection",
    "fenced projection must retain its digest-bound validator",
  )
  restore_main_body = shell_function_body(
    restore_source,
    "cma_step10_main",
    "traffic restoration must retain its closed entrypoint",
  )
  projection_binding_error =
    "Projection validators must return the digest of the exact bytes accepted by semantic and live validation"
  check(
    contains_in_order?(
      fence_projection_digest_validator,
      [
        'cma_initial_digest="$(cma_step10_projection_digest',
        "cma_step10_validate_fence_projection_receipt",
        'cma_semantic_digest="$(cma_step10_projection_digest',
        '[ "${cma_semantic_digest}" = "${cma_initial_digest}" ]',
        "cma_step10_projection_check_live",
        'cma_final_digest="$(cma_step10_projection_digest',
        '[ "${cma_final_digest}" = "${cma_initial_digest}" ]',
        'printf \'%s\\n\' "${cma_initial_digest}"',
      ],
    ) && contains_in_order?(
      source_projection_validator,
      [
        'cma_live_digest="$(cma_step10_sha256_file "${cma_projection_file}")"',
        '[ "${cma_recorded_digest}" = "${cma_live_digest}" ]',
        'cma_step10_projection_matches_final_workload "${cma_projection_file}"',
        'cma_semantic_digest="$(cma_step10_projection_digest',
        '[ "${cma_semantic_digest}" = "${cma_live_digest}" ]',
        'cma_step10_projection_check_live "${cma_projection_file}"',
        'cma_final_digest="$(cma_step10_projection_digest',
        '[ "${cma_final_digest}" = "${cma_live_digest}" ]',
        'printf \'%s\\n\' "${cma_live_digest}"',
      ],
    ) && restore_source.scan(
      'cma_fenced_projection_sha256="$(cma_step10_validate_fence_projection',
    ).length == 3 && restore_source.scan(
      'cma_final_projection_sha256="$(cma_step10_validate_source_projection',
    ).length == 1 && [prove_fence_body, fence_body, restore_main_body].none? do |body|
      body.include?("cma_step10_projection_digest")
    end && release_prose.include?(
      "Each fenced and final-source validator returns the SHA-256 of the exact projection bytes that passed its lifecycle-ancestry, workload, and live-routing checks; every caller validates and acknowledges that returned digest without reopening the mutable projection path.",
    ),
    projection_binding_error,
  )
  capture_workload_contracts = [
    '"${lifecycle_stage}" == fenced-pre-reopen',
    '"${lifecycle_stage}" == final-reopen',
    'get replicaset', '"${CMA_FINAL_REPLICA_SET}" -o json',
    'get pod', '"${CMA_FINAL_POD}" -o json',
    "apiDeploymentWorkload:$api_deployment_workload[0]",
    "apiReplicaSetWorkload:$api_replicaset_workload[0]",
    "apiPodWorkload:$api_pod_workload[0]",
    'printf \'%s\\n\' null > "${state_dir}/api-replicaset-workload.json"',
    'printf \'%s\\n\' null > "${state_dir}/api-pod-workload.json"',
  ]
  derived_workload_contracts = [
    'if %w[fenced-pre-reopen final-reopen].include?(lifecycle_stage)',
    'full_deployment.dig("metadata", "uid") == metadata["uid"]',
    'replica_metadata["ownerReferences"] == [{',
    'pod_metadata["ownerReferences"] == [{',
    'container_statuses.fetch(0)["restartCount"] == 0',
    '"specSha256" => Digest::SHA256.hexdigest(canonical_json(full_deployment["spec"]))',
    '"specSha256" => Digest::SHA256.hexdigest(canonical_json(replica_set["spec"]))',
    '"specSha256" => Digest::SHA256.hexdigest(canonical_json(retained_pod["spec"]))',
    '"verified" => verified',
    'assert!(full_deployment.nil? && replica_set.nil? && retained_pod.nil?',
  ]
  sealed_workload_contracts = [
    ".fullCapture.kubernetes.apiDeploymentWorkload",
    ".fullCapture.kubernetes.apiReplicaSetWorkload",
    ".fullCapture.kubernetes.apiPodWorkload",
    ".graph.apiWorkload.verified",
    '"${cma_final_deployment_uid}"', '"${cma_final_replica_set}"',
    '"${cma_final_replica_set_uid}"', '"${cma_final_pod}"',
    '"${cma_final_pod_uid}"', '"${cma_final_pod_ip}"',
    '"${cma_final_pod_image}"', '"${cma_final_pod_image_id}"',
    "cma_step10_validate_workload_contract",
  ]
  check(
    capture_workload_contracts.all? { |contract| files[:capture].include?(contract) } &&
      derived_workload_contracts.all? { |contract| files[:projection].include?(contract) } &&
      sealed_workload_contracts.all? { |contract| projection_workload_body.include?(contract) } &&
      fence_projection_validator.include?(
        'cma_step10_projection_matches_final_workload "${cma_projection_file}"',
      ) && source_projection_validator.include?(
        'cma_step10_projection_matches_final_workload "${cma_projection_file}"',
      ) && contains_in_order?(
        restore_source,
        [
          "fenced-pre-reopen \"${cma_post_scale_projection}\" fenced-post-scale",
          "cma_step10_validate_fence_projection",
          "cma_route_and_policies_ready",
          "if ! cma_final_workload_is_exact ||",
          "cma_step10_kubectl patch service cma-api --type=json",
          "final-reopen \"${cma_fenced_projection_dir}/projection.json\" fenced-pre-reopen",
          'cma_final_projection_sha256="$(cma_step10_validate_source_projection',
          "if ! cma_final_workload_is_exact ||",
        ],
      ) && release_prose.include?(
        "Both the `fenced-pre-reopen` and `final-reopen` projections capture the full normalized Deployment, retained ReplicaSet, and retained Pod objects",
      ) && release_prose.include?(
        "An exact live workload recheck immediately before the Service reopen and another immediately after final projection validation bracket both capture windows",
      ),
    "Reopen projections must seal and recheck the exact retained workload chain",
  )

  admission_publisher_body = shell_function_body(
    files[:admission_bootstrap],
    "cma_admission_publish_directory",
    "admission bootstrap must retain its atomic publisher",
  )
  check(
    admission_publisher_body.include?("File.rename(source, destination)") &&
      !admission_publisher_body.include?("fsync") &&
      files[:admission_bootstrap].scan("cma_admission_sync_published_directory").length == 3 &&
      files[:admission_bootstrap].include?(
        "existing admission receipt could not be durably re-synchronized",
      ),
    "Admission receipt publication must re-prove durability after atomic rename",
  )

  doc_contracts = [
    "trusted `env -i` launcher", "reviewed host toolchain",
    "non-ignored untracked", "assume-unchanged", "skip-worktree",
    "blocked-pending-reviewed-live-capture", "adopted projection",
    "durable pre-patch fence receipt", "ValidatingAdmissionPolicy",
    "two consecutive empty", "pre-reopen fenced projection", "final source projection",
    "Cloudflare", "Pseudo IPv4", "15 IPv4", "7 IPv6", "`curl --resolve`",
    "compute.backendServices.getHealth", "compute.firewalls.get",
    "production mutation remains hard-disabled",
  ]
  check(
    doc_contracts.all? { |contract| release_prose.include?(contract) },
    "Release documentation must describe the closed projection, custody, admission, and origin evidence gates",
  )

  check(
    bash_syntax_valid?(release_step10_invocation(release_doc)),
    "Release traffic-restoration step must remain valid Bash",
  )

  check(state_source.include?("cma_gateway_validate_backend_policy") &&
        source_contract_source.include?("--seal-bundle"),
        "existing Gateway desired/effective state contracts must remain wired")

  verify_gateway_release_contract_legacy_unused(
    ci_workflow,
    release_doc,
    state_source,
    adoption_source,
    source_contract_source,
    restore_source,
  )
end

def run_gateway_release_self_tests(
  ci_workflow,
  release_doc,
  state_source,
  adoption_source,
  source_contract_source,
  restore_source
)
  tests = [
    [
      "Gateway source self-test removed from CI",
      "CI must syntax-check and self-test every Gateway release contract",
      lambda do |ci, _release, _state, _adoption, _source_contract|
        run = mutation_step(
          ci,
          "trigger-release-contract",
          "Runtime provenance and release evidence stay mandatory",
        ).fetch("run")
        replace_once!(
          run,
          "ruby .github/scripts/cma-trigger-gateway-source-contract.rb --self-test",
          "true",
        )
      end,
    ],
    [
      "Gateway adoption behavioral tests removed from CI",
      "CI must syntax-check and self-test every Gateway release contract",
      lambda do |ci, _release, _state, _adoption, _source_contract|
        run = mutation_step(
          ci,
          "trigger-release-contract",
          "Runtime provenance and release evidence stay mandatory",
        ).fetch("run")
        replace_once!(run, ".github/scripts/cma-trigger-gateway-adoption-tests.sh", "true")
      end,
    ],
    [
      "Cloudflare behavioral tests removed from CI",
      "CI must syntax-check, shellcheck, and behavior-test all Gateway release tooling",
      lambda do |ci, _release, _state, _adoption, _source_contract|
        run = mutation_step(
          ci,
          "trigger-release-contract",
          "Runtime provenance and release evidence stay mandatory",
        ).fetch("run")
        replace_once!(
          run,
          "bash .github/scripts/cma-trigger-gateway-cloudflare-capture-tests.sh",
          "true # Cloudflare behavior tests removed",
        )
      end,
    ],
    [
      "Cloudflare capture removed from the adoption source seal",
      "admission, adoption, and traffic restoration must byte-seal every behavior-bearing release source",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          ".github/scripts/cma-trigger-gateway-cloudflare-capture.sh ",
          "",
        )
      end,
    ],
    [
      "annotated release-tag type check removed from adoption",
      "every mutating Gateway entrypoint must derive confirmation from one annotated exact SemVer release tag",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          '"${tag_object_type}" == tag',
          '"${tag_object_type}" != ""',
        )
      end,
    ],
    [
      "reviewed API Deployment removed from traffic source seal",
      "admission, adoption, and traffic restoration must byte-seal every behavior-bearing release source",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        restore_marker = "\n<!-- gateway-restore-self-test-source -->\n"
        marker_index = release.index(restore_marker)
        raise "self-test fixture is missing the restore-source marker" unless marker_index

        replace_once_after!(
          release,
          restore_marker,
          "infra/api/deployment.yaml ",
          "",
        )
      end,
    ],
    [
      "direct Gateway adoption reintroduced in release runbook",
      "Release runbook must invoke Gateway mutations only through the trusted launcher",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          ".github/scripts/cma-trigger-gateway-launch.sh adoption --adopt",
          ".github/scripts/cma-trigger-gateway-adoption.sh --adopt",
        )
      end,
    ],
    [
      "step-10 durable fence receipt digest omitted",
      "Release traffic restoration must carry durable admission, fence-root, and projection custody evidence",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "CMA_GATEWAY_FENCE_RECEIPT_SHA256='<externally-recorded-fence-receipt-SHA-256>'",
          "CMA_GATEWAY_FENCE_RECEIPT_DIGEST_OMITTED=true",
        )
      end,
    ],
    [
      "post-scale fence proof removed from release runbook",
      "Release runbook must retain the independently authenticated post-scale fence proof",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          ".github/scripts/cma-trigger-gateway-launch.sh traffic --prove-fence",
          "true # post-scale fence projection not captured",
        )
      end,
    ],
    [
      "full rollback projection comparison removed",
      "Gateway rollback success must compare a fresh full closed projection with sealed source behavior",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        comparison = [
          "  cma_gateway_compare_projection_behavior \\",
          '    "${source_projection}" "${rollback_projection_dir}/projection.json"',
        ].join("\n")
        replace_once!(
          adoption,
          comparison,
          "  true # full rollback behavior not compared",
        )
      end,
    ],
    [
      "final immutable imageID binding weakened",
      "Release traffic reopen must bind the exact EndpointSlice and zonal NEG population",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          ".status.containerStatuses[0].imageID == $image_id",
          '(.status.containerStatuses[0].imageID | type) == "string"',
        )
      end,
    ],
    [
      "Step-10 emitted shell lint removed from CI",
      "CI must syntax-check and self-test every Gateway release contract",
      lambda do |ci, _release, _state, _adoption, _source_contract|
        run = mutation_step(
          ci,
          "trigger-release-contract",
          "Runtime provenance and release evidence stay mandatory",
        ).fetch("run")
        replace_once!(
          run,
          "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10 | shellcheck -s bash -",
          "true # Step 10 shell lint removed",
        )
      end,
    ],
    [
      "documented Step-10 wrapper lint removed from CI",
      "CI must syntax-check and self-test every Gateway release contract",
      lambda do |ci, _release, _state, _adoption, _source_contract|
        run = mutation_step(
          ci,
          "trigger-release-contract",
          "Runtime provenance and release evidence stay mandatory",
        ).fetch("run")
        replace_once!(
          run,
          "ruby .github/scripts/cma-trigger-publish-contract.rb --emit-step10-invocation | shellcheck -s bash -",
          "true # documented Step 10 wrapper lint removed",
        )
      end,
    ],
    [
      "documented Step-10 wrapper syntax corrupted",
      "Release traffic-restoration step must remain valid Bash",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "cma_restore_status=0",
          "cma_restore_status=0\nif (",
        )
      end,
    ],
    [
      "full checked-in Gateway shell lint removed from CI",
      "CI must run full ShellCheck over every checked-in Gateway shell program",
      lambda do |ci, _release, _state, _adoption, _source_contract|
        run = mutation_step(
          ci,
          "trigger-release-contract",
          "Runtime provenance and release evidence stay mandatory",
        ).fetch("run")
        replace_once!(
          run,
          "shellcheck -x .github/scripts/cma-trigger-gateway-*.sh",
          "true # full Gateway shell lint removed",
        )
      end,
    ],
    [
      "Gateway source object equality weakened",
      "Gateway source contract must validate and emit only the reviewed object bundle",
      lambda do |_ci, _release, _state, _adoption, source_contract|
        replace_once!(source_contract, "routes == expected_routes", "routes.length == 3")
      end,
    ],
    [
      "durable rollback bundle fsync removed",
      "Gateway source contract must durably seal exactly four custody files",
      lambda do |_ci, _release, _state, _adoption, source_contract|
        replace_once!(source_contract, "file.fsync", "file.flush")
      end,
    ],
    [
      "direct route validation removed",
      "Gateway state checker must bind every route and effective GCP policy",
      lambda do |_ci, _release, state, _adoption, _source_contract|
        replace_once!(
          state,
          'cma_gateway_validate_api_route "${state_dir}/cma-api-route.json" || return 1',
          "true # direct route validation removed",
        )
      end,
    ],
    [
      "route generation fence weakened",
      "Gateway state checker must bind every route and effective GCP policy",
      lambda do |_ci, _release, state, _adoption, _source_contract|
        replace_once!(state, ".observedGeneration == $generation", ".observedGeneration != null")
      end,
    ],
    [
      "owned metadata fence removed",
      "Gateway state checker must bind every route and effective GCP policy",
      lambda do |_ci, _release, state, _adoption, _source_contract|
        replace_once!(state, '(.metadata.ownerReferences // []) == []', "true")
      end,
    ],
    [
      "state Kubernetes timeout removed",
      "Gateway state checker must bound every Kubernetes request",
      lambda do |_ci, _release, state, _adoption, _source_contract|
        marker = "cma_gateway_kubectl() {"
        start = state.index(marker)
        raise "self-test fixture is missing the state kubectl wrapper" unless start

        timeout = "timeout --signal=TERM --kill-after=5s 25s"
        index = state.index(timeout, start)
        raise "self-test fixture is missing the state kubectl timeout" unless index

        state[index, timeout.length] = "true # timeout removed"
      end,
    ],
    [
      "state Compute environment isolation removed",
      "Gateway state checker must isolate every Compute truth read from ambient overrides",
      lambda do |_ci, _release, state, _adoption, _source_contract|
        replace_once_after!(
          state,
          "cma_gateway_gcloud() {",
          "env -i",
          "env",
        )
      end,
    ],
    [
      "effective health path weakened",
      "Gateway state checker must bind every route and effective GCP policy",
      lambda do |_ci, _release, state, _adoption, _source_contract|
        replace_once!(
          state,
          ".httpHealthCheck.requestPath == $expected_path",
          ".httpHealthCheck.requestPath != null",
        )
      end,
    ],
    [
      "effective NEG binding weakened",
      "Gateway state checker must bind every route and effective GCP policy",
      lambda do |_ci, _release, state, _adoption, _source_contract|
        replace_once!(
          state,
          "([.backends[]?.group] | sort) == ($groups | sort)",
          "([.backends[]?.group] | length) > 0",
        )
      end,
    ],
    [
      "rollback health transition set widened",
      "Gateway state checker must bind transition, rollback, and adopted state to the sealed snapshot",
      lambda do |_ci, _release, state, _adoption, _source_contract|
        replace_once!(
          state,
          '"${effective_path}" == /livez || "${effective_path}" == /ready',
          '-n "${effective_path}"',
        )
      end,
    ],
    [
      "reviewed release revision weakened",
      "Gateway adoption must seal and preflight source state before ordered CAS writes",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          '[[ "${CMA_GATEWAY_RELEASE_REVISION:-}" =~ ^[0-9a-f]{40}$ ]]',
          '[[ -n "${CMA_GATEWAY_RELEASE_REVISION:-}" ]]',
        )
      end,
    ],
    [
      "reviewed source aggregate comparison removed",
      "Gateway adoption must seal and preflight source state before ordered CAS writes",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          '[[ "${live_source_sha256}" == "${reviewed_source_sha256}" ]]',
          "true",
        )
      end,
    ],
    [
      "exclusive spec-writer attestation removed",
      "Gateway adoption must seal and preflight source state before ordered CAS writes",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          '[[ "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:-}" == sole-spec-writer ]]',
          "true",
        )
      end,
    ],
    [
      "server-side replacement preflight removed",
      "Gateway adoption must seal and preflight source state before ordered CAS writes",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(adoption, "--dry-run=server", "--dry-run=client")
      end,
    ],
    [
      "adoption rollback replacement preflight removed",
      "Gateway adoption and manual restore must preflight every forward and rollback replacement",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        marker = "cma_gateway_preflight_replacements"
        adopt_index = adoption.index("cma_gateway_adopt() {")
        raise "self-test fixture is missing Gateway adoption" unless adopt_index

        desired_index = adoption.index(marker, adopt_index)
        rollback_index = desired_index && adoption.index(marker, desired_index + marker.length)
        raise "self-test fixture is missing the adoption rollback preflight" unless rollback_index

        adoption[rollback_index, marker.length] = "true # rollback preflight removed"
      end,
    ],
    [
      "standalone restore replacement preflight disabled",
      "Gateway adoption and manual restore must preflight every forward and rollback replacement",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          'cma_gateway_restore "${snapshot_dir}" 1',
          'cma_gateway_restore "${snapshot_dir}" 0',
        )
      end,
    ],
    [
      "health-effective forward barrier removed",
      "Gateway adoption must seal and preflight source state before ordered CAS writes",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          'cma_gateway_wait_for_state ready-transition "${CMA_GATEWAY_ROLLBACK_DIR}"',
          "true # health transition not proved",
        )
      end,
    ],
    [
      "write-response validation removed",
      "Gateway adoption must validate write responses and re-read before every forward CAS",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          "cma_gateway_validate_response() {",
          "cma_gateway_accept_response() {",
        )
      end,
    ],
    [
      "durable rollback custody attestation removed",
      "Gateway adoption must capture rollback material in attested durable custody",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          '[[ "${CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM:-}" == durable-off-host ]]',
          "true",
        )
      end,
    ],
    [
      "rollback custody root ownership check removed",
      "Gateway adoption must capture rollback material in attested durable custody",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(adoption, '-O "${custody_root}"', "true")
      end,
    ],
    [
      "rollback temporary-directory failure ignored",
      "Gateway rollback must preserve helper failures while running under set +e",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          'restore_dir="$(mktemp -d)" || return 1',
          'restore_dir="$(mktemp -d)"',
        )
      end,
    ],
    [
      "CAS replace changed to apply",
      "Gateway rollback must mask signals and restore every sealed object through CAS",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          "cma_gateway_kubectl replace -f",
          "cma_gateway_kubectl apply -f",
        )
      end,
    ],
    [
      "rollback signal mask removed",
      "Gateway rollback must mask signals and restore every sealed object through CAS",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(adoption, "trap '' HUP INT QUIT TERM", "trap - HUP INT QUIT TERM")
      end,
    ],
    [
      "rollback signal mask delayed until after state initialization",
      "Gateway rollback must mask signals and restore every sealed object through CAS",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        fragment = <<~'SHELL'.strip
          trap '' HUP INT QUIT TERM
            local original_status="$1"
            local rollback_status=0
        SHELL
        replacement = <<~'SHELL'.strip
          local original_status="$1"
            trap '' HUP INT QUIT TERM
            local rollback_status=0
        SHELL
        replace_once!(adoption, fragment, replacement)
      end,
    ],
    [
      "adoption success signal mask removed",
      "Gateway adoption success must remain signal-masked through disarm, cleanup, and output",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        adopt_index = adoption.index("cma_gateway_adopt() {")
        raise "self-test fixture is missing Gateway adoption" unless adopt_index

        mask_index = adoption.index("trap '' HUP INT QUIT TERM", adopt_index)
        raise "self-test fixture is missing the adoption success signal mask" unless mask_index

        adoption[mask_index, "trap '' HUP INT QUIT TERM".length] = "trap - HUP INT QUIT TERM"
      end,
    ],
    [
      "rollback route and source-probe gate removed",
      "Gateway rollback must mask signals and restore every sealed object through CAS",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          "cma_gateway_wait_for_state rollback-routes",
          "true # rollback routes not proved",
        )
      end,
    ],
    [
      "manual restore confirmation removed",
      "Gateway adoption and sealed manual restore must require separate confirmations",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          "cma_gateway_require_confirmation restore route",
          "true",
        )
      end,
    ],
    [
      "manual restore seal removed",
      "Gateway adoption and sealed manual restore must require separate confirmations",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          '[[ "${CMA_GATEWAY_ROLLBACK_SHA256:-}" =~ ^[0-9a-f]{64}$ ]]',
          "true",
        )
      end,
    ],
    [
      "external rollback seal omitted",
      "Release documentation must state the sealed CAS adoption and recovery contract",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "CMA_GATEWAY_ROLLBACK_SHA256='<externally-recorded-rollback-SHA-256>'",
          "CMA_GATEWAY_ROLLBACK_SHA256=unknown",
        )
      end,
    ],
    [
      "durable rollback custody omitted from release command",
      "Release documentation must state the sealed CAS adoption and recovery contract",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host",
          "CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=unknown",
        )
      end,
    ],
    [
      "exclusive spec-writer release attestation omitted",
      "Release adoption and restore must require the exclusive spec-writer attestation",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer",
          "CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=unknown",
        )
      end,
    ],
    [
      "release routing-graph freeze narrowed",
      "Release documentation must state the sealed CAS adoption and recovery contract",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "entire checked routing graph",
          "four managed objects",
        )
      end,
    ],
    [
      "shared-Gateway attachable-route freeze narrowed",
      "Release documentation must state the sealed CAS adoption and recovery contract",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "every `HTTPRoute` in any namespace allowed to attach to either the HTTP or\nHTTPS listener",
          "the three owned `HTTPRoute` objects",
        )
      end,
    ],
    [
      "effective-GCP production pre-mutation gate removed",
      "Release documentation must keep effective routing evidence as a production pre-mutation gate",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "production adoption must remain disabled",
          "production adoption may proceed",
        )
      end,
    ],
    [
      "step-10 reviewed checker byte binding removed",
      "Release traffic restoration must execute only from the clean reviewed commit",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          '"${cma_step10_live_checker_sha256}" !=',
          '"${cma_step10_live_checker_sha256}" =',
        )
      end,
    ],
    [
      "step-10 checker symlink rejection removed",
      "Release traffic restoration must execute only from the clean reviewed commit",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          '[ -L "${cma_step10_checker_path}" ]',
          '[ -L /dev/null ]',
        )
      end,
    ],
    [
      "step-10 sourced adoption-helper byte binding removed",
      "Release traffic restoration must byte-bind every sourced Gateway helper",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          '"${cma_step10_live_adoption_sha256}" !=',
          '"${cma_step10_live_adoption_sha256}" =',
        )
      end,
    ],
    [
      "step-10 routing-graph freeze check removed",
      "Release traffic restoration must require and retain the checked-routing-graph freeze",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          'if [ "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM}" != sole-spec-writer ]; then',
          "if false; then",
        )
      end,
    ],
    [
      "dedicated fence production gate removed",
      "Dedicated traffic fencing must remain hard-disabled before every boundary",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_fence_main() {",
          "cma_step10_require_production_mutation_enabled || return 1",
          "true # production fence gate removed",
        )
      end,
    ],
    [
      "dedicated fence CAS test removed",
      "Dedicated traffic fencing must remain an idempotent source-to-fence CAS",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_fence_main() {",
          '"op":"test","path":"/spec/selector"',
          '"op":"add","path":"/spec/selector"',
        )
      end,
    ],
    [
      "dedicated fence already-fenced branch removed",
      "Dedicated traffic fencing must remain an idempotent source-to-fence CAS",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_fence_main() {",
          'elif .spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"} then "fenced"',
          'elif false then "fenced"',
        )
      end,
    ],
    [
      "dedicated fence source-only patch guard removed",
      "Dedicated traffic fencing must remain an idempotent source-to-fence CAS",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_fence_main() {",
          'if [ "${cma_fence_initial_state}" = source ]; then',
          "if true; then",
        )
      end,
    ],
    [
      "dedicated fence ambiguous-write re-read removed",
      "Dedicated traffic fencing must reconcile ambiguous applied writes before success",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_fence_main() {",
          'cma_fence_observed_state="$(cma_read_selector_state)"',
          "cma_fence_observed_state=fenced",
        )
      end,
    ],
    [
      "dedicated fence initial Service-NEG root pin removed",
      "Dedicated traffic fencing must pin Service/NEG identity and prove no-Pod external drain",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_fence_main() {",
          'cma_pin_or_check_api_root "${cma_fence_initial_record}"',
          "true # initial Service-NEG root pin removed",
        )
      end,
    ],
    [
      "dedicated fence reserved-label Pod boundary removed",
      "Dedicated traffic fencing must pin Service/NEG identity and prove no-Pod external drain",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_fence_main() {",
          "if ! cma_fence_label_has_no_pods; then",
          "if ! true; then",
        )
      end,
    ],
    [
      "dedicated fence external drain boundary removed",
      "Dedicated traffic fencing must reconcile ambiguous applied writes before success",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_fence_main() {",
          "if ! cma_wait_for_fenced_endpoints; then",
          "if ! true; then",
        )
      end,
    ],
    [
      "step-10 snapshot seal recheck removed",
      "Release traffic restoration must authenticate and bind the retained adoption snapshot",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "cma_step10_validate_snapshot_seal || return 1",
          "true # snapshot seal not rechecked",
        )
      end,
    ],
    [
      "step-10 bounded kubectl wrapper removed",
      "Release traffic restoration must bound every cluster and Gateway check",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(release, "cma_step10_kubectl() {", "cma_step10_raw_kubectl() {")
      end,
    ],
    [
      "step-10 Compute environment isolation removed",
      "Release traffic re-fence must prove coherent empty zonal NEGs",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_gcloud() {",
          "env -i",
          "env",
        )
      end,
    ],
    [
      "step-10 zonal NEG membership read removed",
      "Dedicated traffic fencing must pin Service/NEG identity and prove no-Pod external drain",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "compute network-endpoint-groups list-network-endpoints",
          "compute network-endpoint-groups describe",
        )
      end,
    ],
    [
      "step-10 Service top-level root opened",
      "Release traffic gate must close the Service and EndpointSlice roots",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          '(keys | sort) == ["apiVersion","kind","metadata","spec","status"]',
          '(keys | index("apiVersion")) != null',
        )
      end,
    ],
    [
      "step-10 Service spec root opened",
      "Release traffic gate must close the Service and EndpointSlice roots",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          '"clusterIP","clusterIPs","internalTrafficPolicy","ipFamilies",',
          '"clusterIP","clusterIPs","ipFamilies",',
        )
      end,
    ],
    [
      "step-10 desired NEG intent removed",
      "Release traffic gate must close the Service and EndpointSlice roots",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          '(.metadata.annotations["cloud.google.com/neg"] | fromjson) ==',
          '(.metadata.annotations["cloud.google.com/neg"] | fromjson) !=',
        )
      end,
    ],
    [
      "step-10 EndpointSlice list envelope opened",
      "Release traffic gate must close the Service and EndpointSlice roots",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          '(keys | sort) == ["apiVersion","items","kind","metadata"]',
          '(.items | type) == "array"',
        )
      end,
    ],
    [
      "step-10 EndpointSlice owner-Service UID binding removed",
      "Release traffic gate must close the Service and EndpointSlice roots",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(release, "uid:$service_uid", 'uid:(.uid // "")')
      end,
    ],
    [
      "step-10 consecutive empty NEG observation removed",
      "Release traffic re-fence must prove coherent empty zonal NEGs",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          'cma_fence_previous_empty_observation=""',
          'cma_fence_previous_empty_observation="already-empty"',
        )
      end,
    ],
    [
      "step-10 five-minute fence deadline removed",
      "Release traffic re-fence must prove coherent empty zonal NEGs",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          'cma_fence_deadline=$((SECONDS + 300))',
          'cma_fence_endpoint_attempt=0',
        )
      end,
    ],
    [
      "step-10 exact reopened EndpointSlice address weakened",
      "Release traffic reopen must bind the exact EndpointSlice and zonal NEG population",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(release, "addresses:[$ip]", "addresses:(.addresses | sort)")
      end,
    ],
    [
      "step-10 exact aggregate NEG membership weakened",
      "Release traffic reopen must bind the exact EndpointSlice and zonal NEG population",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          'length == 1 and .[0].ipAddress == $ip and .[0].port == 8080',
          'length > 0 and any(.[]; .ipAddress == $ip)',
        )
      end,
    ],
    [
      "step-10 second post-probe reopened proof removed",
      "Release traffic reopen must bind the exact EndpointSlice and zonal NEG population",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        contract = "if ! cma_wait_for_reopened_endpoint; then"
        index = release.rindex(contract)
        raise "self-test fixture is missing the second reopened proof" unless index

        release[index, contract.length] = "if ! true; then"
      end,
    ],
    [
      "step-10 Service identity recheck removed from NEG proof",
      "Dedicated traffic fencing must pin Service/NEG identity and prove no-Pod external drain",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          '"${cma_fence_neg_record_after}" != "${cma_fence_neg_record_before}"',
          '"${cma_fence_neg_record_after}" == "${cma_fence_neg_record_before}"',
        )
      end,
    ],
    [
      "step-10 exit signal mask delayed",
      "Release traffic restoration must mask signals during re-fencing and success disarm",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        fragment = <<~'SHELL'.strip
          cma_refence_on_exit() {
            trap '' HUP INT QUIT TERM
            exec 9<&-
            cma_exit_status="$1"
            trap - EXIT
        SHELL
        replacement = fragment.sub(
          "trap '' HUP INT QUIT TERM\n  exec 9<&-\n  cma_exit_status=\"$1\"",
          "cma_exit_status=\"$1\"\n  trap '' HUP INT QUIT TERM\n  exec 9<&-",
        )
        replace_once!(release, fragment, replacement)
      end,
    ],
    [
      "step-10 success signal mask removed",
      "Release traffic restoration must mask signals during re-fencing and success disarm",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        success_marker =
          "unset cma_release_access_token cma_direct_livez cma_direct_ready cma_app_openapi"
        marker_index = release.index(success_marker)
        raise "self-test fixture is missing the step-10 success marker" unless marker_index

        mask_index = release.rindex("trap '' HUP INT QUIT TERM", marker_index)
        raise "self-test fixture is missing the step-10 success signal mask" unless mask_index

        release[mask_index, "trap '' HUP INT QUIT TERM".length] = "trap - HUP INT QUIT TERM"
      end,
    ],
    [
      "CMA OpenAPI identity weakened",
      "Release traffic restoration must identify the CMA OpenAPI document",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          '(.paths["/api/v1/sessions"].get | type) == "object"',
          '(.paths | type) == "object"',
        )
      end,
    ],
    [
      "runbook tag derivation accepts ambiguous or lightweight tags",
      "Release runbook must derive every confirmation from one annotated exact SemVer tag",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          'cma_gateway_release_tag_record="$(git --no-replace-objects for-each-ref',
          'cma_gateway_release_tag_record="$(git describe --tags --exact-match',
        )
      end,
    ],
    [
      "manual restore admission preflight removed",
      "Gateway manual restore must authenticate admission custody before every write",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once_after!(
          adoption,
          "cma_gateway_manual_restore() {",
          "cma_gateway_verify_admission_receipt",
          "true # manual admission preflight removed",
        )
      end,
    ],
    [
      "manual restore admission custody removed from runbook",
      "Gateway manual restore must authenticate admission custody before every write",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        marker = "Manual restore has its own explicit confirmation"
        replace_once_after!(
          release,
          marker,
          "CMA_GATEWAY_ADMISSION_RECEIPT='<absolute-admission-receipt>'",
          "CMA_GATEWAY_MANUAL_ADMISSION_RECEIPT_OMITTED='<omitted>'",
        )
        replace_once_after!(
          release,
          marker,
          "CMA_GATEWAY_ADMISSION_RECEIPT_SHA256='<externally-recorded-admission-receipt-SHA-256>'",
          "CMA_GATEWAY_MANUAL_ADMISSION_RECEIPT_SHA256_OMITTED='<omitted>'",
        )
      end,
    ],
    [
      "manual restore source-projection preflight removed",
      "Gateway manual restore must validate source projection custody before every write",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once_after!(
          adoption,
          "cma_gateway_manual_restore() {",
          "cma_gateway_validate_projection_receipt 0",
          "true # source projection preflight removed",
        )
      end,
    ],
    [
      "full workload manifest validator removed from reopen",
      "Release traffic workload proof must bind every live spec to the reviewed Deployment manifest",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "<!-- gateway-restore-self-test-source -->",
          "--validate-workload infra/api/deployment.yaml",
          "--workload-validation-removed",
        )
      end,
    ],
    [
      "post-scale zero-workload verifier removed",
      "Post-scale fence proof must bind predecessor Pod deletion and stable zero replicas before publication",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_prove_fence_main() {",
          "cma_step10_validate_scaled_down_workload",
          "true # scaled-down workload proof removed",
        )
      end,
    ],
    [
      "pre-mutation fence receipt acknowledgement removed",
      "Traffic fence must surface durable receipt custody before its Service mutation",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "CMA durable fence root committed before Service mutation",
          "CMA fence receipt withheld until final success",
        )
      end,
    ],
    [
      "post-rename fence receipt durability barrier removed",
      "Fence receipt and projection publication must prove durability after atomic rename",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "File.rename(source, destination)",
          "cma_step10_sync_published_fence_bundle",
          "true # post-rename durability barrier removed",
        )
      end,
    ],
    [
      "manual restore source lifecycle root weakened",
      "Gateway manual restore must validate source projection custody before every write",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          '.lifecycle == {stage:"source",predecessorSha256:"genesis"}',
          '.lifecycle.stage == "source"',
        )
      end,
    ],
    [
      "adopted projection digest acknowledgement removed",
      "Gateway adoption must externally acknowledge adopted projection custody",
      lambda do |_ci, _release, _state, adoption, _source_contract|
        replace_once!(
          adoption,
          'predecessor SHA-256 ${CMA_GATEWAY_PROJECTION_SHA256}',
          "predecessor digest withheld",
        )
      end,
    ],
    [
      "source projection external digest removed from fence",
      "Every Gateway projection transition must bind an externally retained predecessor digest",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "<!-- gateway-restore-self-test-source -->",
          'cma_source_projection_sha256="${CMA_GATEWAY_SOURCE_PROJECTION_SHA256:-}"',
          'cma_source_projection_sha256="$(cma_step10_projection_digest "${cma_source_projection}")"',
        )
      end,
    ],
    [
      "pre-scale projection external digest removed from prove-fence",
      "Every Gateway projection transition must bind an externally retained predecessor digest",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_prove_fence_main() {",
          'cma_predecessor_projection_sha256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256:-}"',
          'cma_predecessor_projection_sha256="$(cma_step10_projection_digest "${cma_predecessor_projection}")"',
        )
      end,
    ],
    [
      "pre-scale projection external digest removed from restore",
      "Every Gateway projection transition must bind an externally retained predecessor digest",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_main() {",
          'cma_pre_scale_projection_sha256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256:-}"',
          'cma_pre_scale_projection_sha256="$(cma_step10_projection_digest "${cma_post_scale_projection}")"',
        )
      end,
    ],
    [
      "post-scale projection external digest removed from restore",
      "Every Gateway projection transition must bind an externally retained predecessor digest",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_main() {",
          'cma_post_scale_projection_sha256="${CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256:-}"',
          'cma_post_scale_projection_sha256="$(cma_step10_projection_digest "${cma_post_scale_projection}")"',
        )
      end,
    ],
    [
      "fenced lifecycle predecessor comparison removed",
      "Fenced projection validation must bind exact externally expected lifecycle ancestry",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_validate_fence_projection_receipt() {",
          'stage:$expected_stage,predecessorSha256:$expected_predecessor',
          'stage:$expected_stage,predecessorSha256:.lifecycle.predecessorSha256',
        )
      end,
    ],
    [
      "existing fence receipt overwrites caller digest",
      "Fence receipt retry must retain the caller's external digest",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        before = 'cma_step10_sha256_valid "${CMA_GATEWAY_FENCE_RECEIPT_SHA256:-}" || {'
        replacement = <<~'SHELL'.strip
          CMA_GATEWAY_FENCE_RECEIPT_SHA256="${cma_published_receipt_digest}"
              cma_step10_sha256_valid "${CMA_GATEWAY_FENCE_RECEIPT_SHA256:-}" || {
        SHELL
        replace_once_after!(
          release,
          "cma_step10_write_fence_receipt() {",
          before,
          replacement,
        )
      end,
    ],
    [
      "sealed pre-reopen workload validator removed",
      "Reopen projections must seal and recheck the exact retained workload chain",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_validate_fence_projection_receipt() {",
          'cma_step10_projection_matches_final_workload "${cma_projection_file}"',
          "true # sealed workload validator removed",
        )
      end,
    ],
    [
      "pre-reopen exact live workload check removed",
      "Reopen projections must seal and recheck the exact retained workload chain",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          'fenced-pre-reopen "${cma_post_scale_projection}" fenced-post-scale',
          "if ! cma_final_workload_is_exact ||",
          "if ! true ||",
        )
      end,
    ],
    [
      "post-final-projection exact live workload check removed",
      "Reopen projections must seal and recheck the exact retained workload chain",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          'cma_final_projection_sha256="$(cma_step10_validate_source_projection',
          "if ! cma_final_workload_is_exact ||",
          "if ! true ||",
        )
      end,
    ],
    [
      "runbook pre-scale projection digest handoff removed",
      "Gateway runbook must carry every external projection digest between processes",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "10. Restore ordinary traffic only after every final check has passed.",
          'CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256}"',
          "CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256='<locally-recovered-digest>'",
        )
      end,
    ],
    [
      "runbook post-scale projection digest handoff removed",
      "Gateway runbook must carry every external projection digest between processes",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          'CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256="${CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256}"',
          "CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256='<locally-recovered-digest>'",
        )
      end,
    ],
    [
      "runbook sealed workload projection language removed",
      "Reopen projections must seal and recheck the exact retained workload chain",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once!(
          release,
          "Both the `fenced-pre-reopen` and `final-reopen` projections capture the full normalized Deployment, retained ReplicaSet, and retained Pod objects",
          "The reopen projections capture workload metadata",
        )
      end,
    ],
    [
      "fenced semantic digest binding removed",
      "Projection validators must return the digest of the exact bytes accepted by semantic and live validation",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_validate_fence_projection() {",
          '[ "${cma_semantic_digest}" = "${cma_initial_digest}" ]',
          "true # fenced semantic digest binding removed",
        )
      end,
    ],
    [
      "fenced live digest binding removed",
      "Projection validators must return the digest of the exact bytes accepted by semantic and live validation",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_validate_fence_projection() {",
          '[ "${cma_final_digest}" = "${cma_initial_digest}" ]',
          "true # fenced live digest binding removed",
        )
      end,
    ],
    [
      "source semantic digest binding removed",
      "Projection validators must return the digest of the exact bytes accepted by semantic and live validation",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_validate_source_projection() {",
          '[ "${cma_semantic_digest}" = "${cma_live_digest}" ]',
          "true # source semantic digest binding removed",
        )
      end,
    ],
    [
      "source live digest binding removed",
      "Projection validators must return the digest of the exact bytes accepted by semantic and live validation",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_validate_source_projection() {",
          '[ "${cma_final_digest}" = "${cma_live_digest}" ]',
          "true # source live digest binding removed",
        )
      end,
    ],
    [
      "caller-side unbound fenced digest restored",
      "Projection validators must return the digest of the exact bytes accepted by semantic and live validation",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        replace_once_after!(
          release,
          "cma_step10_prove_fence_main() {",
          'cma_fenced_projection_sha256="$(cma_step10_validate_fence_projection',
          'cma_fenced_projection_sha256="$(cma_step10_projection_digest',
        )
      end,
    ],
    [
      "final source validator return ignored",
      "Projection validators must return the digest of the exact bytes accepted by semantic and live validation",
      lambda do |_ci, release, _state, _adoption, _source_contract|
        before = <<~'SHELL'.strip
          cma_final_projection_sha256="$(cma_step10_validate_source_projection \
            "${cma_final_projection_dir}")" || {
            cma_abort_after_reopen "the final live graph differs from the durable pre-fence backend root" || :
            exit 1
          }
        SHELL
        replacement = <<~'SHELL'.strip
          if ! cma_step10_validate_source_projection \
              "${cma_final_projection_dir}" >/dev/null; then
            cma_abort_after_reopen "the final live graph differs from the durable pre-fence backend root" || :
            exit 1
          fi
          cma_final_projection_sha256=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
        SHELL
        replace_once_after!(
          release,
          "cma_step10_main() {",
          before,
          replacement,
        )
      end,
    ],
  ]

  tests.each do |name, expected_error, mutation|
    candidate_ci = cloned(ci_workflow)
    restore_separator = "\n<!-- gateway-restore-self-test-source -->\n"
    candidate_release_bundle = release_doc.dup + restore_separator + restore_source.dup
    candidate_state = state_source.dup
    candidate_adoption = adoption_source.dup
    candidate_source_contract = source_contract_source.dup
    mutation.call(
      candidate_ci,
      candidate_release_bundle,
      candidate_state,
      candidate_adoption,
      candidate_source_contract,
    )
    candidate_release, candidate_restore = candidate_release_bundle.split(
      restore_separator,
      2,
    )
    unless candidate_release && candidate_restore
      raise ContractError, "self-test #{name.inspect} corrupted its release fixture"
    end
    begin
      verify_gateway_release_contract(
        candidate_ci,
        candidate_release,
        candidate_state,
        candidate_adoption,
        candidate_source_contract,
        candidate_restore,
      )
    rescue ContractError => error
      unless error.message.include?(expected_error)
        raise ContractError,
              "self-test #{name.inspect} failed for the wrong reason: #{error.message}"
      end
      next
    end
    raise ContractError, "self-test #{name.inspect} was not rejected"
  end

  support_tests = [
    [
      "pre-scale projection digest removed from trusted launcher",
      "Gateway launcher must preserve every external projection digest",
      lambda do |sources|
        replace_once!(
          sources.fetch(:launcher),
          "CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256",
          "CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_DIGEST_REMOVED",
        )
      end,
    ],
    [
      "post-scale projection digest removed from trusted launcher",
      "Gateway launcher must preserve every external projection digest",
      lambda do |sources|
        replace_once!(
          sources.fetch(:launcher),
          "CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256",
          "CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_DIGEST_REMOVED",
        )
      end,
    ],
    [
      "retained Pod object removed from projection capture",
      "Reopen projections must seal and recheck the exact retained workload chain",
      lambda do |sources|
        replace_once!(
          sources.fetch(:capture),
          "apiPodWorkload:$api_pod_workload[0]",
          "apiPodWorkload:null",
        )
      end,
    ],
    [
      "reopen lifecycle derived workload proof removed",
      "Reopen projections must seal and recheck the exact retained workload chain",
      lambda do |sources|
        replace_once!(
          sources.fetch(:projection),
          'if %w[fenced-pre-reopen final-reopen].include?(lifecycle_stage)',
          'if lifecycle_stage == "final-reopen"',
        )
      end,
    ],
    [
      "TLS residual PEM rejection removed",
      "Gateway TLS sanitizer must reject residual PEM and publish only parsed public certificates",
      lambda do |sources|
        replace_once!(
          sources.fetch(:tls_sanitizer),
          'raise ArgumentError unless certificate_remainder.match?(/\A[[:space:]]*\z/)',
          "raise ArgumentError if false # residual PEM accepted",
        )
      end,
    ],
    [
      "TLS certificate reconstruction removed",
      "Gateway TLS sanitizer must reject residual PEM and publish only parsed public certificates",
      lambda do |sources|
        replace_once!(
          sources.fetch(:tls_sanitizer),
          "certificate_pem = certificates.map(&:to_pem).join",
          "certificate_pem = certificate_source",
        )
      end,
    ],
  ]
  support_sources = load_gateway_support_sources
  support_tests.each do |name, expected_error, mutation|
    candidate_support = support_sources.transform_values(&:dup)
    mutation.call(candidate_support)
    begin
      verify_gateway_release_contract(
        cloned(ci_workflow),
        release_doc.dup,
        state_source.dup,
        adoption_source.dup,
        source_contract_source.dup,
        restore_source.dup,
        candidate_support,
      )
    rescue ContractError => error
      unless error.message.include?(expected_error)
        raise ContractError,
              "self-test #{name.inspect} failed for the wrong reason: #{error.message}"
      end
      next
    end
    raise ContractError, "self-test #{name.inspect} was not rejected"
  end

  mutation_count = tests.length + support_tests.length
  puts "CMA Trigger Gateway release self-tests passed (#{mutation_count} mutations)"
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

def replace_once_after!(source, marker, before, after)
  marker_index = source.index(marker)
  raise "self-test fixture is missing #{marker.inspect}" unless marker_index

  index = source.index(before, marker_index + marker.length)
  raise "self-test fixture is missing #{before.inspect} after #{marker.inspect}" unless index

  source[index, before.length] = after
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

def run_self_tests(workflow, release_doc, readiness_doc, restore_source)
  tests = [
    [
      "package writer action floated",
      "every action with package-write authority must retain its exact reviewed SHA",
      lambda do |candidate, _release, _readiness|
        steps = candidate.fetch("jobs").fetch("images").fetch("steps")
        checkout = steps.find do |step|
          step.is_a?(Hash) && step["uses"] == PACKAGE_WRITER_ACTION_REFS.first
        end
        raise "self-test fixture is missing the pinned package-writer checkout" unless checkout

        checkout["uses"] = "actions/checkout@v5"
      end,
    ],
    [
      "second package writer added",
      "only the publish images job may receive package-write authority",
      lambda do |candidate, _release, _readiness|
        candidate.fetch("jobs").fetch("image-exports").fetch("permissions")["packages"] = "write"
      end,
    ],
    [
      "GHCR owner exception removed from release documentation",
      "Release documentation must retain the audited GHCR writer and owner-exception contract",
      lambda do |_candidate, release, _readiness|
        replace_once!(
          release,
          "Literal\n   zero-human package administration is not a capability GHCR offers",
          "Literal\n   zero-human package administration is a capability GHCR offers",
        )
      end,
    ],
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
      "Gateway adoption removed from release documentation",
      "Release documentation must adopt the health policy and route before the first-production cutover",
      lambda do |_candidate, release, _readiness|
        replace_once!(
          release,
          ".github/scripts/cma-trigger-gateway-launch.sh adoption --adopt",
          ".github/scripts/cma-trigger-gateway-state.sh --check-adopted",
        )
      end,
    ],
    [
      "pre-reopen route gate removed from release documentation",
      "Release documentation must prove the direct route and OpenAPI body around traffic restoration",
      lambda do |_candidate, release, _readiness|
        replace_once!(
          release,
          "if ! cma_route_and_policies_ready; then",
          "if ! true; then",
        )
      end,
    ],
    [
      "OpenAPI body validation weakened in release documentation",
      "Release documentation must prove the direct route and OpenAPI body around traffic restoration",
      lambda do |_candidate, release, _readiness|
        replace_once!(
          release,
          '(.paths["/api/v1/sessions"].get | type) == "object"',
          '(.paths | type) == "object"',
        )
      end,
    ],
    [
      "post-reopen route gate removed from release documentation",
      "Release documentation must reassert the direct route before and after traffic restoration",
      lambda do |_candidate, release, _readiness|
        contract = "if ! cma_route_and_policies_ready; then"
        index = release.rindex(contract)
        raise "self-test fixture is missing the second route gate" unless index

        release[index, contract.length] = "if ! true; then"
      end,
    ],
    [
      "honest GHCR limitation removed from release documentation",
      "Release documentation must state that D-09 cannot prevent or retract GHCR image tags",
      lambda do |_candidate, release, _readiness|
        replace_once!(release, "cannot prevent or retract those image tags", "prevents those image tags")
      end,
    ],
    [
      "routing-graph freeze narrowed in release-readiness",
      "release-readiness must retain the routing-graph freeze and signal contract",
      lambda do |_candidate, _release, readiness|
        replace_once!(
          readiness,
          "entire checked routing graph",
          "four managed objects",
        )
      end,
    ],
    [
      "EndpointSlice drain promoted to the zero-writer boundary",
      "Release documentation must not treat EndpointSlice drain as the zero-writer boundary",
      lambda do |_candidate, release, _readiness|
        replace_once!(
          release,
          "Every advertised zonal NEG must be empty across two coherent sweeps under unchanged Service UID, resourceVersion, NEG identity, and EndpointSlice identities",
          "An empty EndpointSlice alone proves the external Gateway has drained",
        )
      end,
    ],
  ]

  tests.each do |name, expected_error, mutation|
    candidate = cloned(workflow)
    restore_separator = "\n<!-- publish-restore-self-test-source -->\n"
    candidate_release_bundle = release_doc.dup + restore_separator + restore_source.dup
    candidate_readiness = readiness_doc.dup
    mutation.call(candidate, candidate_release_bundle, candidate_readiness)
    candidate_release, candidate_restore = candidate_release_bundle.split(
      restore_separator,
      2,
    )
    unless candidate_release && candidate_restore
      raise ContractError, "self-test #{name.inspect} corrupted its release fixture"
    end
    begin
      verify_contract(
        candidate,
        candidate_release,
        candidate_readiness,
        candidate_restore,
      )
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

unless ARGV.empty? || ARGV == ["--self-test"] || ARGV == ["--emit-step10"] ||
       ARGV == ["--emit-step10-invocation"]
  warn "usage: #{File.basename($PROGRAM_NAME)} [--self-test|--emit-step10|--emit-step10-invocation]"
  exit 2
end

if ARGV == ["--emit-step10"]
  print release_step10_script
  exit
end

if ARGV == ["--emit-step10-invocation"]
  print release_step10_invocation(File.read(RELEASE_DOC_PATH))
  exit
end

workflow, release_doc, readiness_doc = load_inputs
ci_workflow, smoke_source = load_d09_host_inputs
gateway_state_source = File.read(GATEWAY_STATE_PATH)
gateway_adoption_source = File.read(GATEWAY_ADOPTION_PATH)
gateway_source_contract_source = File.read(GATEWAY_SOURCE_CONTRACT_PATH)
gateway_restore_traffic_source = File.read(GATEWAY_RESTORE_TRAFFIC_PATH)
verify_contract(workflow, release_doc, readiness_doc, gateway_restore_traffic_source)
verify_d09_host_contract(ci_workflow, workflow, smoke_source)
verify_gateway_release_contract(
  ci_workflow,
  release_doc,
  gateway_state_source,
  gateway_adoption_source,
  gateway_source_contract_source,
  gateway_restore_traffic_source,
)
if ARGV == ["--self-test"]
  run_self_tests(
    workflow,
    release_doc,
    readiness_doc,
    gateway_restore_traffic_source,
  )
  run_d09_host_self_tests(ci_workflow, workflow, smoke_source)
  run_gateway_release_self_tests(
    ci_workflow,
    release_doc,
    gateway_state_source,
    gateway_adoption_source,
    gateway_source_contract_source,
    gateway_restore_traffic_source,
  )
else
  puts "CMA Trigger publish contract passed"
end

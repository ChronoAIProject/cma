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
  ci_workflow: ".github/workflows/ci.yml",
  workflow: ".github/workflows/cma-trigger-provider-smoke.yml",
  smoke: ".github/scripts/cma-trigger-provider-smoke.mjs",
  settlement: ".github/scripts/cma-trigger-provider-object-settlement.mjs",
  settlement_tests: ".github/scripts/cma-trigger-provider-object-settlement-tests.mjs",
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

PINNED_PROVIDER_ACTIONS = {
  "actions/checkout" => "fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09",
  "actions/download-artifact" => "d3f86a106a0bac45b974a628896c90dbdf5c8093",
  "actions/setup-node" => "249970729cb0ef3589644e2896645e5dc5ba9c38",
  "azure/setup-kubectl" => "776406bce94f63e41d621b960d78ee25c8b76ede",
  "actions/upload-artifact" => "ea165f8d65b6e75b540449e92b4886f43607fa02",
}.freeze

PINNED_CI_IMAGE_ACTION_REFS = [
  "actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09",
  "docker/setup-qemu-action@c7c53464625b32c7a7e944ae62b3e17d2b600130",
  "docker/setup-buildx-action@8d2750c68a42422c14e847fe6c8ac0403b4cbd6f",
  "docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8",
  "docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8",
  "actions/setup-node@249970729cb0ef3589644e2896645e5dc5ba9c38",
  "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02",
  "aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25",
  "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02",
].freeze

CI_IMAGE_PROVENANCE_CONDITION =
  "${{ success() && github.repository_id == '1330597394' && github.event_name == 'push' && github.ref == 'refs/heads/develop' && (matrix.component == 'api' || matrix.component == 'runtime') }}"

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

def closed_key_sets(source)
  source.scan(/keys\)?\s*==\s*\[(.*?)\]/m).map do |match|
    match.first.scan(/"([^"]+)"/).flatten
  end
end

def source_matches(source, pattern, message)
  check(source.match?(pattern), message)
end

def load_inputs
  ci_workflow_source = File.read(File.join(ROOT, PATHS.fetch(:ci_workflow)))
  workflow_source = File.read(File.join(ROOT, PATHS.fetch(:workflow)))
  example_source = File.read(File.join(ROOT, PATHS.fetch(:example)))
  manifests = {}
  [:configmap, :deployment, :secretproviderclass, :serviceaccount].each do |name|
    source = File.read(File.join(ROOT, PATHS.fetch(name)))
    manifests[name] = YAML.safe_load_stream(source, aliases: true).compact
  end
  sources = {}
  [
    :smoke,
    :settlement,
    :settlement_tests,
    :trigger,
    :config,
    :kernels,
    :codex,
    :hosted,
    :reclaimer,
  ].each do |name|
    sources[name] = File.read(File.join(ROOT, PATHS.fetch(name)))
  end
  {
    ci_workflow: YAML.safe_load(ci_workflow_source, aliases: true),
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
    "runtime_image_ids",
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
  check(
    example["schema_version"] == 3 &&
      example["runtime_image_ids"].is_a?(Array) &&
      example["runtime_image_ids"].include?(example["runtime_image_digest"]),
    "provider example must bind the closed runtime image identity set",
  )
  check(
    example["fixture_policy"] == {
      "repository_owner" => "ChronoAIProject",
      "lifecycle" => "persistent_public_fixture",
      "secrets" => "forbidden",
      "github_cleanup" => "preserve",
      "cma_state_cleanup" => "destroy_with_terminal_ephemeral_environment",
      "session_cleanup" => "stop_runtime_before_terminal_ephemeral_teardown",
    },
    "provider example must state the exact terminal ephemeral lifecycle",
  )
end

def verify_ci_image_provenance(ci_workflow)
  jobs = ci_workflow.fetch("jobs")
  images = jobs.fetch("images")
  check(
    images["name"] == "Images" &&
      images["runs-on"] == "ubuntu-24.04" && images["timeout-minutes"] == 60,
    "CI image provenance must retain the reviewed job name, Ubuntu runner, and transfer timeout",
  )
  check(
    images.dig("strategy", "matrix", "component") == %w[api web runtime],
    "CI image matrix must retain api, web, and runtime",
  )
  image_steps = images.fetch("steps")
  action_refs = image_steps.each_with_object([]) do |step, refs|
    refs << step["uses"] if step.is_a?(Hash) && step["uses"]
  end
  check(
    action_refs == PINNED_CI_IMAGE_ACTION_REFS &&
      action_refs.all? { |ref| ref.match?(%r{\A[^/@]+/[^/@]+@[0-9a-f]{40}\z}) },
    "every CI image-provenance action must retain its exact reviewed SHA",
  )

  build = named_step(image_steps, "Build image for all target platforms")
  load = named_step(image_steps, "Load native image for scanning")
  runtime_pass = named_step(image_steps, "Require passing Trigger runtime evidence")
  scan = named_step(image_steps, "Vulnerability scan")
  export = named_step(image_steps, "Export scanned image provenance")
  upload = named_step(image_steps, "Upload scanned image provenance")
  check(build && load && runtime_pass && scan && export && upload, "CI image provenance chain is incomplete")
  [build, load].each do |step|
    check(
      step.dig("with", "build-args").to_s.include?("CHRONO_BUILD_SHA=${{ github.sha }}"),
      "CI image builds must bind the exact source SHA",
    )
  end
  check(
    scan["uses"] == "aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25" &&
      scan.dig("with", "image-ref") == "cma-${{ matrix.component }}:ci" &&
      scan.dig("with", "exit-code") == "1",
    "CI image provenance must follow the reviewed failing Trivy gate",
  )
  check(
    image_steps.index(load) < image_steps.index(runtime_pass) &&
      image_steps.index(runtime_pass) < image_steps.index(scan) &&
      image_steps.index(scan) < image_steps.index(export) &&
      image_steps.index(export) < image_steps.index(upload),
    "CI must load, runtime-gate, scan, export, then upload image provenance",
  )

  check(
    export["if"] == CI_IMAGE_PROVENANCE_CONDITION &&
      upload["if"] == CI_IMAGE_PROVENANCE_CONDITION,
    "CI image provenance must be restricted to successful develop pushes for api and runtime",
  )
  check(
    export.fetch("env") == {
      "COMPONENT" => "${{ matrix.component }}",
      "EXPECTED_REPOSITORY_ID" => "1330597394",
      "IMAGE_EXPORT_ROOT" => "${{ runner.temp }}/cma-trigger-ci-image-${{ matrix.component }}-${{ github.sha }}",
    },
    "CI image export must bind its reviewed repository and exact-SHA path",
  )
  export_run = export.fetch("run")
  [
    'component="$COMPONENT"',
    'case "$component" in',
    "api|runtime)",
    '[ "$GITHUB_EVENT_NAME" != push ]',
    '[ "$GITHUB_REF" != refs/heads/develop ]',
    '[ "$GITHUB_REPOSITORY_ID" != "$EXPECTED_REPOSITORY_ID" ]',
    '[[ ! "$GITHUB_SHA" =~ ^[0-9a-f]{40}$ ]]',
    'reference="cma-${component}:ci"',
    'archive_name="cma-trigger-${component}.tar"',
    'metadata_name="cma-trigger-${component}.json"',
    "docker image inspect --format '{{.Id}}' \"$reference\"",
    'docker image save --output "$archive_path" "$reference"',
    'archive_sha256=$(sha256sum "$archive_path"',
    "archive_size=$(stat --format='%s' \"$archive_path\")",
    '--arg source_sha "$GITHUB_SHA"',
    'kind: "cma-trigger-ci-image"',
    'source_sha: $source_sha',
    '.source_sha == $source_sha',
    '.image_id == $image_id',
    '.archive_sha256 == $archive_sha256',
    '.archive_size == $archive_size',
    'chmod 0400 "$archive_path"',
    'chmod 0400 "$metadata_path"',
  ].each do |contract|
    check(export_run.include?(contract), "CI image export is missing #{contract}")
  end
  metadata_schema = export_run.match(/and \(keys == \[(.*?)\]\)/m)
  metadata_keys = metadata_schema ? metadata_schema[1].scan(/"([^"]+)"/).flatten : []
  check(
    metadata_keys == %w[
      archive_name archive_sha256 archive_size component image_id image_reference
      kind repository_id run_attempt run_id schema_version source_sha
    ],
    "CI image metadata must retain its exact closed schema",
  )

  upload_with = upload.fetch("with")
  check(
    upload["uses"] ==
      "actions/upload-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/upload-artifact")}" &&
      upload_with["name"] ==
        "cma-trigger-ci-image-${{ matrix.component }}-${{ github.sha }}" &&
      upload_with["if-no-files-found"] == "error" &&
      upload_with["retention-days"] == 1 &&
      upload_with["compression-level"] == 0 &&
      upload_with["overwrite"] == true,
    "CI image transfer artifact must be exact, fail closed, and expire after one day",
  )
  expected_paths = %w[tar json].map do |extension|
    "${{ runner.temp }}/cma-trigger-ci-image-${{ matrix.component }}-${{ github.sha }}/cma-trigger-${{ matrix.component }}.#{extension}"
  end
  actual_paths = upload_with.fetch("path").lines.map(&:strip).reject(&:empty?)
  check(actual_paths == expected_paths, "CI image artifact must contain exactly its archive and metadata")

  smoke_contract = jobs.fetch("trigger-provider-smoke-contract")
  harness = named_step(
    smoke_contract.fetch("steps"),
    "Harness and fixture configuration are closed and valid",
  )
  check(harness, "CI must retain the Trigger provider harness gate")
  [
    "node --check .github/scripts/cma-trigger-provider-object-settlement.mjs",
    "node --check .github/scripts/cma-trigger-provider-object-settlement-tests.mjs",
    "node .github/scripts/cma-trigger-provider-object-settlement-tests.mjs",
  ].each do |contract|
    check(
      harness.fetch("run").include?(contract),
      "CI provider-object settlement gate is missing #{contract}",
    )
  end
end

def verify_workflow(workflow)
  triggers = workflow.fetch(true)
  check(
    triggers.keys == ["workflow_dispatch"],
    "provider smoke must remain workflow_dispatch-only",
  )
  check(
    workflow.fetch("run-name") ==
      "CMA Trigger provider smoke ${{ inputs.dispatch_nonce }}",
    "provider smoke run name must retain its exact operator nonce",
  )
  check(
    triggers.fetch("workflow_dispatch") == {
      "inputs" => {
        "dispatch_nonce" => {
          "description" => "One operator-generated 128-bit correlation nonce",
          "required" => true,
          "type" => "string",
        },
        "expected_fork_mode" => {
          "description" => "Require the contributor fork to be absent or already reusable before the run",
          "required" => true,
          "type" => "choice",
          "options" => ["new", "reuse"],
        },
        "expected_head_sha" => {
          "description" => "Exact reviewed lowercase develop commit SHA that this run must execute",
          "required" => true,
          "type" => "string",
        },
        "handoff_secret_name" => {
          "description" => "Nonce-bound one-run protected handoff secret name",
          "required" => true,
          "type" => "string",
        },
        "preservation_confirmation" => {
          "description" => "Type PRESERVE to retain the fork, branch, commit, and pull request after the run",
          "required" => true,
          "type" => "string",
        },
      },
    },
    "provider smoke must retain its exact closed dispatch input schema",
  )
  check(
    workflow.fetch("permissions") == {
      "actions" => "read",
      "contents" => "read",
    },
    "provider smoke must retain only environment-inspection and checkout permissions",
  )
  check(
    workflow.dig("concurrency", "cancel-in-progress") == false,
    "provider smoke runs must never cancel an in-flight writer",
  )

  jobs = workflow.fetch("jobs")
  plan = jobs.fetch("plan")
  plan_steps = plan.fetch("steps")
  check(
    plan.fetch("runs-on") == "ubuntu-latest" && plan.fetch("timeout-minutes") == 30,
    "provider image planning must retain the reviewed hosted runner and transfer timeout",
  )
  check(
    plan.fetch("outputs") == {
      "approved_manifest_artifact_digest" => "${{ steps.approved_manifest_upload.outputs.artifact-digest }}",
      "approved_manifest_artifact_id" => "${{ steps.approved_manifest_upload.outputs.artifact-id }}",
      "approved_manifest_sha256" => "${{ steps.approved_manifest.outputs.manifest_sha256 }}",
      "api_artifact_digest" => "${{ steps.ci_images.outputs.api_artifact_digest }}",
      "api_artifact_id" => "${{ steps.ci_images.outputs.api_artifact_id }}",
      "ci_run_attempt" => "${{ steps.ci_images.outputs.ci_run_attempt }}",
      "ci_run_id" => "${{ steps.ci_images.outputs.ci_run_id }}",
      "runtime_artifact_digest" => "${{ steps.ci_images.outputs.runtime_artifact_digest }}",
      "runtime_artifact_id" => "${{ steps.ci_images.outputs.runtime_artifact_id }}",
    },
    "provider plan must expose only exact approved run and artifact identities",
  )
  consent = named_step(
    plan_steps,
    "Require the deployed develop workflow and preservation consent",
  )
  check(consent, "provider smoke must require explicit preservation consent")
  check(
    consent.fetch("env") == {
      "DISPATCH_NONCE" => "${{ inputs.dispatch_nonce }}",
      "DISPATCH_REF" => "${{ github.ref }}",
      "EXPECTED_HEAD_SHA" => "${{ inputs.expected_head_sha }}",
      "HANDOFF_SECRET_NAME" => "${{ inputs.handoff_secret_name }}",
      "PRESERVATION_CONFIRMATION" => "${{ inputs.preservation_confirmation }}",
      "WORKFLOW_SHA" => "${{ github.sha }}",
    },
    "provider smoke plan must bind the dispatch ref, reviewed head, consent, and event SHA",
  )
  consent_run = consent.fetch("run")
  check(consent_run.include?('"refs/heads/develop"'), "provider smoke must run only from develop")
  check(
    contains_in_order?(
      consent_run,
      [
        '[[ ! "$DISPATCH_NONCE" =~ ^[0-9a-f]{32}$ ]]',
        'expected_handoff_secret_name="CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_${DISPATCH_NONCE^^}"',
        '[ "$HANDOFF_SECRET_NAME" != "$expected_handoff_secret_name" ]',
        '[[ ! "$EXPECTED_HEAD_SHA" =~ ^[0-9a-f]{40}$ ]]',
        '[ "$WORKFLOW_SHA" != "$EXPECTED_HEAD_SHA" ]',
      ],
    ),
    "provider smoke must reject malformed or moved reviewed develop snapshots",
  )
  check(consent_run.include?('"PRESERVE"'), "provider smoke must retain exact preservation consent")

  environment = named_step(plan_steps, "Require the live protected smoke environment")
  check(environment, "provider smoke must verify its live GitHub environment")
  [
    "environments/cma-trigger-smoke",
    ".can_admins_bypass == false",
    ".protection_rules | length == 2",
    '[.protection_rules[] | select(.type == "branch_policy")] | length == 1',
    '[.protection_rules[] | select(.type == "required_reviewers")] | length == 1',
    '.type == "required_reviewers"',
    ".prevent_self_review == true",
    '.reviewers | length == 1',
    '.reviewers[0].type == "Team"',
    ".reviewers[0].reviewer.id == 15748251",
    '.reviewers[0].reviewer.node_id == "T_kwDODyHfns4A8Eyb"',
    '.reviewers[0].reviewer.slug == "ai-application"',
    ".deployment_branch_policy.protected_branches == true",
    ".deployment_branch_policy.custom_branch_policies == false",
  ].each do |contract|
    check(
      environment.fetch("run").include?(contract),
      "provider environment verification is missing #{contract}",
    )
  end

  resolve_images = named_step(plan_steps, "Resolve exact successful CI image artifacts")
  check(resolve_images && resolve_images["id"] == "ci_images", "provider plan must resolve exact CI images")
  check(
    resolve_images.fetch("env") == {
      "EXPECTED_REPOSITORY_ID" => "1330597394",
      "GH_TOKEN" => "${{ github.token }}",
      "SOURCE_SHA" => "${{ github.sha }}",
    },
    "CI image resolution must bind only GitHub read authority and the exact source SHA",
  )
  resolve_run = resolve_images.fetch("run")
  [
    "actions/workflows/ci.yml/runs?branch=develop&event=push&status=completed",
    '.name == "CI"',
    '.path == ".github/workflows/ci.yml"',
    '.event == "push"',
    '.status == "completed"',
    '.conclusion == "success"',
    '.head_branch == "develop"',
    '.head_sha == $source_sha',
    '.repository.id == $repository_id',
    '.head_repository.id == $repository_id',
    '[ "$run_count" -ne 1 ]',
    'actions/runs/$ci_run_id/jobs?filter=latest',
    '.name == "Images (api)"',
    '.name == "Images (runtime)"',
    'actions/runs/$ci_run_id/artifacts?per_page=100',
    'artifact_name="cma-trigger-ci-image-${component}-${SOURCE_SHA}"',
    '[ "$artifact_count" -ne 1 ]',
    '.expired == false',
    '.digest | test("^sha256:[0-9a-f]{64}$")',
    '.workflow_run.id == $run_id',
    '.workflow_run.head_sha == $source_sha',
    'echo "${component}_artifact_id=$artifact_id"',
    'echo "${component}_artifact_digest=$artifact_digest"',
    'echo "ci_run_id=$ci_run_id"',
    'echo "ci_run_attempt=$ci_run_attempt"',
  ].each do |contract|
    check(resolve_run.include?(contract), "exact CI image resolution is missing #{contract}")
  end

  plan_api_download = named_step(plan_steps, "Download exact API image artifact for approval")
  plan_runtime_download = named_step(plan_steps, "Download exact runtime image artifact for approval")
  pinned_download =
    "actions/download-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/download-artifact")}"
  check(
    plan_api_download && plan_api_download["uses"] == pinned_download &&
      plan_api_download.fetch("with") == {
        "artifact-ids" => "${{ steps.ci_images.outputs.api_artifact_id }}",
        "path" => "${{ runner.temp }}/cma-trigger-image-provenance-plan/api",
        "github-token" => "${{ github.token }}",
        "repository" => "${{ github.repository }}",
        "run-id" => "${{ steps.ci_images.outputs.ci_run_id }}",
      },
    "provider plan must download the API artifact by exact ID and run",
  )
  check(
    plan_runtime_download && plan_runtime_download["uses"] == pinned_download &&
      plan_runtime_download.fetch("with") == {
        "artifact-ids" => "${{ steps.ci_images.outputs.runtime_artifact_id }}",
        "path" => "${{ runner.temp }}/cma-trigger-image-provenance-plan/runtime",
        "github-token" => "${{ github.token }}",
        "repository" => "${{ github.repository }}",
        "run-id" => "${{ steps.ci_images.outputs.ci_run_id }}",
      },
    "provider plan must download the runtime artifact by exact ID and run",
  )

  build_manifest = named_step(plan_steps, "Build closed approved image provenance manifest")
  check(
    build_manifest && build_manifest["id"] == "approved_manifest",
    "provider plan must build one closed approved image manifest",
  )
  build_manifest_run = build_manifest.fetch("run")
  source_image_metadata_keys = %w[
    archive_name archive_sha256 archive_size component image_id image_reference
    kind repository_id run_attempt run_id schema_version source_sha
  ]
  approved_image_keys = %w[
    archive_name archive_sha256 archive_size artifact_digest artifact_id
    artifact_name image_id image_reference metadata_name metadata_sha256
  ]
  approved_manifest_keys = %w[ci_run images kind repository_id schema_version source_sha]
  approved_ci_run_keys = %w[
    attempt conclusion event head_branch head_sha id workflow_path
  ]
  approved_image_map_keys = %w[api runtime]
  [
    'find "$artifact_dir" -mindepth 1 -maxdepth 1',
    '"archive_name", "archive_sha256", "archive_size", "component"',
    '"run_attempt", "run_id", "schema_version", "source_sha"',
    'archive_sha256=$(sha256sum "$archive_path"',
    "archive_size=$(stat --format='%s' \"$archive_path\")",
    '[ "$archive_sha256" != "$expected_archive_sha256" ]',
    'docker image load --input "$archive_path"',
    "docker image inspect --format '{{.Id}}' \"$image_reference\"",
    'metadata_sha256=$(sha256sum "$metadata_path"',
    'artifact_digest: $artifact_digest',
    'kind: "cma-trigger-approved-ci-images"',
    'workflow_path: ".github/workflows/ci.yml"',
    'images: {api: $api[0], runtime: $runtime[0]}',
    'keys == ["ci_run", "images", "kind", "repository_id", "schema_version", "source_sha"]',
    '(.images | keys) == ["api", "runtime"]',
    '.ci_run.event == "push"',
    '.ci_run.head_branch == "develop"',
    '.ci_run.head_sha == $source_sha',
    '.ci_run.conclusion == "success"',
    '.images.api.artifact_id != .images.runtime.artifact_id',
    'chmod 0400 "$manifest_path"',
    'manifest_sha256=$(sha256sum "$manifest_path"',
    'echo "manifest_sha256=$manifest_sha256" >> "$GITHUB_OUTPUT"',
  ].each do |contract|
    check(build_manifest_run.include?(contract), "approved image manifest construction is missing #{contract}")
  end
  check(
    closed_key_sets(build_manifest_run) == [
      source_image_metadata_keys,
      approved_image_keys,
      approved_manifest_keys,
      approved_ci_run_keys,
      approved_image_map_keys,
    ],
    "approved image manifest construction must enforce every exact closed schema",
  )

  manifest_upload = named_step(plan_steps, "Persist approved image provenance manifest")
  check(
    manifest_upload && manifest_upload["id"] == "approved_manifest_upload" &&
      manifest_upload["uses"] ==
        "actions/upload-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/upload-artifact")}" &&
      manifest_upload.fetch("with") == {
        "name" => "cma-trigger-approved-ci-images-${{ github.run_id }}",
        "path" => "${{ runner.temp }}/cma-trigger-approved-image-provenance/approved-image-provenance.json",
        "if-no-files-found" => "error",
        "retention-days" => 1,
        "compression-level" => 0,
        "overwrite" => true,
      },
    "approved image manifest must be persisted as one exact short-lived artifact",
  )
  manifest_summary = named_step(plan_steps, "Summarize approved image provenance")
  check(manifest_summary, "approved image provenance must be visible before protected approval")
  [
    "Approved CI image provenance",
    ".images.api.artifact_id",
    ".images.api.artifact_digest",
    ".images.api.archive_sha256",
    ".images.api.archive_size",
    ".images.api.image_id",
    ".images.runtime.artifact_id",
    ".images.runtime.artifact_digest",
    ".images.runtime.archive_sha256",
    ".images.runtime.archive_size",
    ".images.runtime.image_id",
    "APPROVED_MANIFEST_ARTIFACT_ID",
    "APPROVED_MANIFEST_ARTIFACT_DIGEST",
    "APPROVED_MANIFEST_SHA256",
    '>> "$GITHUB_STEP_SUMMARY"',
  ].each do |contract|
    check(manifest_summary.fetch("run").include?(contract), "image approval summary is missing #{contract}")
  end
  check(
    plan_steps.index(environment) < plan_steps.index(resolve_images) &&
      plan_steps.index(resolve_images) < plan_steps.index(plan_api_download) &&
      plan_steps.index(plan_api_download) < plan_steps.index(plan_runtime_download) &&
      plan_steps.index(plan_runtime_download) < plan_steps.index(build_manifest) &&
      plan_steps.index(build_manifest) < plan_steps.index(manifest_upload) &&
      plan_steps.index(manifest_upload) < plan_steps.index(manifest_summary),
    "provider plan must resolve, inspect, persist, then summarize image provenance",
  )

  plan_contract = named_step(plan_steps, "Validate the checked-in harness contract")
  check(plan_contract, "provider smoke plan must validate every staging component")
  [
    "cma-trigger-provider-smoke.mjs --self-test",
    "cma-trigger-provider-object-settlement.mjs",
    "cma-trigger-provider-object-settlement-tests.mjs",
    "cma-trigger-kind-staging-fixture.mjs",
    "cma-trigger-nyxid-smoke-bootstrap-tests.mjs",
    "bash -n .github/scripts/cma-trigger-kind-staging.sh",
    "shellcheck .github/scripts/cma-trigger-kind-staging.sh",
    ".github/scripts/cma-trigger-kind-staging-tests.sh",
  ].each do |contract|
    check(
      plan_contract.fetch("run").include?(contract),
      "provider smoke plan validation is missing #{contract}",
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
    provider.fetch("runs-on") == "ubuntu-latest" &&
      provider.fetch("timeout-minutes") == 120,
    "ephemeral Kind staging must retain the reviewed hosted runner and timeout",
  )
  check(
    provider.fetch("outputs") == {
      "evidence_id" => "${{ steps.evidence_audit.outputs.evidence_id }}",
      "evidence_result" => "${{ steps.evidence_audit.outputs.evidence_result }}",
      "retained_artifact_url" => "${{ steps.evidence_upload.outputs.artifact-url }}",
    },
    "provider smoke must expose the closed evidence identity, result, and artifact URL",
  )
  expected_provider_env = {
    "CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE" => "${{ vars.CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE }}",
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
    provider.fetch("env") == expected_provider_env,
    "provider smoke must bind only reviewed ephemeral paths, fixture variables, and run identity",
  )

  provider_steps = provider.fetch("steps")
  custody_paths = named_step(provider_steps, "Bind ephemeral staging custody paths")
  check(custody_paths, "provider smoke must bind runner custody before bootstrap")
  check(
    custody_paths.fetch("run").include?(
      'staging_root="$RUNNER_TEMP/cma-trigger-kind-staging"',
    ) && custody_paths.fetch("run").include?(
      'handoff_path="$staging_root/credentials/nyxid-handoff.json"',
    ) && custody_paths.fetch("run").include?(
      'CMA_TRIGGER_KIND_STAGING_ROOT=%s\\n',
    ) && custody_paths.fetch("run").include?(
      'CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_PATH=%s\\n',
    ),
    "provider smoke must derive both custody paths from RUNNER_TEMP at step runtime",
  )
  action_uses = (plan_steps + provider_steps).each_with_object([]) do |step, uses|
    uses << step["uses"] if step.is_a?(Hash) && step["uses"]
  end
  expected_action_uses = [
    "actions/download-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/download-artifact")}",
    "actions/download-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/download-artifact")}",
    "actions/download-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/download-artifact")}",
    "actions/download-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/download-artifact")}",
    "actions/download-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/download-artifact")}",
    "actions/checkout@#{PINNED_PROVIDER_ACTIONS.fetch("actions/checkout")}",
    "actions/setup-node@#{PINNED_PROVIDER_ACTIONS.fetch("actions/setup-node")}",
    "actions/checkout@#{PINNED_PROVIDER_ACTIONS.fetch("actions/checkout")}",
    "actions/setup-node@#{PINNED_PROVIDER_ACTIONS.fetch("actions/setup-node")}",
    "azure/setup-kubectl@#{PINNED_PROVIDER_ACTIONS.fetch("azure/setup-kubectl")}",
    "actions/upload-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/upload-artifact")}",
    "actions/upload-artifact@#{PINNED_PROVIDER_ACTIONS.fetch("actions/upload-artifact")}",
  ]
  check(
    action_uses.sort == expected_action_uses.sort &&
      action_uses.all? { |uses| uses.match?(%r{\A[^/@]+/[^/@]+@[0-9a-f]{40}\z}) },
    "every provider-workflow action must be one reviewed full-commit pin",
  )
  recheck = named_step(provider_steps, "Revalidate the protected smoke environment after approval")
  check(recheck, "provider writer must revalidate its environment after approval")
  [
    "environments/cma-trigger-smoke",
    ".can_admins_bypass == false",
    ".protection_rules | length == 2",
    '[.protection_rules[] | select(.type == "branch_policy")] | length == 1',
    '[.protection_rules[] | select(.type == "required_reviewers")] | length == 1',
    '.type == "required_reviewers"',
    ".prevent_self_review == true",
    '.reviewers | length == 1',
    '.reviewers[0].type == "Team"',
    ".reviewers[0].reviewer.id == 15748251",
    '.reviewers[0].reviewer.node_id == "T_kwDODyHfns4A8Eyb"',
    '.reviewers[0].reviewer.slug == "ai-application"',
    ".deployment_branch_policy.protected_branches == true",
    ".deployment_branch_policy.custom_branch_policies == false",
  ].each do |contract|
    check(
      recheck.fetch("run").include?(contract),
      "post-approval environment verification is missing #{contract}",
    )
  end

  approval = named_step(provider_steps, "Capture independent approval provenance")
  check(approval && approval["id"] == "approval", "provider approval capture is absent")
  [
    'actions/runs/$GITHUB_RUN_ID/approvals',
    'users/$GITHUB_TRIGGERING_ACTOR',
    '.name == "cma-trigger-smoke"',
    "approval_observed_at=$(date -u +'%Y-%m-%dT%H:%M:%SZ')",
    'approval_observed_at: $approval_observed_at',
    "unique_by(.account_id)",
    ".account_id != $operator.account_id",
    "reviewers_json=$reviewers",
  ].each do |contract|
    check(approval.fetch("run").include?(contract), "approval provenance is missing #{contract}")
  end
  check(
    !approval.fetch("run").include?(".submitted_at"),
    "approval provenance must not invent an absent timestamp",
  )

  artifact_recheck = named_step(
    provider_steps,
    "Revalidate approved image artifact identities after approval",
  )
  check(artifact_recheck, "protected provider job must revalidate every approved artifact identity")
  check(
    artifact_recheck.fetch("env") == {
      "API_ARTIFACT_DIGEST" => "${{ needs.plan.outputs.api_artifact_digest }}",
      "API_ARTIFACT_ID" => "${{ needs.plan.outputs.api_artifact_id }}",
      "APPROVED_MANIFEST_ARTIFACT_DIGEST" => "${{ needs.plan.outputs.approved_manifest_artifact_digest }}",
      "APPROVED_MANIFEST_ARTIFACT_ID" => "${{ needs.plan.outputs.approved_manifest_artifact_id }}",
      "APPROVED_MANIFEST_SHA256" => "${{ needs.plan.outputs.approved_manifest_sha256 }}",
      "CI_RUN_ATTEMPT" => "${{ needs.plan.outputs.ci_run_attempt }}",
      "CI_RUN_ID" => "${{ needs.plan.outputs.ci_run_id }}",
      "EXPECTED_REPOSITORY_ID" => "1330597394",
      "GH_TOKEN" => "${{ github.token }}",
      "RUNTIME_ARTIFACT_DIGEST" => "${{ needs.plan.outputs.runtime_artifact_digest }}",
      "RUNTIME_ARTIFACT_ID" => "${{ needs.plan.outputs.runtime_artifact_id }}",
      "SOURCE_SHA" => "${{ github.sha }}",
    },
    "post-approval image revalidation must consume only approved plan identities",
  )
  artifact_recheck_run = artifact_recheck.fetch("run")
  [
    'actions/runs/$CI_RUN_ID',
    '.run_attempt == $run_attempt',
    '.path == ".github/workflows/ci.yml"',
    '.event == "push"',
    '.status == "completed"',
    '.conclusion == "success"',
    '.head_branch == "develop"',
    '.head_sha == $source_sha',
    'actions/artifacts/$artifact_id',
    '.id == $artifact_id',
    '.digest == $artifact_digest',
    '.expired == false',
    '.workflow_run.id == $run_id',
    '.workflow_run.head_sha == $source_sha',
    '[[ ! "$APPROVED_MANIFEST_SHA256" =~ ^[0-9a-f]{64}$ ]]',
    '"cma-trigger-approved-ci-images-${GITHUB_RUN_ID}"',
    '"cma-trigger-ci-image-api-${SOURCE_SHA}"',
    '"cma-trigger-ci-image-runtime-${SOURCE_SHA}"',
    '"$APPROVED_MANIFEST_ARTIFACT_ID"',
    '"$API_ARTIFACT_ID"',
    '"$RUNTIME_ARTIFACT_ID"',
  ].each do |contract|
    check(artifact_recheck_run.include?(contract), "post-approval artifact revalidation is missing #{contract}")
  end

  manifest_download = named_step(
    provider_steps,
    "Download exact approved image provenance manifest",
  )
  protected_api_download = named_step(provider_steps, "Download exact approved API image artifact")
  protected_runtime_download = named_step(
    provider_steps,
    "Download exact approved runtime image artifact",
  )
  check(
    manifest_download && manifest_download["uses"] == pinned_download &&
      manifest_download.fetch("with") == {
        "artifact-ids" => "${{ needs.plan.outputs.approved_manifest_artifact_id }}",
        "path" => "${{ runner.temp }}/cma-trigger-ci-images/manifest",
        "github-token" => "${{ github.token }}",
        "repository" => "${{ github.repository }}",
        "run-id" => "${{ github.run_id }}",
      },
    "protected job must download the exact approved manifest artifact by ID",
  )
  check(
    protected_api_download && protected_api_download["uses"] == pinned_download &&
      protected_api_download.fetch("with") == {
        "artifact-ids" => "${{ needs.plan.outputs.api_artifact_id }}",
        "path" => "${{ runner.temp }}/cma-trigger-ci-images/api",
        "github-token" => "${{ github.token }}",
        "repository" => "${{ github.repository }}",
        "run-id" => "${{ needs.plan.outputs.ci_run_id }}",
      },
    "protected job must download the exact approved API artifact by ID",
  )
  check(
    protected_runtime_download && protected_runtime_download["uses"] == pinned_download &&
      protected_runtime_download.fetch("with") == {
        "artifact-ids" => "${{ needs.plan.outputs.runtime_artifact_id }}",
        "path" => "${{ runner.temp }}/cma-trigger-ci-images/runtime",
        "github-token" => "${{ github.token }}",
        "repository" => "${{ github.repository }}",
        "run-id" => "${{ needs.plan.outputs.ci_run_id }}",
      },
    "protected job must download the exact approved runtime artifact by ID",
  )

  image_verification = named_step(
    provider_steps,
    "Verify approved image bytes and expose staging contract",
  )
  check(image_verification, "protected job must verify approved image bytes before credentials")
  check(
    image_verification.fetch("env") == {
      "API_ARTIFACT_DIGEST" => "${{ needs.plan.outputs.api_artifact_digest }}",
      "API_ARTIFACT_ID" => "${{ needs.plan.outputs.api_artifact_id }}",
      "APPROVED_MANIFEST_SHA256" => "${{ needs.plan.outputs.approved_manifest_sha256 }}",
      "CI_RUN_ATTEMPT" => "${{ needs.plan.outputs.ci_run_attempt }}",
      "CI_RUN_ID" => "${{ needs.plan.outputs.ci_run_id }}",
      "EXPECTED_REPOSITORY_ID" => "1330597394",
      "RUNTIME_ARTIFACT_DIGEST" => "${{ needs.plan.outputs.runtime_artifact_digest }}",
      "RUNTIME_ARTIFACT_ID" => "${{ needs.plan.outputs.runtime_artifact_id }}",
      "SOURCE_SHA" => "${{ github.sha }}",
    },
    "protected image verification must bind only the approved manifest identities",
  )
  image_verification_run = image_verification.fetch("run")
  [
    'approved_manifest="$image_root/approved-image-provenance.json"',
    'find "$manifest_download_dir" -mindepth 1 -maxdepth 1',
    'sha256sum "$downloaded_manifest"',
    '!= "$APPROVED_MANIFEST_SHA256"',
    'keys == ["ci_run", "images", "kind", "repository_id", "schema_version", "source_sha"]',
    '"archive_name", "archive_sha256", "archive_size"',
    '"artifact_digest", "artifact_id", "artifact_name", "image_id"',
    '.artifact_id == $artifact_id',
    '.artifact_digest == $digest',
    '.ci_run.id == $ci_run_id',
    '.ci_run.attempt == $ci_run_attempt',
    '.ci_run.event == "push"',
    '.ci_run.head_branch == "develop"',
    '.ci_run.head_sha == $source_sha',
    '.ci_run.conclusion == "success"',
    'sha256sum "$metadata_path"',
    'sha256sum "$archive_path"',
    "stat --format='%s' \"$archive_path\"",
    '[ "$actual_archive_sha256" != "$approved_archive_sha256" ]',
    '[ "$actual_archive_size" != "$approved_archive_size" ]',
    'docker image load --input "$archive_path"',
    "docker image inspect --format '{{.Id}}' \"$image_reference\"",
    'install -m 0400 "$downloaded_manifest" "$approved_manifest"',
    'CMA_TRIGGER_KIND_STAGING_IMAGE_PROVENANCE_PATH=$approved_manifest',
    'CMA_TRIGGER_KIND_STAGING_API_IMAGE_ARCHIVE=$image_root/api/cma-trigger-api.tar',
    'CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ARCHIVE=$image_root/runtime/cma-trigger-runtime.tar',
    "CMA_TRIGGER_KIND_STAGING_API_IMAGE_ID=$(jq -r '.images.api.image_id' \"$approved_manifest\")",
    "CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ID=$(jq -r '.images.runtime.image_id' \"$approved_manifest\")",
    '>> "$GITHUB_ENV"',
  ].each do |contract|
    check(image_verification_run.include?(contract), "protected image verification is missing #{contract}")
  end
  check(
    closed_key_sets(image_verification_run) == [
      approved_image_keys,
      approved_manifest_keys,
      approved_ci_run_keys,
      approved_image_map_keys,
      source_image_metadata_keys,
    ],
    "protected image verification must enforce every exact closed schema",
  )

  kubectl_steps = provider_steps.select do |step|
    step.is_a?(Hash) &&
      step["uses"] == "azure/setup-kubectl@#{PINNED_PROVIDER_ACTIONS.fetch("azure/setup-kubectl")}"
  end
  check(
    kubectl_steps.length == 1 &&
      kubectl_steps.first.fetch("with") == { "version" => "v1.34.1" },
    "provider smoke must install exactly the reviewed kubectl client",
  )

  bootstrap = named_step(provider_steps, "Validate the protected ephemeral staging bootstrap")
  check(bootstrap, "provider smoke must validate its protected Kind bootstrap")
  check(
    bootstrap.fetch("env") == {
      "CMA_TRIGGER_SMOKE_KUBECONFIG_B64" => "${{ secrets.CMA_TRIGGER_SMOKE_KUBECONFIG_B64 }}",
      "CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256" => "${{ vars.CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256 }}",
      "CMA_TRIGGER_SMOKE_KIND_CA_KEY_B64" => "${{ secrets.CMA_TRIGGER_SMOKE_KIND_CA_KEY_B64 }}",
    },
    "Kind bootstrap must receive exactly the observer bytes, digest, and matching CA key",
  )
  check(
    bootstrap.fetch("run") == ".github/scripts/cma-trigger-kind-staging.sh validate",
    "Kind bootstrap must run only the reviewed validator",
  )
  secret_step_indexes = provider_steps.each_index.select do |index|
    step = provider_steps[index]
    step.is_a?(Hash) && step["env"].is_a?(Hash) &&
      step["env"].values.any? { |value| value.to_s.match?(/\$\{\{ secrets(?:\.|\[)/) }
  end
  check(!secret_step_indexes.empty?, "provider smoke must retain reviewed protected credentials")
  check(
    provider_steps.index(approval) < provider_steps.index(artifact_recheck) &&
      provider_steps.index(artifact_recheck) < provider_steps.index(manifest_download) &&
      provider_steps.index(manifest_download) < provider_steps.index(protected_api_download) &&
      provider_steps.index(protected_api_download) < provider_steps.index(protected_runtime_download) &&
      provider_steps.index(protected_runtime_download) < provider_steps.index(image_verification) &&
      provider_steps.index(image_verification) < secret_step_indexes.min &&
      provider_steps.index(image_verification) < provider_steps.index(bootstrap),
    "approved artifact download and byte verification must precede every protected credential",
  )

  handoff = named_step(provider_steps, "Materialize and accept the closed NyxID handoff")
  check(handoff, "provider smoke must materialize its closed NyxID handoff")
  check(
    handoff.fetch("env") == {
      "DISPATCH_NONCE" => "${{ inputs.dispatch_nonce }}",
      "HANDOFF_SECRET_NAME" => "${{ inputs.handoff_secret_name }}",
      "NYXID_BOOTSTRAP_B64" => "${{ secrets[inputs.handoff_secret_name] }}",
      "PROTECTED_SERVICE_ID" => "${{ secrets.CMA_TRIGGER_SMOKE_SERVICE_ID }}",
    },
    "handoff acceptance must receive only its blob and exact UserService ID",
  )
  check(
    contains_in_order?(
      handoff.fetch("run"),
      [
        "{ set +x; } 2>/dev/null",
        'handoff="$CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_PATH"',
        'expected_handoff_secret_name="CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_${DISPATCH_NONCE^^}"',
        '[ "$HANDOFF_SECRET_NAME" != "$expected_handoff_secret_name" ]',
        '"$CMA_TRIGGER_KIND_STAGING_ROOT/credentials/nyxid-handoff.json"',
        'printf \'%s\' "$NYXID_BOOTSTRAP_B64" | base64 --decode > "$handoff"',
        "unset NYXID_BOOTSTRAP_B64",
        'chmod 0600 "$handoff"',
        'CMA_TRIGGER_SMOKE_SERVICE_ID="$PROTECTED_SERVICE_ID"',
        "cma-trigger-kind-staging.sh accept-handoff",
        "::add-mask::%s",
        "unset PROTECTED_SERVICE_ID",
      ],
    ),
    "handoff materialization, acceptance, masking, and release are not closed in order",
  )

  prepare = named_step(provider_steps, "Build and provision the ephemeral Kind provider lanes")
  deploy = named_step(provider_steps, "Deploy the reviewed CMA build into ephemeral staging")
  seal = named_step(provider_steps, "Seal all ephemeral staging bootstrap authority")
  check(prepare && deploy && seal, "provider smoke must prepare, deploy, and seal Kind staging")
  check(
    prepare.fetch("run") == ".github/scripts/cma-trigger-kind-staging.sh prepare" &&
      deploy.fetch("run") == ".github/scripts/cma-trigger-kind-staging.sh deploy" &&
      seal.fetch("run") == ".github/scripts/cma-trigger-kind-staging.sh seal",
    "ephemeral staging phases must invoke only their reviewed commands",
  )

  inventory = named_step(provider_steps, "Verify exact read-only BatchSandbox inventory authority")
  check(inventory, "provider smoke must verify its exact inventory authority")
  check(
    inventory.fetch("env") == {
      "KUBECONFIG_PATH" => "${{ runner.temp }}/cma-trigger-smoke-kubeconfig",
      "SANDBOX_NAMESPACE" => "${{ vars.CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE }}",
    },
    "inventory preflight must bind only the protected observer path and namespace",
  )
  inventory_run = inventory.fetch("run")
  namespace_guard = 'if [[ ! "$SANDBOX_NAMESPACE" =~ ^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$ ]]; then'
  check(inventory_run.include?(namespace_guard), "inventory namespace guard is absent")
  [
    "resource='batchsandboxes.sandbox.opensandbox.io'",
    'list "$resource" --namespace "$SANDBOX_NAMESPACE"',
    "for verb in get watch create update patch delete deletecollection",
    'list "$resource" --all-namespaces',
    "for resource in secrets configmaps pods",
    "get configmap/cma-trigger-runtime-provenance",
    "for verb in list watch create update patch delete",
    '"$verb" configmaps --namespace cma-trigger-system',
  ].each do |contract|
    check(inventory_run.include?(contract), "inventory authority is missing #{contract}")
  end

  fixture = named_step(provider_steps, "Publish the ephemeral Trigger fixture and export provenance")
  check(fixture, "provider smoke must publish an ephemeral Trigger fixture")
  check(
    contains_in_order?(
      fixture.fetch("run"),
      [
        "cma-trigger-kind-staging.sh bootstrap-fixture",
        "cma-trigger-kind-staging.sh export-env",
      ],
    ),
    "fixture publication must precede non-secret provenance export",
  )
  ordered_phases = [bootstrap, prepare, handoff, deploy, seal, inventory, fixture]
  ordered_phases.each_cons(2) do |before, after|
    check(
      provider_steps.index(before) < provider_steps.index(after),
      "ephemeral staging phase order drifted",
    )
  end

  materialize = named_step(provider_steps, "Materialize the non-secret pinned fixture contract")
  check(materialize, "provider smoke must materialize a reviewed fixture contract")
  check(
    contains_in_order?(
      materialize.fetch("run"),
      [
        '--arg build_sha "$CMA_TRIGGER_SMOKE_EXPECTED_BUILD_SHA"',
        '--arg runtime_image_digest "$CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_DIGEST"',
        '--argjson runtime_image_ids "$CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_IDS_JSON"',
        '--arg runtime_version "$CMA_TRIGGER_SMOKE_EXPECTED_RUNTIME_VERSION"',
        '--arg configuration_revision "$CMA_TRIGGER_SMOKE_CONFIGURATION_REVISION"',
        '--arg trigger_sandbox_namespace "$CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE"',
        "schema_version: 3",
        'cma_state_cleanup: "destroy_with_terminal_ephemeral_environment"',
        'session_cleanup: "stop_runtime_before_terminal_ephemeral_teardown"',
      ],
    ),
    "provider fixture must bind exported build, runtime, configuration, and namespace provenance",
  )

  run_smoke = named_step(provider_steps, "Run one exact provider path")
  check(run_smoke, "provider smoke must retain one exact writer path")
  check(
    run_smoke.fetch("env") == {
      "CONFIG_PATH" => "${{ runner.temp }}/cma-trigger-smoke-config.json",
      "EVIDENCE_PATH" => "${{ runner.temp }}/cma-trigger-smoke-evidence.json",
      "CMA_TRIGGER_SMOKE_KUBECONFIG" => "${{ runner.temp }}/cma-trigger-smoke-kubeconfig",
      "CMA_TRIGGER_SMOKE_OPERATOR_LOGIN" => "${{ steps.approval.outputs.operator_login }}",
      "CMA_TRIGGER_SMOKE_OPERATOR_ACCOUNT_ID" => "${{ steps.approval.outputs.operator_account_id }}",
      "CMA_TRIGGER_SMOKE_REVIEWERS_JSON" => "${{ steps.approval.outputs.reviewers_json }}",
    },
    "provider writer must bind only reviewed local paths and actors",
  )
  check(
    contains_in_order?(
      run_smoke.fetch("run"),
      [
        "{ set +x; } 2>/dev/null",
        'CMA_TRIGGER_SMOKE_ACCESS_TOKEN="$(<"$CMA_TRIGGER_KIND_STAGING_ROOT/credentials/caller-access-token")"',
        'CMA_TRIGGER_SMOKE_SERVICE_ID="$(<"$CMA_TRIGGER_KIND_STAGING_ROOT/credentials/github-service-id")"',
        '--run "$CONFIG_PATH" "$EVIDENCE_PATH"',
      ],
    ),
    "provider writer must read accepted caller custody only inside its process",
  )

  audit = named_step(provider_steps, "Audit and validate closed provider evidence")
  check(audit && audit["id"] == "evidence_audit", "provider evidence audit is absent")
  check(audit["if"].to_s.include?("always()"), "provider evidence audit must run after failure")
  check(
    audit.fetch("env") == {
      "EVIDENCE_PATH" => "${{ runner.temp }}/cma-trigger-smoke-evidence.json",
    },
    "provider evidence audit must receive only the evidence path",
  )
  check(
    contains_in_order?(
      audit.fetch("run"),
      [
        'CMA_TRIGGER_SMOKE_ACCESS_TOKEN="$(<"$CMA_TRIGGER_KIND_STAGING_ROOT/credentials/caller-access-token")"',
        'CMA_TRIGGER_SMOKE_SERVICE_ID="$(<"$CMA_TRIGGER_KIND_STAGING_ROOT/credentials/github-service-id")"',
        '--audit-evidence "$EVIDENCE_PATH"',
        '--validate-evidence "$EVIDENCE_PATH"',
        "evidence_id=$(jq -r '.evidence_id'",
        "evidence_result=$(jq -r '.result'",
        "artifact_name=$(jq -r '.provenance.workflow.artifact_name'",
      ],
    ),
    "provider evidence must be audited before its outputs are exposed",
  )

  cleanup = named_step(
    provider_steps,
    "Destroy staging, settle provider objects, and purge runner custody",
  )
  check(cleanup && cleanup["id"] == "staging_cleanup", "terminal staging cleanup is absent")
  check(cleanup["if"].to_s.include?("always()"), "staging cleanup must run after every outcome")
  check(
    cleanup.fetch("env") == {
      "EVIDENCE_AUDIT_OUTCOME" => "${{ steps.evidence_audit.outcome }}",
      "EVIDENCE_PATH" => "${{ runner.temp }}/cma-trigger-smoke-evidence.json",
      "SETTLEMENT_RECEIPT_PATH" => "${{ runner.temp }}/cma-trigger-provider-object-settlement.json",
    },
    "terminal staging cleanup must receive only the audit outcome and fixed redacted paths",
  )
  check(
    contains_in_order?(
      cleanup.fetch("run"),
      [
        'if [ -f "$CMA_TRIGGER_KIND_STAGING_ROOT/ownership.json" ]',
        "cma-trigger-kind-staging.sh destroy",
        '[ -f "$CMA_TRIGGER_KIND_STAGING_ROOT/phase-destroyed" ]',
        'evidence_result=$(jq -er \'.result\' "$EVIDENCE_PATH")',
        '[ "${evidence_result:-}" = "pass" ]',
        "cma-trigger-provider-object-settlement.mjs settle",
        '--handoff "$CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_PATH"',
        '--evidence "$EVIDENCE_PATH"',
        '--output "$SETTLEMENT_RECEIPT_PATH"',
        "cma-trigger-kind-staging.sh remove-observer",
        "cma-trigger-kind-staging.sh purge",
        "cma-trigger-kind-staging.sh verify-cleanup",
        'exit "$status"',
      ],
    ),
    "staging cleanup must prove destruction, settle passing provider objects, then remove observer custody and purge",
  )

  finalize = named_step(provider_steps, "Finalize evidence against terminal ephemeral cleanup")
  check(finalize && finalize["id"] == "evidence_finalize", "terminal evidence finalizer is absent")
  check(
    finalize["if"].to_s.include?("always()") &&
      finalize["if"].to_s.include?("steps.evidence_audit.outcome == 'success'") &&
      finalize["if"].to_s.include?("steps.staging_cleanup.outcome == 'success'"),
    "evidence finalization must require its audit and terminal cleanup receipt",
  )
  check(
    finalize.fetch("env") == {
      "EVIDENCE_PATH" => "${{ runner.temp }}/cma-trigger-smoke-evidence.json",
      "CLEANUP_RECEIPT_PATH" => "${{ runner.temp }}/cma-trigger-kind-staging-cleanup.json",
    },
    "evidence finalizer must receive only its evidence and cleanup receipt paths",
  )
  check(
    contains_in_order?(
      finalize.fetch("run"),
      [
        '--finalize-ephemeral-cleanup "$EVIDENCE_PATH" "$CLEANUP_RECEIPT_PATH"',
        '--validate-evidence "$EVIDENCE_PATH"',
      ],
    ),
    "terminal evidence must be finalized from the cleanup receipt and revalidated",
  )

  upload = named_step(provider_steps, "Upload redacted provider evidence")
  check(upload && upload["id"] == "evidence_upload", "provider evidence upload is absent")
  check(
    upload["if"].to_s.include?("always()") &&
      upload["if"].to_s.include?("steps.evidence_audit.outcome == 'success'") &&
      upload["if"].to_s.include?("steps.staging_cleanup.outcome == 'success'") &&
      upload["if"].to_s.include?("steps.evidence_finalize.outcome == 'success'") &&
      upload.fetch("with").fetch("if-no-files-found") == "error",
    "provider evidence upload must require successful audit, cleanup, and finalization",
  )
  check(
    provider_steps.index(audit) < provider_steps.index(cleanup) &&
      provider_steps.index(cleanup) < provider_steps.index(finalize) &&
      provider_steps.index(finalize) < provider_steps.index(upload),
    "evidence must be audited, cleaned, finalized, then uploaded in order",
  )

  protected_bindings = provider_steps.flat_map do |step|
    next [] unless step.is_a?(Hash) && step["env"].is_a?(Hash)

    step["env"].values.grep(/\A\$\{\{ secrets(?:\.|\[)/)
  end.sort
  check(
    protected_bindings == [
      "${{ secrets.CMA_TRIGGER_SMOKE_KIND_CA_KEY_B64 }}",
      "${{ secrets.CMA_TRIGGER_SMOKE_KUBECONFIG_B64 }}",
      "${{ secrets[inputs.handoff_secret_name] }}",
      "${{ secrets.CMA_TRIGGER_SMOKE_SERVICE_ID }}",
    ].sort,
    "provider secrets escaped the two reviewed materialization steps",
  )
  check(
    !JSON.generate(workflow).include?("secrets.CMA_TRIGGER_SMOKE_ACCESS_TOKEN"),
    "provider smoke must not retain a separately uploaded caller-token secret",
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
    "schema_version: 6",
    "expected_kernel: TRIGGER_KERNEL_ID",
    "observed_kernel: null",
    "deployment.expected_kernel !== TRIGGER_KERNEL_ID",
    "deployment.observed_kernel !== deployment.expected_kernel",
    "sessionCleanupStages.length !== 1",
    'stage.name === "session_cleanup"',
    "raw.stages[raw.stages.length - 1] !== sessionCleanupStages[0]",
    "CMA_STATE_CLEANUP_POLICY",
    "SESSION_CLEANUP_POLICY",
    "SESSION_IDLE_AWAITING_TEARDOWN",
    "EPHEMERAL_STATE_DESTROYED",
    "finalizeEphemeralCleanupEvidence",
    'mode === "--finalize-ephemeral-cleanup"',
    'const BATCH_SANDBOX_RESOURCE = "batchsandboxes.sandbox.opensandbox.io"',
    '"trigger_sandbox_namespace"',
    '"CMA_TRIGGER_SMOKE_KUBECONFIG"',
    '"kubernetes_credentials"',
    '"runtime_reservation_sha256"',
    '"runtime_image_ids"',
    '"runtime_attestation"',
    'currentStage = "runtime_image_attestation"',
    "waitForRuntimeAttestation",
    'const RUNTIME_ATTESTATION_CONFIGMAP = "cma-trigger-runtime-provenance"',
    'const STORAGE_CONTRACT_REPOSITORY = "ChronoAIProject/chrono-bucket"',
    '"provider_object_settlement"',
    "validateProviderObjectSettlement",
    "context.unrelated_live_objects_observed !== 0",
    "workspace.unrelated_live_objects_observed !== 0",
    "value.session_id_sha256 !== sha256(evidence.resources.session_id)",
    "value.current_run_live_objects_after !== 0",
    "receipt.schema_version !== 2",
    '"--chunk-size=0"',
  ].each do |contract|
    check(smoke.include?(contract), "provider smoke source is missing #{contract}")
  end

  settlement = sources.fetch(:settlement)
  [
    'const STORAGE_CONTRACT_REPOSITORY = "ChronoAIProject/chrono-bucket"',
    '"4087662e2a65e3747b3203ba619eca7342089272"',
    "raw.schema_version !== 6",
    '"storage_owner_service_account_id"',
    "ownerServiceAccountId: handoff.storage_owner_service_account_id",
    "subjectHash: sha256(handoff.subject_id)",
    'url.searchParams.set("mutationGeneration", String(object.generation + 1))',
    "validateDeleteAcknowledgement",
    "exactObjectSnapshot(contextClassified.unrelated)",
    'fsConstants.O_CREAT |',
    'fsConstants.O_EXCL |',
    "await unlink(outputPath)",
  ].each do |contract|
    check(
      settlement.include?(contract),
      "provider-object settlement source is missing #{contract}",
    )
  end
  settlement_tests = sources.fetch(:settlement_tests)
  [
    "same Launch under another valid subject hash",
    "bucket owner drift aborts before the first delete",
    "fenced generation drift",
    "an ambiguous delete transport failure never produces a receipt",
    "a residual current-run live object after reread fails settlement",
    "unrelated inventory drift during settlement is detected",
  ].each do |contract|
    check(
      settlement_tests.include?(contract),
      "provider-object settlement tests are missing #{contract}",
    )
  end
  check(
    settlement.scan("sessionId: evidence.resources.session_id").length == 4 &&
      settlement.scan("launchId: evidence.resources.launch_id").length == 4 &&
      settlement.scan("subjectHash: sha256(handoff.subject_id)").length == 2,
    "provider-object settlement must bind both initial and final inventories to the exact session, Launch, and subject",
  )
  check(
    !smoke.include?("stop_runtime_preserve_session"),
    "provider smoke must not claim ephemeral session preservation",
  )
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
    /currentStage = "runtime_image_attestation".*?evidence\.document\.runtime_attestation\s*=\s*await waitForRuntimeAttestation\(.*?sandboxId: liveSandbox\.labels\.sandbox_id/m,
    "provider smoke must bind the exact live sandbox to runtime image attestation",
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
  verify_ci_image_provenance(inputs.fetch(:ci_workflow))
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

  pr_image_export = deep_copy(base)
  named_step(
    pr_image_export.dig(:ci_workflow, "jobs", "images", "steps"),
    "Export scanned image provenance",
  )["if"] = "${{ success() && (matrix.component == 'api' || matrix.component == 'runtime') }}"
  mutations << ["CI image provenance exported for pull requests", pr_image_export]

  missing_ci_source_binding = deep_copy(base)
  ci_export = named_step(
    missing_ci_source_binding.dig(:ci_workflow, "jobs", "images", "steps"),
    "Export scanned image provenance",
  )
  replaced = ci_export.fetch("run").sub!(
    ".source_sha == $source_sha",
    ".source_sha != null",
  )
  check(replaced, "CI source-binding mutation did not change the fixture")
  mutations << ["CI image metadata missing exact source SHA binding", missing_ci_source_binding]

  export_before_scan = deep_copy(base)
  ci_steps = export_before_scan.dig(:ci_workflow, "jobs", "images", "steps")
  scan_step = named_step(ci_steps, "Vulnerability scan")
  export_step = named_step(ci_steps, "Export scanned image provenance")
  ci_steps.delete(scan_step)
  ci_steps.insert(ci_steps.index(export_step) + 1, scan_step)
  mutations << ["CI image exported before vulnerability scan", export_before_scan]

  floated_ci_provenance_action = deep_copy(base)
  named_step(
    floated_ci_provenance_action.dig(:ci_workflow, "jobs", "images", "steps"),
    "Upload scanned image provenance",
  )["uses"] = "actions/upload-artifact@v4"
  mutations << ["floated CI image provenance action", floated_ci_provenance_action]

  weakened_ci_run_resolution = deep_copy(base)
  ci_resolution = named_step(
    weakened_ci_run_resolution.dig(:workflow, "jobs", "plan", "steps"),
    "Resolve exact successful CI image artifacts",
  )
  replaced = ci_resolution.fetch("run").gsub!(
    '.conclusion == "success"',
    ".conclusion != null",
  )
  check(replaced, "successful-CI-run mutation did not change the fixture")
  mutations << ["CI run resolution accepts unsuccessful runs", weakened_ci_run_resolution]

  name_selected_artifact = deep_copy(base)
  api_download = named_step(
    name_selected_artifact.dig(:workflow, "jobs", "provider", "steps"),
    "Download exact approved API image artifact",
  )
  api_download.fetch("with").delete("artifact-ids")
  api_download.fetch("with")["name"] = "cma-trigger-ci-image-api-${{ github.sha }}"
  mutations << ["protected image artifact selected by name", name_selected_artifact]

  missing_artifact_digest = deep_copy(base)
  missing_artifact_digest.dig(:workflow, "jobs", "plan", "outputs")
    .delete("api_artifact_digest")
  mutations << ["approved image provenance omits artifact digest", missing_artifact_digest]

  missing_archive_hash_check = deep_copy(base)
  image_verification = named_step(
    missing_archive_hash_check.dig(:workflow, "jobs", "provider", "steps"),
    "Verify approved image bytes and expose staging contract",
  )
  replaced = image_verification.fetch("run").sub!(
    'actual_archive_sha256=$(sha256sum "$archive_path" | awk \'{print $1}\')',
    'actual_archive_sha256="$approved_archive_sha256"',
  )
  check(replaced, "archive-hash mutation did not change the fixture")
  mutations << ["protected image archive hash verification removed", missing_archive_hash_check]

  open_approved_manifest = deep_copy(base)
  image_verification = named_step(
    open_approved_manifest.dig(:workflow, "jobs", "provider", "steps"),
    "Verify approved image bytes and expose staging contract",
  )
  replaced = image_verification.fetch("run").sub!(
    'keys == ["ci_run", "images", "kind", "repository_id", "schema_version", "source_sha"]',
    'has("ci_run") and has("images")',
  )
  check(replaced, "closed-manifest mutation did not change the fixture")
  mutations << ["approved image manifest schema opened", open_approved_manifest]

  missing_manifest_field = deep_copy(base)
  image_verification = named_step(
    missing_manifest_field.dig(:workflow, "jobs", "provider", "steps"),
    "Verify approved image bytes and expose staging contract",
  )
  replaced = image_verification.fetch("run").sub!(
    '"image_reference", "metadata_name", "metadata_sha256"',
    '"image_reference", "metadata_name"',
  )
  check(replaced, "manifest-field mutation did not change the fixture")
  mutations << ["approved image manifest schema field removed", missing_manifest_field]

  image_verification_after_secret = deep_copy(base)
  provider_steps = image_verification_after_secret.dig(:workflow, "jobs", "provider", "steps")
  verification_step = named_step(provider_steps, "Verify approved image bytes and expose staging contract")
  bootstrap_step = named_step(provider_steps, "Validate the protected ephemeral staging bootstrap")
  provider_steps.delete(verification_step)
  provider_steps.insert(provider_steps.index(bootstrap_step) + 1, verification_step)
  mutations << ["image verification moved after protected credentials", image_verification_after_secret]

  missing_staging_image_id = deep_copy(base)
  image_verification = named_step(
    missing_staging_image_id.dig(:workflow, "jobs", "provider", "steps"),
    "Verify approved image bytes and expose staging contract",
  )
  removed = image_verification.fetch("run").sub!(
    /^\s*echo "CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ID=.*\n/,
    "",
  )
  check(removed, "staging image-ID contract mutation did not change the fixture")
  mutations << ["staging runtime image ID contract removed", missing_staging_image_id]

  floated_download_action = deep_copy(base)
  named_step(
    floated_download_action.dig(:workflow, "jobs", "provider", "steps"),
    "Download exact approved image provenance manifest",
  )["uses"] = "actions/download-artifact@v4"
  mutations << ["floated approved artifact download action", floated_download_action]

  missing_dispatch_nonce = deep_copy(base)
  missing_dispatch_nonce.dig(:workflow, true, "workflow_dispatch", "inputs")
    .delete("dispatch_nonce")
  mutations << ["missing dispatch nonce", missing_dispatch_nonce]

  static_handoff_secret = deep_copy(base)
  named_step(
    static_handoff_secret.dig(:workflow, "jobs", "provider", "steps"),
    "Materialize and accept the closed NyxID handoff",
  ).fetch("env")["NYXID_BOOTSTRAP_B64"] =
    "${{ secrets.CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_B64 }}"
  mutations << ["static shared handoff secret", static_handoff_secret]

  missing_expected_head = deep_copy(base)
  missing_expected_head.dig(:workflow, true, "workflow_dispatch", "inputs")
    .delete("expected_head_sha")
  mutations << ["missing reviewed dispatch head", missing_expected_head]

  missing_head_equality = deep_copy(base)
  head_guard = named_step(
    missing_head_equality.dig(:workflow, "jobs", "plan", "steps"),
    "Require the deployed develop workflow and preservation consent",
  )
  replaced = head_guard.fetch("run").sub!(
    'if [ "$WORKFLOW_SHA" != "$EXPECTED_HEAD_SHA" ]; then',
    'if [ "$WORKFLOW_SHA" != "$WORKFLOW_SHA" ]; then',
  )
  check(replaced, "reviewed-head equality mutation did not change the fixture")
  mutations << ["missing reviewed dispatch head equality", missing_head_equality]

  wrong_plan_reviewer = deep_copy(base)
  plan_environment = named_step(
    wrong_plan_reviewer.dig(:workflow, "jobs", "plan", "steps"),
    "Require the live protected smoke environment",
  )
  replaced = plan_environment.fetch("run").sub!(
    ".reviewers[0].reviewer.id == 15748251",
    ".reviewers[0].reviewer.id == 1",
  )
  check(replaced, "plan reviewer-identity mutation did not change the fixture")
  mutations << ["wrong pre-approval reviewer identity", wrong_plan_reviewer]

  weakened_plan_rule_set = deep_copy(base)
  plan_environment = named_step(
    weakened_plan_rule_set.dig(:workflow, "jobs", "plan", "steps"),
    "Require the live protected smoke environment",
  )
  replaced = plan_environment.fetch("run").sub!(
    ".protection_rules | length == 2",
    ".protection_rules | length >= 2",
  )
  check(replaced, "plan protection-rule mutation did not change the fixture")
  mutations << ["open pre-approval protection rule set", weakened_plan_rule_set]

  wrong_post_approval_reviewer = deep_copy(base)
  provider_environment = named_step(
    wrong_post_approval_reviewer.dig(:workflow, "jobs", "provider", "steps"),
    "Revalidate the protected smoke environment after approval",
  )
  replaced = provider_environment.fetch("run").sub!(
    '.reviewers[0].reviewer.slug == "ai-application"',
    '.reviewers[0].reviewer.slug == "some-other-team"',
  )
  check(replaced, "post-approval reviewer-identity mutation did not change the fixture")
  mutations << ["wrong post-approval reviewer identity", wrong_post_approval_reviewer]

  weakened_post_approval_branch_rule = deep_copy(base)
  provider_environment = named_step(
    weakened_post_approval_branch_rule.dig(:workflow, "jobs", "provider", "steps"),
    "Revalidate the protected smoke environment after approval",
  )
  replaced = provider_environment.fetch("run").sub!(
    '[.protection_rules[] | select(.type == "branch_policy")] | length == 1',
    '[.protection_rules[] | select(.type == "branch_policy")] | length > 0',
  )
  check(replaced, "post-approval branch-rule mutation did not change the fixture")
  mutations << ["open post-approval branch rule set", weakened_post_approval_branch_rule]

  missing_example_namespace = deep_copy(base)
  missing_example_namespace.fetch(:example).delete("trigger_sandbox_namespace")
  mutations << ["missing example sandbox namespace", missing_example_namespace]

  extended_example_schema = deep_copy(base)
  extended_example_schema.fetch(:example)["kubeconfig"] = "forbidden"
  mutations << ["extended example schema", extended_example_schema]

  floated_plan_action = deep_copy(base)
  floated_checkout = floated_plan_action.dig(:workflow, "jobs", "plan", "steps")
    .find { |step| step.is_a?(Hash) && step["uses"]&.start_with?("actions/checkout@") }
  floated_checkout["uses"] = "actions/checkout@v5"
  mutations << ["floated plan action", floated_plan_action]

  floated_provider_action = deep_copy(base)
  floated_upload = floated_provider_action.dig(:workflow, "jobs", "provider", "steps")
    .find { |step| step.is_a?(Hash) && step["uses"]&.start_with?("actions/upload-artifact@") }
  floated_upload["uses"] = "actions/upload-artifact@v4"
  mutations << ["floated credential-job action", floated_provider_action]

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
    "Validate the protected ephemeral staging bootstrap",
  ).fetch("env")["CMA_TRIGGER_SMOKE_KUBECONFIG_B64"] = "${{ secrets.OTHER_KUBECONFIG }}"
  mutations << ["wrong kubeconfig secret binding", wrong_kubeconfig_secret]

  missing_kubeconfig_digest = deep_copy(base)
  named_step(
    missing_kubeconfig_digest.dig(:workflow, "jobs", "provider", "steps"),
    "Validate the protected ephemeral staging bootstrap",
  ).fetch("env").delete("CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256")
  mutations << ["missing kubeconfig digest binding", missing_kubeconfig_digest]

  wrong_handoff_secret = deep_copy(base)
  named_step(
    wrong_handoff_secret.dig(:workflow, "jobs", "provider", "steps"),
    "Materialize and accept the closed NyxID handoff",
  ).fetch("env")["NYXID_BOOTSTRAP_B64"] = "${{ secrets.OTHER_HANDOFF }}"
  mutations << ["wrong NyxID handoff secret binding", wrong_handoff_secret]

  skipped_handoff_acceptance = deep_copy(base)
  handoff_step = named_step(
    skipped_handoff_acceptance.dig(:workflow, "jobs", "provider", "steps"),
    "Materialize and accept the closed NyxID handoff",
  )
  handoff_step["run"] = handoff_step.fetch("run").sub(
    "cma-trigger-kind-staging.sh accept-handoff",
    "cma-trigger-kind-staging.sh prepare",
  )
  mutations << ["skipped NyxID handoff acceptance", skipped_handoff_acceptance]

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

  skipped_terminal_cleanup = deep_copy(base)
  named_step(
    skipped_terminal_cleanup.dig(:workflow, "jobs", "provider", "steps"),
    "Destroy staging, settle provider objects, and purge runner custody",
  )["if"] = "success()"
  mutations << ["non-always terminal staging cleanup", skipped_terminal_cleanup]

  purge_without_destroy = deep_copy(base)
  cleanup_step = named_step(
    purge_without_destroy.dig(:workflow, "jobs", "provider", "steps"),
    "Destroy staging, settle provider objects, and purge runner custody",
  )
  cleanup_step["run"] = cleanup_step.fetch("run").sub(
    "cma-trigger-kind-staging.sh destroy",
    "cma-trigger-kind-staging.sh verify-cleanup",
  )
  mutations << ["staging purge without destruction", purge_without_destroy]

  missing_provider_object_settlement = deep_copy(base)
  cleanup_step = named_step(
    missing_provider_object_settlement.dig(:workflow, "jobs", "provider", "steps"),
    "Destroy staging, settle provider objects, and purge runner custody",
  )
  replaced = cleanup_step.fetch("run").sub!(
    "cma-trigger-provider-object-settlement.mjs settle",
    "provider-object-settlement-disabled",
  )
  check(replaced, "settlement-omission mutation did not change the fixture")
  mutations << ["provider-object settlement omitted", missing_provider_object_settlement]

  settlement_before_destroy = deep_copy(base)
  cleanup_step = named_step(
    settlement_before_destroy.dig(:workflow, "jobs", "provider", "steps"),
    "Destroy staging, settle provider objects, and purge runner custody",
  )
  run = cleanup_step.fetch("run")
  replaced = run.sub!(
    "cma-trigger-provider-object-settlement.mjs settle",
    "provider-object-settlement-disabled",
  )
  check(replaced, "pre-destroy settlement mutation did not disable the original call")
  replaced = run.sub!(
    ".github/scripts/cma-trigger-kind-staging.sh destroy || status=1",
    ".github/scripts/cma-trigger-provider-object-settlement.mjs settle\n            .github/scripts/cma-trigger-kind-staging.sh destroy || status=1",
  )
  check(replaced, "pre-destroy settlement mutation did not change the fixture")
  mutations << ["provider-object settlement before destruction", settlement_before_destroy]

  purge_before_settlement = deep_copy(base)
  cleanup_step = named_step(
    purge_before_settlement.dig(:workflow, "jobs", "provider", "steps"),
    "Destroy staging, settle provider objects, and purge runner custody",
  )
  run = cleanup_step.fetch("run")
  replaced = run.sub!(
    ".github/scripts/cma-trigger-kind-staging.sh purge || status=1",
    "true # purge deferred mutation",
  )
  check(replaced, "pre-settlement purge mutation did not disable the original call")
  replaced = run.sub!(
    "node .github/scripts/cma-trigger-provider-object-settlement.mjs settle",
    ".github/scripts/cma-trigger-kind-staging.sh purge || status=1\n                node .github/scripts/cma-trigger-provider-object-settlement.mjs settle",
  )
  check(replaced, "pre-settlement purge mutation did not change the fixture")
  mutations << ["runner purge before provider-object settlement", purge_before_settlement]

  wrong_settlement_generation = deep_copy(base)
  replaced = wrong_settlement_generation.dig(:sources, :settlement).sub!(
    'url.searchParams.set("mutationGeneration", String(object.generation + 1))',
    'url.searchParams.set("mutationGeneration", String(object.generation))',
  )
  check(replaced, "settlement-generation mutation did not change the fixture")
  mutations << ["provider-object settlement reuses the live generation", wrong_settlement_generation]

  wrong_settlement_owner = deep_copy(base)
  replaced = wrong_settlement_owner.dig(:sources, :settlement).sub!(
    "ownerServiceAccountId: handoff.storage_owner_service_account_id",
    "ownerServiceAccountId: handoff.subject_id",
  )
  check(replaced, "settlement-owner mutation did not change the fixture")
  mutations << ["provider-object settlement drops bucket-owner binding", wrong_settlement_owner]

  wrong_settlement_session = deep_copy(base)
  replaced = wrong_settlement_session.dig(:sources, :settlement).sub!(
    "sessionId: evidence.resources.session_id",
    "sessionId: evidence.resources.launch_id",
  )
  check(replaced, "settlement-session mutation did not change the fixture")
  mutations << ["provider-object settlement drops session binding", wrong_settlement_session]

  wrong_settlement_launch = deep_copy(base)
  replaced = wrong_settlement_launch.dig(:sources, :settlement).sub!(
    "launchId: evidence.resources.launch_id",
    "launchId: evidence.resources.session_id",
  )
  check(replaced, "settlement-Launch mutation did not change the fixture")
  mutations << ["provider-object settlement drops Launch binding", wrong_settlement_launch]

  settlement_allows_unrelated_objects = deep_copy(base)
  replaced = settlement_allows_unrelated_objects.dig(:sources, :smoke).sub!(
    "context.unrelated_live_objects_observed !== 0",
    "false",
  )
  check(replaced, "unrelated-object mutation did not change the fixture")
  mutations << ["provider-object settlement admits unrelated live objects", settlement_allows_unrelated_objects]

  settlement_allows_residual_objects = deep_copy(base)
  replaced = settlement_allows_residual_objects.dig(:sources, :smoke).sub!(
    "value.current_run_live_objects_after !== 0",
    "false",
  )
  check(replaced, "residual-object mutation did not change the fixture")
  mutations << ["provider-object settlement admits residual live objects", settlement_allows_residual_objects]

  missing_evidence_finalization = deep_copy(base)
  provider_steps = missing_evidence_finalization.dig(:workflow, "jobs", "provider", "steps")
  provider_steps.reject! do |step|
    step.is_a?(Hash) && step["name"] == "Finalize evidence against terminal ephemeral cleanup"
  end
  mutations << ["missing terminal evidence finalization", missing_evidence_finalization]

  upload_before_finalization = deep_copy(base)
  named_step(
    upload_before_finalization.dig(:workflow, "jobs", "provider", "steps"),
    "Upload redacted provider evidence",
  )["if"] = "${{ always() && steps.evidence_audit.outcome == 'success' && steps.staging_cleanup.outcome == 'success' }}"
  mutations << ["upload without terminal evidence finalization", upload_before_finalization]

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

#!/usr/bin/env ruby

require "yaml"

ROOT = File.expand_path("../..", __dir__)
WORKFLOW_PATH = File.join(ROOT, ".github/workflows/cma-trigger-provider-smoke.yml")

def check(condition, message)
  raise message unless condition
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

workflow = YAML.safe_load(File.read(WORKFLOW_PATH), aliases: true)
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
consent = named_step(plan_steps, "Require the deployed develop workflow and preservation consent")
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
{
  "CMA_TRIGGER_SMOKE_EXPECTED_BUILD_SHA" => "${{ vars.CMA_TRIGGER_SMOKE_EXPECTED_BUILD_SHA }}",
  "CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_DIGEST" => "${{ vars.CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_DIGEST }}",
  "CMA_TRIGGER_SMOKE_EXPECTED_RUNTIME_VERSION" => "${{ vars.CMA_TRIGGER_SMOKE_EXPECTED_RUNTIME_VERSION }}",
  "CMA_TRIGGER_SMOKE_CONFIGURATION_REVISION" => "${{ vars.CMA_TRIGGER_SMOKE_CONFIGURATION_REVISION }}",
}.each do |name, binding|
  check(provider_env[name] == binding, "provider smoke must bind #{name} from its protected environment")
end
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
      'schema_version: 2',
      'expected_build_sha: $build_sha',
      'runtime_image_digest: $runtime_image_digest',
      'expected_runtime_version: $runtime_version',
      'configuration_revision: $configuration_revision',
    ],
  ),
  "provider fixture must bind the exact build, runtime digest/version, and configuration revision",
)

run_smoke = named_step(provider_steps, "Run one exact provider path")
check(run_smoke, "provider smoke must retain its one exact writer path")
run_env = run_smoke.fetch("env")
{
  "CMA_TRIGGER_SMOKE_OPERATOR_LOGIN" => "${{ steps.approval.outputs.operator_login }}",
  "CMA_TRIGGER_SMOKE_OPERATOR_ACCOUNT_ID" => "${{ steps.approval.outputs.operator_account_id }}",
  "CMA_TRIGGER_SMOKE_REVIEWERS_JSON" => "${{ steps.approval.outputs.reviewers_json }}",
}.each do |name, binding|
  check(run_env[name] == binding, "provider writer must bind reviewed #{name}")
end

audit = named_step(provider_steps, "Audit and validate closed provider evidence")
check(audit, "provider smoke must always audit its closed evidence")
check(audit["id"] == "evidence_audit", "provider evidence audit must retain its stable step id")
check(audit["if"].to_s.include?("always()"), "provider evidence audit must run after failure")
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
upload_with = upload.fetch("with")
check(
  upload_with == {
    "name" => "${{ steps.evidence_audit.outputs.artifact_name }}",
    "path" => "${{ runner.temp }}/cma-trigger-smoke-evidence.json",
    "if-no-files-found" => "error",
    "retention-days" => 30,
  },
  "provider evidence upload must retain only its validated exact file for 30 days",
)

puts "CMA Trigger provider smoke contract passed"

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
provider_recheck = named_step(
  provider.fetch("steps"),
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

puts "CMA Trigger provider smoke contract passed"

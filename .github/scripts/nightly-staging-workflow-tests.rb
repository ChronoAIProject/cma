#!/usr/bin/env ruby
require 'yaml'


def require_check(value, message)
  raise message unless value
end

def validate(workflow, repo)
  events = workflow.fetch('on', workflow[true])
  require_check(events['schedule'] == [{'cron' => '17 19 * * *'}], 'normal nightly schedule required')
  require_check(workflow['concurrency'] == {'group' => 'nightly-staging', 'cancel-in-progress' => false},
    'nightly evidence must not be cancelled by a later run')
  smoke = workflow.fetch('jobs').fetch('smoke')
  require_check(smoke['runs-on'] == 'ubuntu-24.04', 'pinned browser requires the supported Ubuntu 24.04 runner')
  require_check(!smoke.key?('if') && !smoke.key?('continue-on-error') && smoke['timeout-minutes'] == 15,
    'smoke must run with the existing timeout and propagate failures')
  require_check(smoke['defaults'] == {'run' => {'working-directory' => 'web'}}, 'smoke shell and workspace differ')
  require_check(smoke.dig('env', 'CMA_E2E_MODE') == 'smoke', 'nightly smoke must remain read-only')
  steps = smoke.fetch('steps')
  node = steps.index { |s| s['name'] == 'Use supported Node LTS for the pinned browser harness' }
  install = steps.index { |s| s['name'] == 'Install pinned browser' }
  harness = steps.index { |s| s['id'] == 'harness' }
  require_check(node && install && harness && node < install && install < harness, 'browser setup must precede smoke')
  require_check(steps[node].fetch('uses', '').start_with?('actions/setup-node@') &&
    steps[node].dig('with', 'node-version') == '24.15.0', 'supported browser Node version differs')
  require_check(steps[install]['run'] == 'npx --no-install playwright install --with-deps chromium',
    'pinned browser and system dependencies must install successfully')
  require_check(steps[harness]['run'] == 'npm run e2e -- --project=smoke', 'GET-only smoke invocation differs')
  [node, install, harness].each do |index|
    require_check((steps[index].keys & %w[if continue-on-error shell working-directory]).empty?,
      'browser setup and smoke cannot skip or tolerate failures')
  end
  structure = repo.fetch('jobs').fetch('structure')
  require_check(!structure.key?('if') && !structure.key?('continue-on-error'), 'Repository structure must be required')
  checks = structure.fetch('steps').select { |s| s['name'] == 'Nightly smoke workflow regressions' }
  require_check(checks.size == 1 && checks.first['run'] == 'ruby .github/scripts/nightly-staging-workflow-tests.rb' &&
    (checks.first.keys & %w[if continue-on-error shell]).empty?, 'nightly workflow contract must run in Repository CI')
  evidence = structure.fetch('steps').select { |s| s['name'] == 'Synthetic smoke evidence regressions' }
  require_check(evidence.size == 1 && evidence.first['run'] == 'node --test .github/scripts/smoke-evidence/*.test.mjs' &&
    (evidence.first.keys & %w[if continue-on-error shell working-directory]).empty?, 'smoke evidence regressions must run in Repository CI')
end

root = File.expand_path('../..', __dir__)
workflow = YAML.load_file(File.join(root, '.github/workflows/nightly-staging.yml'))
repo = YAML.load_file(File.join(root, '.github/workflows/ci-repo.yml'))
validate(workflow, repo)
mutations = [
  ->(w, _) { w['jobs']['smoke']['runs-on'] = 'ubuntu-latest' },
  ->(w, _) { w[true].delete('schedule') },
  ->(w, _) { w['concurrency']['cancel-in-progress'] = true },
  ->(w, _) { w['jobs']['smoke']['if'] = 'false' },
  ->(w, _) { w['jobs']['smoke']['continue-on-error'] = true },
  ->(w, _) { w['jobs']['smoke']['timeout-minutes'] = 1 },
  ->(w, _) { w['jobs']['smoke']['env']['CMA_E2E_MODE'] = 'mutation' },
  ->(w, _) { w['jobs']['smoke']['defaults']['run']['shell'] = 'bash {0} || true' },
  ->(_, r) { r['jobs']['structure']['steps'].reject! { |s| s['name'] == 'Nightly smoke workflow regressions' } },
  ->(_, r) { r['jobs']['structure']['steps'].reject! { |s| s['name'] == 'Synthetic smoke evidence regressions' } },
  ->(_, r) { r['jobs']['structure']['if'] = 'false' },
]
install_index = workflow['jobs']['smoke']['steps'].index { |s| s['name'] == 'Install pinned browser' }
mutations << ->(w, _) { w['jobs']['smoke']['steps'].delete_at(install_index) }
['true', 'npx --no-install playwright install chromium',
 'npx --no-install playwright install --with-deps chromium || true'].each do |command|
  mutations << ->(w, _) { w['jobs']['smoke']['steps'][install_index]['run'] = command }
end
[{'if' => 'false'}, {'continue-on-error' => true}, {'shell' => 'bash {0} || true'}].each do |bypass|
  mutations << ->(w, _) { w['jobs']['smoke']['steps'][install_index].merge!(bypass) }
  mutations << ->(_, r) { r['jobs']['structure']['steps'].find { |s| s['name'] == 'Nightly smoke workflow regressions' }.merge!(bypass) }
  mutations << ->(_, r) { r['jobs']['structure']['steps'].find { |s| s['name'] == 'Synthetic smoke evidence regressions' }.merge!(bypass) }
end
mutations.each do |mutation|
  changed_workflow, changed_repo = Marshal.load(Marshal.dump([workflow, repo]))
  mutation.call(changed_workflow, changed_repo)
  begin
    validate(changed_workflow, changed_repo)
  rescue RuntimeError
    next
  end
  abort 'nightly workflow regression accepted'
end
puts "Nightly runner, browser installation and required CI contracts passed (#{mutations.size} mutations)"

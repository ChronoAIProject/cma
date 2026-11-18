#!/usr/bin/env ruby
require 'yaml'
require 'json'
root = File.expand_path('../..', __dir__)


def require_check(value, message)
  raise message unless value
end

def validate(documents)
  ci = documents.fetch('ci')
  events = ci.fetch('on', ci[true])
  require_check(events.keys == ['pull_request'], 'CI must be PR-only')
  pr = events.fetch('pull_request')
  require_check(pr.fetch('branches') == ['develop'], 'CI target differs')
  require_check(pr.fetch('types').sort == %w[opened reopened synchronize edited labeled unlabeled].sort, 'label event coverage differs')
  require_check(!pr.key?('paths') && !pr.key?('paths-ignore'), 'required checks must always be created')
  jobs = ci.fetch('jobs')
  require_check(jobs.keys.none? { |key| key.start_with?('trigger-') }, 'Trigger job retained')
  required = {
    'gate-repo' => ['CI / Repo', %w[changes repo version-label]],
    'gate-am' => ['CI / Agent Manager', %w[changes rust-test-build am am-images runtime-images omarchy]],
    'gate-eg' => ['CI / Event Gateway', %w[changes rust-test-build eg eg-images runtime-images omarchy]],
    'gate-web' => ['CI / Web', %w[changes web web-images]]
  }
  required.each do |key, (name, dependencies)|
    job = jobs.fetch(key)
    require_check(job.fetch('name') == name && job.fetch('if') == 'always()', 'stable gate missing or conditional')
    require_check(job.fetch('needs').sort == dependencies.sort, 'gate dependency omitted')
    require_check(job.fetch('steps').any? { |step| step.fetch('run', '').include?('ci_tracks/gate.py') }, 'gate validator missing')
  end
  version = jobs.fetch('version-label')
  require_check(version.fetch('name') == 'CI / Version label' && version.fetch('if') == 'always()', 'version gate missing')
  require_check(version.fetch('needs') == ['changes'], 'version must use selector')
  %w[deployables test_tracks].each { |key| require_check(jobs.fetch('changes').fetch('outputs').key?(key), 'shared selector output absent') }
  %w[runtime-images omarchy rust-test-build].each do |key|
    require_check(jobs.fetch(key).fetch('if') == "needs.changes.outputs.am == 'true' || needs.changes.outputs.eg == 'true'", 'shared Rust prerequisite selection differs')
  end
  rust = documents.fetch('ci-rust').fetch('jobs')
  require_check(rust.fetch('tests').fetch('strategy') == {'fail-fast' => false, 'matrix' => {'shard' => [1, 2, 3]}}, 'three test shards required')
  require_check(rust.fetch('result').fetch('if') == 'always()', 'reusable failure propagation missing')
  require_check(rust.fetch('result').fetch('needs').sort == %w[lint tests timing mongo isolation].sort, 'Rust child omitted')
  timing = rust.fetch('timing').fetch('steps')
  ['Image normalization budgets', 'Gateway socket latency budget', 'Enforce the production budgets (one retry)'].each do |name|
    require_check(timing.find { |s| s['name'] == name }&.fetch('if') == "inputs.track == 'eg'", 'Gateway timing ownership differs')
  end
  steps = documents.values.flat_map { |doc| doc.fetch('jobs').values.flat_map { |job| job.fetch('steps', []) } }
  archives = steps.select { |s| s.fetch('run', '').include?('cargo nextest archive') }
  require_check(archives.length == 1 && archives[0]['run'].include?('--workspace --locked'), 'one locked workspace archive required')
  uploads = steps.select { |s| s.fetch('with', {})['name'] == 'rust-tests-${{ github.sha }}' }
  require_check(uploads.length == 1 && uploads[0]['with']['retention-days'] == 1, 'event SHA archive retention differs')
  structure = documents.fetch('ci-repo').fetch('jobs').fetch('structure').fetch('steps')
  require_check(structure.none? { |s| s.fetch('name', '').match?(/Release |Skopeo|image reconciliation|image import/i) }, 'release tooling leaked into Repo')
  serialized = JSON.generate(documents)
  require_check(!serialized.include?('continue-on-error":true,"run'), 'product gate bypass')
  require_check(!serialized.include?('ci-contract.rb') && !serialized.include?('staging-authority.test'), 'Trigger tooling retained')
end

documents = Dir.glob(File.join(root, '.github/workflows/ci*.yml')).to_h { |file| [File.basename(file, '.yml'), YAML.load_file(file)] }
validate(documents)
mutations = [
  ->(d) { d['ci'][true]['push'] = {'branches' => ['develop']} },
  ->(d) { d['ci']['jobs']['gate-eg']['needs'].delete('runtime-images') },
  ->(d) { d['ci']['jobs']['gate-am']['if'] = 'success()' },
  ->(d) { d['ci']['jobs']['version-label']['name'] = 'Optional label' },
  ->(d) { d['ci-rust']['jobs']['result']['needs'].delete('mongo') },
  ->(d) { d['ci-rust']['jobs']['tests']['strategy']['matrix']['shard'] = [1, 2] },
]
mutations.each do |mutate|
  copy = Marshal.load(Marshal.dump(documents))
  mutate.call(copy)
  begin
    validate(copy)
  rescue RuntimeError
    next
  end
  abort 'workflow regression accepted'
end
puts 'CI event, required gate, archive, ownership and mutation contracts passed'

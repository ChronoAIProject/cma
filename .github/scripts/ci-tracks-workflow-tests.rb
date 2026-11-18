#!/usr/bin/env ruby
require 'yaml'
require 'json'
ROOT = File.expand_path('../..', __dir__)
TRACKS = {'repo' => '1 Repo', 'am' => '2 Agent Manager', 'eg' => '3 Event Gateway', 'web' => '4 Web'}.freeze

def require_check(value, message)
  raise message unless value
end

def validate(documents, actions)
  require_check(documents.keys.sort == TRACKS.keys.sort, 'exactly four CI workflows required')
  TRACKS.each do |track, name|
    workflow = documents.fetch(track)
    require_check(workflow.fetch('name') == name, 'workflow name differs')
    events = workflow.fetch('on', workflow[true])
    require_check(events.keys == ['pull_request'], 'CI must remain PR-only')
    pr = events.fetch('pull_request')
    require_check(pr.fetch('branches') == ['develop'], 'CI target differs')
    require_check(pr.fetch('types').sort == %w[opened reopened synchronize edited labeled unlabeled].sort, 'event coverage differs')
    require_check(!pr.key?('paths') && !pr.key?('paths-ignore'), 'path skips must emit the gate')
    jobs = workflow.fetch('jobs')
    require_check(jobs.keys.first == 'changes', 'change detection must be first')
    require_check(jobs.values.none? { |job| job.key?('uses') }, 'reusable workflow group retained')
    require_check(jobs.values.count { |job| job['name'].end_with?(' / Gate') } == 1, 'one gate required')
    gate = jobs.fetch('gate')
    require_check(gate['name'] == name + ' / Gate' && gate['if'] == 'always()', 'gate must always run')
    require_check(gate['needs'].sort == (jobs.keys - ['gate']).sort, 'task omitted from gate')
    require_check(gate['steps'].last['run'] == "python3 -B .github/scripts/ci_tracks/gate.py #{track}", 'gate validator absent')
    jobs.each do |key, job|
      next if key == 'gate'
      require_check(job['name'].match?(/\A#{name[0]}\.\d+ /), 'task numbering differs')
      next if key == 'changes' || track == 'repo'
      selection = track == 'am' && %w[runtime omarchy].include?(key) ?
        "needs.changes.outputs.am == 'true' || needs.changes.outputs.eg == 'true'" : "needs.changes.outputs.#{track} == 'true'"
      require_check(job['if'] == selection && job['needs'].include?('changes'), 'task selection differs')
    end
    require_check(jobs['changes']['steps'].last['uses'] == './.github/actions/ci-select', 'local selector missing')
  end
  repo = documents['repo']['jobs']
  require_check(repo.keys.sort == %w[changes structure secrets version-label gate].sort, 'Repo inventory differs')
  require_check(repo['version-label']['name'] == '1.3 Version label', 'label must be task 1.3')
  %w[image_scanner_policy.py scan-secret-history.sh].each do |command|
    require_check(repo['secrets']['steps'].any? { |s| s['run'].to_s.include?(command) }, 'secret policy missing')
  end
  migration = repo['structure']['steps'].find { |s| s['uses'] == './.github/actions/ci-migration-contracts' }
  require_check(migration && migration['timeout-minutes'] == 10, 'migration regressions missing or unbounded')
  %w[am eg].each do |track|
    jobs = documents[track]['jobs']
    require_check(jobs['tests']['strategy'] == {'fail-fast' => false, 'matrix' => {'shard' => [1, 2, 3]}}, 'three shards required')
    require_check(jobs['test-build']['steps'].last['uses'] == './.github/actions/ci-rust-build', 'test archive missing')
    %w[tests timing mongo].each { |key| require_check(jobs[key]['needs'].include?('test-build'), 'archive consumer unordered') }
    require_check(!jobs.key?('isolation') && !jobs.key?('result'), 'old child groups retained')
  end
  require_check(!documents['eg']['jobs'].key?('runtime') && !documents['eg']['jobs'].key?('omarchy'), 'runtime duplicated')
  mongo = documents['am']['jobs']['mongo']['steps']
  require_check(mongo.find { |s| s['name'] == 'Disposable real Mongo persistence contracts' }['timeout-minutes'] == 75, 'Mongo threshold changed')
  require_check(mongo.last['uses'] == './.github/actions/ci-mongo-isolation' && mongo.last['timeout-minutes'] == 30, 'isolation missing or unbounded')
  archive = actions['ci-rust-build']['runs']['steps']
  require_check(archive.count { |s| s['run'].to_s.include?('cargo nextest archive --workspace --locked') } == 1, 'locked archive missing')
  require_check(archive.last['with']['retention-days'] == 1, 'archive retention differs')
  require_check(documents['web']['jobs'].keys.sort == %w[changes install format lint typecheck tests build styles browser audit docker scan gate].sort, 'Web tasks differ')
  %w[am eg web].each do |track|
    jobs = documents[track]['jobs']
    require_check(jobs['scan']['needs'].include?('docker'), 'scanner lacks build prerequisite')
    build = jobs['docker']['steps']
    pre_scan = build.index { |s| s['uses'].to_s.start_with?('aquasecurity/trivy-action@') }
    transfer = build.index { |s| s['run'].to_s.end_with?('image_transfer.py save') }
    require_check(pre_scan && transfer && pre_scan < transfer, 'unscanned image transfer')
    scan = jobs['scan']['steps']
    load_at = scan.index { |s| s['run'].to_s.end_with?('image_transfer.py load') }
    scanner_at = scan.index { |s| s['uses'].to_s.start_with?('aquasecurity/trivy-action@') }
    export_at = scan.index { |s| s['name'] == 'Export tested native image' }
    require_check(load_at && scanner_at && export_at && load_at < scanner_at && scanner_at < export_at, 'scan must precede tested export')
  end
  steps = documents.values.flat_map { |d| d['jobs'].values.flat_map { |j| j.fetch('steps', []) } }
  steps.select { |s| s['uses'].to_s.start_with?('aquasecurity/trivy-action@') }.each do |s|
    require_check(s['uses'] == 'aquasecurity/trivy-action@ed142fd0673e97e23eac54620cfb913e5ce36c25', 'scanner pin changed')
    require_check(s['with'].slice('severity', 'exit-code', 'ignore-unfixed') == {'severity' => 'HIGH,CRITICAL', 'exit-code' => '1', 'ignore-unfixed' => true}, 'scan thresholds changed')
    require_check(!s.key?('continue-on-error'), 'scanner bypassed')
  end
  require_check(!JSON.generate(documents).match?(/ci-contract\.rb|staging-authority\.test|trigger-contract/), 'Trigger tooling retained')
end

documents = Dir.glob(File.join(ROOT, '.github/workflows/ci*.yml')).to_h { |f| [File.basename(f, '.yml').delete_prefix('ci-'), YAML.load_file(f)] }
actions = Dir.glob(File.join(ROOT, '.github/actions/ci-*/action.yml')).to_h { |f| [File.basename(File.dirname(f)), YAML.load_file(f)] }
# GitHub's dynamic CodeQL workflow remains independent; this contract checks
# only source-controlled workflows, without replacing default-setup scanning.
develop_workflows = Dir.glob(File.join(ROOT, '.github/workflows/*.yml')).map do |file|
  doc = YAML.load_file(file)
  pr = doc.fetch('on', doc[true]).fetch('pull_request', nil)
  File.basename(file, '.yml') if pr && pr.fetch('branches', ['develop']).include?('develop')
end.compact
require_check(develop_workflows.sort == TRACKS.keys.map { |key| 'ci-' + key }.sort, 'extra develop PR workflow')
validate(documents, actions)
mutations = [
  ->(d, _) { d['am']['jobs']['gate']['needs'].delete('runtime') },
  ->(d, _) { d['eg']['jobs']['tests']['strategy']['matrix']['shard'] = [1, 2] },
  ->(d, _) { d['web']['jobs']['gate']['if'] = 'success()' },
  ->(d, _) { d['repo']['jobs']['version-label']['name'] = 'Optional label' },
  ->(d, _) { d['am']['jobs']['mongo']['steps'].pop },
  ->(d, _) { d['am']['jobs']['runtime']['if'] = "needs.changes.outputs.am == 'true'" },
  ->(d, _) { d['web']['jobs']['scan']['steps'].find { |s| s['uses'].to_s.start_with?('aquasecurity/') }['with']['exit-code'] = '0' },
  ->(_, a) { a['ci-rust-build']['runs']['steps'].find { |s| s['run'].to_s.include?('nextest archive') }['run'].sub!('--locked', '') }
]
mutations.each do |mutation|
  copy = Marshal.load(Marshal.dump([documents, actions]))
  mutation.call(*copy)
  begin
    validate(*copy)
  rescue RuntimeError
    next
  end
  abort 'workflow regression accepted'
end
puts 'Four workflows, numbered tasks, gates, archive, scanner and mutation contracts passed'

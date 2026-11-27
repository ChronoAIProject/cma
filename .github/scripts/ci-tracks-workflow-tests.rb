#!/usr/bin/env ruby
require 'yaml'
require_relative 'ci-track-validation'
require_relative 'workflow-yaml'
root = File.expand_path('../..', __dir__)

# Every artifact download ends within a bound. Composite steps accept no
# timeout-minutes (the runner's action schema, actions/runner#1979), so no
# composite may use any download-artifact action or `gh run download`: the
# shared Rust archive is restored by a script that stops, repeats and
# digest-checks each attempt. A workflow download action is an optional
# bounded attempt, one identical bounded repeat that can fail the job, and a
# step that names what was not downloaded; `gh run download` in a workflow
# step needs that step's timeout-minutes.
DOWNLOAD_ACTION = /download-artifact@/
RUN_DOWNLOAD = /\bgh\s+run\s+download\b/
ARCHIVE_ACTION = './.github/actions/rust-archive'
ARCHIVE_RESTORE = {
  'name' => 'Download Backend test archive', 'shell' => 'bash', 'working-directory' => '.',
  'env' => {'GH_TOKEN' => '${{ github.token }}', 'ARTIFACT_NAME' => 'rust-tests-${{ github.sha }}'},
  'run' => 'python3 -B .github/scripts/ci_artifact_download.py --name "$ARTIFACT_NAME" --path "$RUNNER_TEMP"'
}.freeze
ARTIFACT_READER = {'contents' => 'read', 'actions' => 'read'}.freeze
COMPOSITE_KEYS = {'run' => %w[name id if run env continue-on-error working-directory shell],
                  'uses' => %w[name id if uses with env continue-on-error]}.freeze
DOWNLOAD_MINUTES = 1..10
# Every artifact download is bounded; the follow-up count stays empty.
UNBOUNDED_DOWNLOADS = {}.freeze

def check_artifact_downloads(workflows, actions)
  actions.each do |reference, action|
    next unless action.fetch('runs')['using'] == 'composite'
    action.fetch('runs').fetch('steps').each do |step|
      require_check(!step.fetch('uses', '').match?(DOWNLOAD_ACTION) && !step.fetch('run', '').match?(RUN_DOWNLOAD),
        "#{reference}: composite downloads cannot be bounded")
      allowed = COMPOSITE_KEYS.fetch(step.key?('uses') ? 'uses' : 'run')
      require_check((step.keys - allowed).empty?, "#{reference}: composite step key the runner rejects")
    end
  end
  archive = actions.fetch(ARCHIVE_ACTION).fetch('runs').fetch('steps')
  restore = archive.index { |s| s['name'] == ARCHIVE_RESTORE['name'] }
  require_check(restore && archive[restore] == ARCHIVE_RESTORE &&
    archive[restore + 1]&.fetch('name', nil) == 'Extract Backend test archive once',
    'Rust archive must be restored by the bounded, digest-checked script before extraction')
  # The restore verifies the digest the upload recorded, so the uploader may not float.
  uploads = workflows.values.flat_map { |w| w.fetch('jobs').values.flat_map { |job| job.fetch('steps', []) } }
    .select { |s| s.dig('with', 'name') == ARCHIVE_RESTORE.dig('env', 'ARTIFACT_NAME') }
  require_check(uploads.length == 1 && uploads[0]['uses'].to_s.match?(%r{\Aactions/upload-artifact@[0-9a-f]{40}\z}),
    'Rust archive upload must stay pinned: its consumers verify the digest it records')
  # The restore reads this run's artifacts; a called workflow holds that only when its caller grants it.
  archive_jobs = ->(job) { job.fetch('steps', []).any? { |s| s['uses'] == ARCHIVE_ACTION } }
  consumers = workflows.select { |_, w| w.fetch('jobs').values.any?(&archive_jobs) }
  require_check(!consumers.empty?, 'Rust archive consumers missing')
  consumers.each do |name, workflow|
    workflow.fetch('jobs').each do |id, job|
      next unless archive_jobs.call(job)
      require_check(job['permissions'] == ARTIFACT_READER, "#{name}/#{id}: archive restore needs exactly actions: read")
    end
    callers = workflows.values.flat_map { |w| w.fetch('jobs').values.select { |job| job['uses'] == "./.github/workflows/#{name}.yml" } }
    require_check(callers.all? { |job| job['permissions'] == ARTIFACT_READER }, "#{name}: every caller must grant actions: read")
  end
  unbounded, chains = Hash.new(0), 0
  workflows.each do |name, workflow|
    workflow.fetch('jobs').each do |id, job|
      steps, index = job.fetch('steps', []), 0
      while index < steps.length
        first = steps[index]
        index += 1
        if first.fetch('run', '').match?(RUN_DOWNLOAD)
          require_check(DOWNLOAD_MINUTES.cover?(first['timeout-minutes']),
            "#{name}/#{id}: gh run download must be bounded by its step's timeout-minutes")
        end
        next unless first.fetch('uses', '').match?(DOWNLOAD_ACTION)
        unless first.key?('timeout-minutes') || first.key?('continue-on-error')
          unbounded[name] += 1
          next
        end
        repeat, explain, minutes = steps[index], steps[index + 1], first['timeout-minutes']
        require_check(first['continue-on-error'] == true && first['id'] && DOWNLOAD_MINUTES.cover?(minutes),
          "#{name}/#{id}: the first download attempt must be optional and bounded")
        condition = "steps.#{first['id']}.outcome == 'failure'"
        require_check(repeat && repeat['uses'] == first['uses'] && repeat['with'] == first['with'] &&
          repeat['timeout-minutes'] == minutes && repeat['id'] && !repeat.key?('continue-on-error') &&
          repeat['if'] == (first.key?('if') ? "#{first['if']} && #{condition}" : condition),
          "#{name}/#{id}: the download repeat must be identical, bounded and able to fail the job")
        require_check(explain && explain['if'] == "failure() && steps.#{repeat['id']}.outcome == 'failure'" &&
          !explain.key?('continue-on-error') && explain['run'].to_s.include?('::error title=') &&
          explain['run'].include?("both attempts failed. Each attempt is limited to #{minutes} minutes") &&
          explain['run'].strip.end_with?('exit 1'), "#{name}/#{id}: an exhausted download must explain itself")
        chains += 1
        index += 2
      end
    end
  end
  require_check(chains.positive?, 'bounded download chains missing')
  contracts = workflows.dig('ci-repo', 'jobs', 'structure', 'steps').find { |s| s['name'] == 'Staging publication and deployment contracts' }
  require_check(contracts && !contracts.key?('if') && !contracts.key?('continue-on-error') &&
    contracts['run'].lines.map(&:strip).include?(%q("$RUNNER_TEMP/staging-cd-tests/bin/python" -B -m unittest discover -s .github/scripts -p 'ci_artifact_download_tests.py')),
    'bounded download regressions must run in Repository checks')
  require_check(unbounded == UNBOUNDED_DOWNLOADS, 'every new artifact download must be bounded; update the follow-up count when one is fixed')
end

documents = Dir.glob(File.join(root, '.github/workflows/ci*.yml')).to_h do |file|
  [File.basename(file, '.yml'), WorkflowYAML.load_file(file)]
end
actions = Dir.glob(File.join(root, '.github/actions/**/action.{yml,yaml}')).to_h do |file|
  ["./#{File.dirname(file).delete_prefix(root + '/')}", YAML.load_file(file)]
end
validate(documents, actions)
check_repo_secret_gate(root)
check_verification_archive_scan_paths(documents)
workflows = Dir.glob(File.join(root, '.github/workflows/*.{yml,yaml}')).to_h do |file|
  [File.basename(file, File.extname(file)), WorkflowYAML.load_file(file)]
end
check_merge_group_triggers(workflows)
[->(w) { w['staging-cd'][true]['merge_group'] = nil },
 ->(w) { w['ci-omarchy'][true]['merge_group'] = {'types' => ['checks_requested']} },
 ->(w) { w['ci'][true].delete('merge_group') },
 ->(w) { w['ci']['permissions'] = 'write-all' },
 ->(w) { w['ci']['jobs']['version-label']['permissions']['pull-requests'] = 'write' }].each do |mutate|
  copy = Marshal.load(Marshal.dump(workflows))
  mutate.call(copy)
  begin
    check_merge_group_triggers(copy)
  rescue RuntimeError
    next
  end
  abort 'merge-group trigger regression accepted'
end
check_flaky_tracker(workflows)
tracker_job = ->(w) { w[TRACKER]['jobs']['track'] }
[->(w) { w[TRACKER][true]['pull_request'] = {'branches' => ['develop']} },
 ->(w) { w[TRACKER][true]['pull_request_target'] = {} },
 ->(w) { w[TRACKER][true]['workflow_run'] = {'workflows' => ['CI'], 'types' => ['completed']} },
 ->(w) { w[TRACKER][true]['schedule'][0]['cron'] = '* * * * *' },
 ->(w) { w[TRACKER]['permissions'] = 'write-all' },
 ->(w) { w[TRACKER]['concurrency']['cancel-in-progress'] = true },
 ->(w) { tracker_job.call(w)['permissions']['contents'] = 'write' },
 ->(w) { tracker_job.call(w)['permissions']['pull-requests'] = 'write' },
 ->(w) { tracker_job.call(w)['if'] = "github.repository == 'ChronoAIProject/cma'" },
 ->(w) { tracker_job.call(w)['steps'][0]['with']['ref'] = '${{ github.event.pull_request.head.sha }}' },
 ->(w) { tracker_job.call(w)['steps'][1]['continue-on-error'] = true },
 ->(w) { w['ci']['permissions']['issues'] = 'write' },
 ->(w) { w['ci-rust']['jobs']['mongo-shards']['permissions']['issues'] = 'write' },
 ->(w) { w['staging-cd']['permissions'] = {'contents' => 'read', 'issues' => 'write'} },
 ->(w) { w.delete(TRACKER) }].each do |mutate|
  copy = Marshal.load(Marshal.dump(workflows))
  mutate.call(copy)
  begin
    check_flaky_tracker(copy)
  rescue RuntimeError, KeyError
    next
  end
  abort 'flaky tracker authority regression accepted'
end
check_artifact_downloads(workflows, actions)
restore_step = ->(a) { a[ARCHIVE_ACTION]['runs']['steps'].find { |s| s['name'] == ARCHIVE_RESTORE['name'] } }
receipt_steps = ->(w) { w['staging-release']['jobs']['finalize']['steps'] }
receipt = ->(w, id) { receipt_steps.call(w).find { |s| s['id'] == id } }
report_steps = ->(w) { w['staging-deploy']['jobs']['report']['steps'] }
archive_upload = ->(w) { w['ci-rust-archive']['jobs']['build']['steps'].find { |s| s['name'] == 'Upload Backend test archive' } }
pinned = 'actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333'
third_party = 'example-owner/action-download-artifact@' + 'b' * 40
composite = 'composite downloads cannot be bounded'
restore_rule = 'Rust archive must be restored by the bounded, digest-checked script'
first_rule = 'staging-release/finalize: the first download attempt must be optional and bounded'
repeat_rule = 'staging-release/finalize: the download repeat must be identical, bounded and able to fail the job'
explain_rule = 'staging-release/finalize: an exhausted download must explain itself'
count_rule = 'every new artifact download must be bounded'
run_rule = "staging-deploy/report: gh run download must be bounded by its step's timeout-minutes"
[[composite, ->(_, a) { a[ARCHIVE_ACTION]['runs']['steps'].insert(3, {'uses' => pinned, 'with' => {'name' => 'x'}}) }],
 [composite, ->(_, a) { a[ARCHIVE_ACTION]['runs']['steps'].insert(3, {'uses' => third_party, 'with' => {'name' => 'x'}}) }],
 [composite, ->(_, a) { a[ARCHIVE_ACTION]['runs']['steps'].insert(3, {'run' => 'gh run download "$GITHUB_RUN_ID" -n x', 'shell' => 'bash'}) }],
 ['composite step key the runner rejects', ->(_, a) { restore_step.call(a)['timeout-minutes'] = 6 }],
 [restore_rule, ->(_, a) { restore_step.call(a)['continue-on-error'] = true }],
 [restore_rule, ->(_, a) { restore_step.call(a)['run'] = 'true' }],
 [restore_rule, ->(_, a) { restore_step.call(a)['env'].delete('GH_TOKEN') }],
 [restore_rule, ->(_, a) { a[ARCHIVE_ACTION]['runs']['steps'].reverse! }],
 ['Rust archive upload must stay pinned', ->(w, _) { archive_upload.call(w)['uses'] = 'actions/upload-artifact@v4' }],
 ['ci-rust/tests: archive restore needs exactly actions: read', ->(w, _) { w['ci-rust']['jobs']['tests'].delete('permissions') }],
 ['ci-rust/mongo-eg: archive restore needs exactly actions: read', ->(w, _) { w['ci-rust']['jobs']['mongo-eg']['permissions']['actions'] = 'write' }],
 ['ci-rust: every caller must grant actions: read', ->(w, _) { w['ci']['jobs']['eg'].delete('permissions') }],
 [first_rule, ->(w, _) { receipt.call(w, 'receipts-download').delete('timeout-minutes') }],
 [first_rule, ->(w, _) { receipt.call(w, 'receipts-download')['timeout-minutes'] = 30 }],
 [repeat_rule, ->(w, _) { receipt.call(w, 'receipts-download-retry').delete('timeout-minutes') }],
 [repeat_rule, ->(w, _) { receipt.call(w, 'receipts-download-retry')['continue-on-error'] = true }],
 [repeat_rule, ->(w, _) { receipt.call(w, 'receipts-download-retry')['with']['path'] = '${{ runner.temp }}' }],
 [repeat_rule, ->(w, _) { receipt.call(w, 'receipts-download-retry')['if'] = 'always()' }],
 [repeat_rule, ->(w, _) { receipt_steps.call(w).delete(receipt.call(w, 'receipts-download-retry')) }],
 [explain_rule, ->(w, _) { receipt_steps.call(w).reject! { |s| s['name'] == 'Explain the unavailable receipt artifact' } }],
 [explain_rule, ->(w, _) { receipt_steps.call(w).find { |s| s['name'] == 'Explain the unavailable receipt artifact' }['run'].sub!('exit 1', 'exit 0') }],
 [count_rule, ->(w, _) { report_steps.call(w) << {'uses' => pinned, 'with' => {'name' => 'evidence'}} }],
 [count_rule, ->(w, _) { report_steps.call(w) << {'uses' => third_party, 'with' => {'name' => 'evidence'}} }],
 [count_rule, ->(w, _) { w['publish']['jobs']['release']['steps'] << {'uses' => pinned, 'with' => {'name' => 'evidence'}} }],
 [count_rule, ->(w, _) {
   steps = w['publish']['jobs']['release']['steps']
   steps.reject! { |s| s['id'] == 'notes-download-retry' || s['name'] == 'Explain the unavailable release notes artifact' }
   steps.find { |s| s['id'] == 'notes-download' }.tap { |s| s.delete('timeout-minutes'); s.delete('continue-on-error') }
 }],
 ['publish/release: the download repeat must be identical, bounded and able to fail the job',
  ->(w, _) { w['publish']['jobs']['release']['steps'].find { |s| s['id'] == 'notes-download-retry' }['with']['name'] = 'other' }],
 [run_rule, ->(w, _) { report_steps.call(w) << {'run' => 'gh run download "$GITHUB_RUN_ID" -n evidence'} }],
 [run_rule, ->(w, _) { report_steps.call(w) << {'run' => 'gh run download "$GITHUB_RUN_ID" -n evidence', 'timeout-minutes' => 60} }],
 ['bounded download regressions must run in Repository checks',
  ->(w, _) { w['ci-repo']['jobs']['structure']['steps'].find { |s| s['name'] == 'Staging publication and deployment contracts' }['run'].sub!(/^.*ci_artifact_download_tests.*\n/, '') }]].each do |expected, mutate|
  copy, actions_copy = Marshal.load(Marshal.dump(workflows)), Marshal.load(Marshal.dump(actions))
  mutate.call(copy, actions_copy)
  begin
    check_artifact_downloads(copy, actions_copy)
  rescue RuntimeError => error
    raise "download mutation refused for another reason: #{error.message} (expected #{expected})" unless error.message.include?(expected)
    next
  end
  abort "unbounded artifact download accepted: #{expected}"
end
# A gh run download bounded by its step's timeout is accepted.
copy = Marshal.load(Marshal.dump(workflows))
report_steps.call(copy) << {'run' => 'gh run download "$GITHUB_RUN_ID" -n evidence', 'timeout-minutes' => 5}
check_artifact_downloads(copy, actions)
before_queue = "success() && (github.event_name == 'push' || github.event.pull_request.head.repo.full_name == github.repository)"
mutations = [
 ->(d) { d['ci-two-am']['jobs']['startup-controls']['steps'][2]['continue-on-error'] = true },
 ->(d) { d['ci-two-am']['jobs']['startup-controls']['steps'][2]['run'] = 'true' },
 ->(d) { d['ci-pr-am']['jobs']['two-am-features']['needs'].delete('rust-test-build') },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Vulnerability scan' }['if'] = 'true' },
  ->(d) { d['ci-images']['jobs']['images']['steps'].reject! { |s| s['name'] == 'Runtime vulnerability gate' } },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Runtime vulnerability gate' }['continue-on-error'] = true },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Runtime vulnerability gate' }['run'] += ' || true' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].reject! { |s| s['name'] == 'Omarchy vulnerability gate' } },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Omarchy vulnerability gate' }['env']['BASE_SHA'] = '' },
  *image_scan_workflow_mutations,
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].reject! { |s| s['name'] == 'API allocator qualification' } },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'] << d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == 'API allocator qualification' }.dup },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == 'API allocator qualification' }['continue-on-error'] = true },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == 'API allocator qualification' }['run'].sub!('--test-threads 1', '--test-threads 2') },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == 'Enforce Gateway latency budgets' }['run'] += ' || true' },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == 'Enforce Gateway latency budgets' }['continue-on-error'] = true },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].reject! { |s| s['name'] == 'Gateway latency runner regressions' } },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s.dig('with', 'name').to_s.start_with?('cmaeg-latency-') }['with']['path'] = 'api/target/cmaeg-latency.txt' },
  ->(d) { d['ci'][true].delete('merge_group') },
  ->(d) { d['ci'][true]['merge_group']['types'] = ['destroyed'] },
  ->(d) { d['ci'][true]['merge_group']['branches'] = ['main'] },
  ->(d) { d['ci'][true]['merge_group']['paths'] = ['web/**'] },
  ->(d) { d['ci']['concurrency']['group'] = 'ci-${{ github.workflow }}-${{ github.event.merge_group.base_ref || github.ref }}' },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Export tested native image' }['if'] = before_queue },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Retain tested native image' }['if'] = 'success()' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Export tested Omarchy image' }['if'] = 'success()' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Retain tested Omarchy image' }['if'] = before_queue },
  ->(d) { d['ci-two-am']['jobs']['features']['steps'].find { |s| s['working-directory'] == 'web' }['if'] = "matrix.fixture == 'prepared'" },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Prepare API Cargo cache identity' }['if'] = "matrix.component == 'api'" },
  ->(d) { d['ci-repo']['jobs']['structure']['steps'].reject! { |s| s['name'] == 'Synthetic authority and observer regressions' } },
  ->(d) { d['ci-repo']['jobs']['structure']['if'] = 'false' },
  ->(d) { d['ci-repo']['jobs']['structure']['continue-on-error'] = true },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s.dig('with', 'cache-from') }['with']['cache-to'] = 'type=gha,mode=max' },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s.dig('with', 'cache-from') }['with']['cache-from'].sub!("steps.layer-validate.outcome == 'success'", 'true') },
  ->(d) { d['ci-web']['jobs']['web']['steps'].find { |s| s['name'] == 'Restore npm download cache' }['uses'] = 'actions/cache@v4' },
  ->(d) { d['ci-web']['jobs']['web-conversation']['steps'].find { |s| s['name'] == 'Restore npm download cache' }['uses'] = 'actions/cache@v4' },
  ->(d) { d['ci-web']['jobs']['web-conversation']['steps'].find { |s| s.fetch('uses', '').start_with?('actions/setup-node@') }['with']['cache'] = 'npm' },
  ->(d) { d['ci-web']['jobs']['web-conversation']['steps'].delete_at(4) },
  ->(d) { d['ci-web']['jobs']['web-conversation']['continue-on-error'] = true },
  ->(d) { d['ci-web']['jobs']['web-conversation']['timeout-minutes'] = 120 },
  ->(d) { d['ci-web']['jobs'].delete('web-conversation'); d['ci-web']['jobs']['web-result']['needs'].delete('web-conversation') },
  ->(d) { d['ci-web']['jobs']['web-result']['needs'].delete('web-design') },
  ->(d) { d['ci-web']['jobs']['web-result']['if'] = 'success()' },
  ->(d) { d['ci-web']['jobs'].delete('web-result') },
  ->(d) { d['ci-web']['jobs']['web-result']['continue-on-error'] = true },
  ->(d) { d['ci-web']['jobs']['web-result']['steps'].last['run'] = 'true' },
  ->(d) { d['ci-web']['jobs']['web-result']['steps'].last['if'] = 'false' },
  ->(d) { d['ci-web']['jobs']['web-conversation']['steps'].reject! { |s| s['name'] == 'Install pinned local browser' } },
  ->(d) { d['ci-web']['jobs']['web-conversation']['steps'] << d['ci-web']['jobs']['web']['steps'].find { |s| s['name'] == 'Tests and critical Agent browser coverage' }.dup },
  ->(d) { d['ci-web']['jobs']['web-conversation']['steps'].find { |s| s['name'] == 'Production build' }['env'].delete('CHRONO_BUILD_ARTIFACT') },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Restore Trivy databases' }['with'].delete('restore-keys') },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Vulnerability scan' }['with'].delete('cache') },
  ->(d) { d['ci'][true].delete('push') },
  ->(d) { d['ci'][true].delete('schedule') },
  ->(d) { d['ci'][true]['workflow_dispatch'] = {'inputs' => {'commit' => {'type' => 'string'}}} },
  ->(d) { d['ci'][true]['push']['paths'] = ['web/**'] },
  ->(d) { d['ci-pr-repo'][true]['pull_request']['branches'].delete('main') },
  ->(d) { d['ci'][true]['push']['branches'].delete('tl/release/**') },
  ->(d) { d['ci'][true]['push']['branches'] << '**' },
  ->(d) { d['ci']['concurrency']['group'] = 'ci-${{ github.workflow }}-${{ github.ref }}' },
  # A suffix lets a dispatch at tl/release/v1 share the push group for tl/release/v1-verification.
  ->(d) { d['ci']['concurrency']['group'] = "ci-${{ github.workflow }}-${{ github.ref }}${{ (github.event_name == 'schedule' || github.event_name == 'workflow_dispatch') && '-verification' || '' }}" },
  ->(d) { d['ci']['jobs']['repo'].delete('if') },
  ->(d) { d['ci']['jobs']['changes'].delete('if') },
  ->(d) { d['ci-images']['jobs']['images']['concurrency']['group'] = 'cma-develop-cache-readers' },
  ->(d) { d['ci-images']['jobs']['images']['concurrency']['cancel-in-progress'] = true },
  ->(d) { d['ci']['jobs']['changes']['steps'].last['run'] = 'python3 -B .github/scripts/ci_tracks/selection.py' },
  ->(d) { d['ci']['jobs']['am']['if'] = 'false' },
  ->(d) { d['ci']['jobs']['eg']['if'] = 'false' },
  ->(d) { d['ci']['jobs']['web-images']['if'] = 'false' },
  ->(d) { d['ci']['jobs']['eg']['with']['packages'] = '["cmaeg"]' },
  ->(d) { d['ci']['permissions']['contents'] = 'write' },
  ->(d) { d['ci']['concurrency']['cancel-in-progress'] = false },
  ->(d) { d['ci']['concurrency']['cancel-in-progress'] = true },
  ->(d) { d['ci']['jobs']['gate-eg']['needs'].delete('runtime-images') },
  ->(d) { d['ci']['jobs']['gate-am']['if'] = 'success()' },
  ->(d) { d['ci']['jobs']['gate-repo']['if'] = 'always()' },
  ->(d) { d['ci']['jobs']['version-label']['if'] = '${{ !cancelled() }}' },
  ->(d) { d['ci']['jobs']['version-label']['name'] = 'Optional label' },
  ->(d) { d['ci']['jobs']['version-label']['if'] = 'always()' },
  ->(d) { d['ci']['jobs']['version-label']['if'] = 'success()' },
  ->(d) { d['ci-pr-repo']['jobs']['version-label']['if'] = 'always()' },
  ->(d) { d['ci-pr-repo']['jobs']['version-label']['if'] = 'success()' },
  ->(d) { d['ci-pr-am'][true]['pull_request']['types'].delete('labeled') },
  ->(d) { d['ci-pr-eg'][true]['pull_request']['types'].delete('unlabeled') },
  ->(d) { d['ci-pr-web'][true]['pull_request']['paths-ignore'] = ['README.md'] },
  ->(d) { d['ci-pr-am']['jobs']['runtime-images']['if'] = "needs.changes.outputs.am == 'true'" },
  ->(d) { d['ci-pr-eg']['jobs']['gate-eg']['needs'] << 'runtime-images' },
  ->(d) { d['ci-track-selection']['jobs']['changes']['steps'].last['run'] = 'true' },
  ->(d) { d['ci']['jobs']['gate-repo']['needs'].delete('version-label') },
  ->(d) { d['ci']['jobs']['version-label']['if'] = "needs.changes.outputs.deployables != '[]'" },
  ->(d) { d['ci']['jobs']['version-label']['continue-on-error'] = true },
  ->(d) { d['ci']['jobs']['version-label']['steps'].last['run'] = 'true' },
  ->(d) { d['ci']['jobs']['version-label']['steps'].last['if'] = "needs.changes.outputs.deployables != '[]'" },
  ->(d) { d['ci']['jobs']['version-label']['steps'].last['continue-on-error'] = true },
  ->(d) { d['ci']['jobs']['version-label']['steps'].last['env']['SELECTOR_RESULT'] = 'success' },
  ->(d) { d['ci']['jobs']['version-label']['steps'].last['env'].delete('GH_TOKEN') },
  ->(d) { d['ci-rust']['jobs']['result']['needs'].delete('mongo') },
  ->(d) { d['ci-rust']['jobs']['tests']['strategy']['matrix']['shard'] = [1, 2] },
  ->(d) { d['ci-rust']['jobs']['mongo']['name'] = 'Optional Mongo' },
  ->(d) { d['ci-rust']['jobs']['mongo']['if'] = 'success()' },
  ->(d) { d['ci-rust']['jobs']['mongo']['needs'].delete('mongo-shards') },
  ->(d) { d['ci-rust']['jobs']['mongo']['steps'].last['run'] = 'true' },
  ->(d) { d['ci-rust']['jobs']['mongo-shards']['strategy']['matrix']['shard'].pop },
  ->(d) { d['ci-rust']['jobs']['mongo-shards']['strategy']['matrix']['shard'].shift },
  ->(d) { d['ci-rust']['jobs']['mongo-shards']['strategy']['fail-fast'] = true },
  ->(d) { d['ci-rust']['jobs']['mongo-shards']['continue-on-error'] = true },
  ->(d) { d['ci-rust']['jobs']['mongo-shards']['steps'].last['run'] = 'true' },
  ->(d) { d['ci-rust']['jobs']['mongo-shards']['runs-on'] = 'ubuntu-latest-16-cores' },
  ->(d) { d['ci-rust']['jobs']['mongo-shards']['env'].delete('CMA_NEXTEST_REUSE') },
  ->(d) { d['ci-rust']['jobs']['mongo-shards']['steps'].first['uses'] = 'actions/checkout@v5' },
  ->(d) { d['ci-rust']['jobs']['mongo-eg']['steps'].last['run'] = 'true' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Scan tested Omarchy image' }['with'].delete('cache') },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'] << {'uses' => 'actions/cache/save@caa296126883cff596d87d8935842f9db880ef25'} },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'build' }['with']['cache-to'] = 'type=local,mode=min,dest=/tmp/omarchy' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'build' }['with']['outputs'] = 'type=docker' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'build' }['with']['build-args'].sub!('SOURCE_DATE_EPOCH=0', 'SOURCE_DATE_EPOCH=1') },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'omarchy-cache-restore' }['if'] = "github.event_name == 'push'" },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'build' }['with']['build-args'].sub!('${{ steps.os-refresh.outputs.value }}', '${{ github.run_id }}-${{ github.run_attempt }}') },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].reject! { |s| s['id'] == 'os-refresh' } },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Load native image for scanning' }['with']['build-args'].sub!('${{ steps.os-refresh.outputs.value }}', '${{ github.run_id }}-${{ github.run_attempt }}') },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['id'] == 'os-refresh' }['continue-on-error'] = true },
]
# The merge-queue flaky retry cannot widen: PR or push runs, the isolated
# shard, a hardcoded count, another job or the nextest environment retrying,
# a skipped or optional report, a lost record, or a moving expiry all fail.
flaky_steps = ->(d) { d['ci-rust']['jobs']['mongo-shards']['steps'] }
flaky_step = ->(d, id) { flaky_steps.call(d).find { |s| s['id'] == id || s['name'] == id } }
retain_step = 'Retain the flaky test record for the tracking issue'
[->(d) { flaky_step.call(d, 'flaky-retry')['if'] = "github.event_name != 'push'" },
 ->(d) { flaky_step.call(d, 'flaky-retry')['if'] = "github.event_name == 'merge_group'" },
 ->(d) { flaky_step.call(d, 'flaky-retry')['if'] = "github.event_name == 'merge_group' || github.event_name == 'pull_request'" },
 ->(d) { flaky_step.call(d, 'flaky-retry').delete('if') },
 ->(d) { flaky_step.call(d, 'flaky-retry')['continue-on-error'] = true },
 ->(d) { flaky_steps.call(d).delete(flaky_step.call(d, 'flaky-retry')) },
 ->(d) { flaky_step.call(d, 'contracts')['env']['CMA_MONGO_FLAKY_RETRIES'] = '1' },
 ->(d) { flaky_step.call(d, 'contracts')['env']['NEXTEST_RETRIES'] = '1' },
 ->(d) { flaky_step.call(d, 'contracts')['run'] += ' --retries 1' },
 ->(d) { flaky_step.call(d, 'flaky-report')['if'] = "steps.flaky-retry.outputs.retries == '1'" },
 ->(d) { flaky_step.call(d, 'flaky-report')['continue-on-error'] = true },
 ->(d) { flaky_step.call(d, 'flaky-report')['run'] += ' || true' },
 ->(d) { flaky_steps.call(d).delete(flaky_step.call(d, 'flaky-report')) },
 ->(d) { flaky_step.call(d, retain_step)['with']['retention-days'] = 90 },
 ->(d) { flaky_step.call(d, retain_step)['with']['if-no-files-found'] = 'ignore' },
 ->(d) { flaky_step.call(d, retain_step)['uses'] = 'actions/upload-artifact@v4' },
 ->(d) { flaky_steps.call(d).delete(flaky_step.call(d, retain_step)) },
 ->(d) { d['ci-rust']['jobs']['mongo-shards']['env'][FLAKY_UNTIL] = '2026-12-1' },
 ->(d) { d['ci-rust']['jobs']['mongo-shards']['env'][FLAKY_UNTIL] = '2026-02-30' },
 ->(d) { d['ci-rust']['jobs']['mongo-shards']['env'].delete(FLAKY_UNTIL) },
 ->(d) { d['ci-rust']['jobs']['mongo-shards']['permissions']['issues'] = 'write' },
 ->(d) { d['ci-rust']['jobs']['tests']['env']['NEXTEST_RETRIES'] = '1' },
 ->(d) { d['ci-rust']['jobs']['tests']['env'][FLAKY_UNTIL] = '2026-12-01' },
 ->(d) { d['ci-rust']['jobs']['mongo-eg']['steps'].last['run'] += ' --retries 1' },
 ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == 'API allocator qualification' }['run'] += ' --flaky-result pass' },
 ->(d) { d['ci']['permissions']['issues'] = 'write' },
 ->(d) { d['ci-repo']['jobs']['structure']['steps'].find { |s| s['name'] == 'Track selection and workflow regressions' }['run'].sub!(/^.*mongo_flaky_retry_tests.*\n/, '') }].each { |mutate| mutations << mutate }
synthetic_index = documents['ci-repo']['jobs']['structure']['steps'].index { |s| s['name'] == 'Synthetic authority and observer regressions' }
restore_index = documents['ci-repo']['jobs']['structure']['steps'].index { |s| s['name'] == 'Synthetic home restore evidence regressions' }
mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'].delete_at(restore_index) }
OMARCHY_RETENTION_STEPS.each_key do |name|
  index = documents['ci-repo']['jobs']['structure']['steps'].index { |s| s['name'] == name }
  mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'].delete_at(index) }
  mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'] << d['ci-repo']['jobs']['structure']['steps'][index].dup }
  [{'if' => 'false'}, {'continue-on-error' => true}, {'run' => 'true'}].each do |bypass|
    mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'][index].merge!(bypass) }
  end
  mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'][index]['run'] += ' || true' }
end
[{'if' => 'false'}, {'continue-on-error' => true}, {'env' => {'CLOUDSDK_CORE_PROJECT' => 'fixture'}},
 {'run' => 'node .github/scripts/home-backup/inventory.mjs fixture'}].each do |bypass|
  mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'][restore_index].merge!(bypass) }
end
%w[staging-authority public-journey-evidence connection-journey-evidence connection-campaign deployment-observer].each do |suite|
  mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'][synthetic_index]['run'].sub!(" .github/scripts/#{suite}.test.mjs", '') }
  mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'][synthetic_index]['run'].sub!("#{suite}.test.mjs", "#{suite}.mjs") }
end
[{'if' => 'false'}, {'continue-on-error' => true}].each do |bypass|
  mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'][synthetic_index].merge!(bypass) }
end
mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'][synthetic_index]['run'] += ' || true' }
%w[staging-authority.mjs deployment-observer.mjs].each do |tool|
  mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'] << {'run' => "node .github/scripts/#{tool}"} }
end
omarchy_steps = ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'] }
key_expiry = ->(d) { omarchy_steps.call(d).find { |s| s['name'] == KEY_EXPIRY_STEP['name'] } }
[->(d) { omarchy_steps.call(d).delete(key_expiry.call(d)) },
 ->(d) { omarchy_steps.call(d).insert(omarchy_steps.call(d).index { |s| s['id'] == 'build' }, key_expiry.call(d).dup) },
 ->(d) { step = omarchy_steps.call(d).delete(key_expiry.call(d)); omarchy_steps.call(d).insert(omarchy_steps.call(d).index { |s| s['id'] == 'build' } + 1, step) },
 ->(d) { key_expiry.call(d)['continue-on-error'] = true },
 ->(d) { key_expiry.call(d)['if'] = 'false' },
 ->(d) { key_expiry.call(d)['run'] = 'true' },
 ->(d) { key_expiry.call(d)['run'].sub!('omarchy_key_expiry.py', 'omarchy_key_expiry.py || true') },
 ->(d) { key_expiry.call(d)['run'].sub!(/^.*_tests\.py\n/, '') },
 ->(d) { key_expiry.call(d)['run'].sub!("omarchy_key_expiry.py\n", "omarchy_key_expiry.py --verify-record\n") },
 ->(d) { key_expiry.call(d)['run'].sub!("omarchy_key_expiry.py\n", "omarchy_key_expiry.py --refresh\n") },
 ->(d) { key_expiry.call(d)['run'].sub!("omarchy_key_expiry.py\n", "omarchy_key_expiry.py --fail-days 30\n") },
 ->(d) { key_expiry.call(d)['env'] = {'GITHUB_STEP_SUMMARY' => '/dev/null'} }].each { |mutate| mutations << mutate }
[['ci-images', 'images'], ['ci-omarchy', 'omarchy']].each do |file, job|
  ['removed', 'optional', 'skipped', 'fake-success', 'late'].each do |change|
    mutations << lambda do |d|
      steps = d[file]['jobs'][job]['steps']
      gate = steps.find { |step| step.fetch('run', '').include?('runtime_image_hygiene.py --image') }
      case change
      when 'removed' then steps.delete(gate)
      when 'optional' then gate['continue-on-error'] = true
      when 'skipped' then gate['if'] = 'false'
      when 'fake-success' then gate['run'] = 'true'
      when 'late' then steps.delete(gate); steps << gate
      end
    end
  end
end
documents['ci-images']['jobs']['images']['steps'].each_with_index do |step, index|
  next unless step.fetch('uses', '').start_with?('actions/setup-node@')
  mutations << ->(d) { d['ci-images']['jobs']['images']['steps'][index]['with'].delete('package-manager-cache') }
  mutations << ->(d) { d['ci-images']['jobs']['images']['steps'][index]['with']['cache'] = 'npm' }
end
repo_jobs = ->(d) { d['ci-repo']['jobs'] }
mutations.concat([
  ->(d) { repo_jobs.call(d)['secrets']['steps'].first['with']['fetch-depth'] = 1 },
  ->(d) { repo_jobs.call(d)['secrets']['steps'].pop },
  ->(d) { repo_jobs.call(d)['scanner-tests'].delete('needs') },
  ->(d) { repo_jobs.call(d)['scanner-tests']['steps'].first['with'].delete('ref') },
  ->(d) { repo_jobs.call(d)['scanner-tests']['strategy']['fail-fast'] = true },
  ->(d) { repo_jobs.call(d)['scanner-tests']['strategy']['matrix']['shard'].pop },
  ->(d) { repo_jobs.call(d)['scanner-tests']['outputs'].delete('shard_1') },
  ->(d) { repo_jobs.call(d)['scanner-tests']['steps'].last['continue-on-error'] = true },
  ->(d) { repo_jobs.call(d)['result'].delete('if') },
  ->(d) { repo_jobs.call(d)['result']['needs'].delete('scanner-tests') },
  ->(d) { repo_jobs.call(d)['result']['steps'].last['run'] = 'true' }
])
(mutations + KeyProviderCIContract.mutations).each do |mutate|
  copy = Marshal.load(Marshal.dump(documents))
  mutate.call(copy)
  begin
    validate(copy, actions)
  rescue RuntimeError
    next
  end
  abort 'workflow regression accepted'
end
# Each formerly mutable archive dependency must remain pinned, including when
# a local composite introduces another local composite between the job and it.
# The archive itself is restored by a repository script, never a third-party
# action; check_artifact_downloads refuses any download action in a composite.
archive = ARCHIVE_ACTION
%w[dtolnay/rust-toolchain@stable taiki-e/install-action@v2].each do |mutable|
  copy = Marshal.load(Marshal.dump(actions))
  step = copy.fetch(archive).fetch('runs').fetch('steps').find { |s| s.fetch('uses', '').start_with?(mutable.split('@').first + '@') }
  require_check(!step.nil?, 'archive dependency missing from pin regression')
  step['uses'] = mutable
  begin
    validate(documents, copy)
  rescue RuntimeError => error
    raise unless error.message == "mutable action: #{mutable}"
    next
  end
  abort 'mutable archive action accepted'
end
nested = './.github/actions/synthetic-pin-contract'
copy = Marshal.load(Marshal.dump(actions))
copy[archive]['runs']['steps'] << {'uses' => nested}
copy[nested] = {'runs' => {'using' => 'composite', 'steps' => [{'uses' => 'actions/checkout@' + 'a' * 40}]}}
validate(documents, copy)
['actions/checkout@v5', archive].each do |reference|
  copy[nested]['runs']['steps'][0]['uses'] = reference
  begin
    validate(documents, copy)
  rescue RuntimeError => error
    raise unless error.message.start_with?('mutable action:', 'local action cycle:')
    next
  end
  abort 'nested mutable action or local cycle accepted'
end
puts 'CI event, required gate, archive, ownership and mutation contracts passed'

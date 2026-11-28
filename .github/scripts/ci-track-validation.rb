# Shared validation for CI workflow regression and mutation tests.
require 'date'
require 'json'
require_relative 'omarchy-ci-contract'
require_relative 'key-provider-ci-contract'
require_relative 'ci-track-security-validation'
require_relative 'ci-track-build-validation'

def require_check(value, message)
  raise message unless value
end

def require_pinned_actions(steps, actions, parents = [])
  steps.each do |step|
    reference = step['uses']
    next unless reference
    if reference.start_with?('./')
      require_check(actions.key?(reference), "local action missing: #{reference}")
      require_check(!parents.include?(reference), "local action cycle: #{reference}")
      runs = actions.fetch(reference).fetch('runs')
      if runs['using'] == 'composite'
        require_pinned_actions(runs.fetch('steps'), actions, parents + [reference])
      end
    else
      require_check(reference.match?(/\A[^@\s]+@[0-9a-f]{40}\z/), "mutable action: #{reference}")
    end
  end
end

STAGING_IMAGE_FALLBACK = "steps.staging-image.outputs.adopted != 'true'"
STAGING_READ_ONLY = {'contents' => 'read', 'actions' => 'read', 'issues' => 'read'}.freeze


NATIVE_IMAGE_EVENTS = "github.event_name == 'push' || github.event_name == 'merge_group' || " \
  "github.event.pull_request.head.repo.full_name == github.repository"
VERIFICATION_IMAGE_EVENT = "inputs.export_verification_image && github.ref == 'refs/heads/develop' && " \
  "(github.event_name == 'schedule' || github.event_name == 'workflow_dispatch')"
API_CACHE_READERS = "matrix.component == 'api' && ((github.event_name == 'push' && github.ref == 'refs/heads/develop') || " \
  "(github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository) || " \
  "github.event_name == 'merge_group')"

# A queue entry tests the exact commit develop becomes, so its tested images
# are retained for exact-source CD reuse; fork PR runs still retain nothing.
def check_merge_group_artifacts(documents)
  [['ci-images', 'images', 'native', " && (matrix.component == 'api' || matrix.component == 'runtime' || matrix.component == 'cmaeg')"],
   ['ci-omarchy', 'omarchy', 'Omarchy', '']].each do |file, job, kind, component_filter|
    steps = documents.fetch(file).fetch('jobs').fetch(job).fetch('steps')
    export, retain = %w[Export Retain].map { |action| steps.select { |s| s['name'] == "#{action} tested #{kind} image" } }
    require_check(export.length == 1 && retain.length == 1, "#{file} must export and retain the tested image once")
    export, retain = export.first, retain.first
    condition = "#{STAGING_IMAGE_FALLBACK} && success() && (#{NATIVE_IMAGE_EVENTS} || (#{VERIFICATION_IMAGE_EVENT}#{component_filter}))"
    require_check(export['id'] == 'native-image-export' && export['if'].gsub(/\s+/, ' ') == condition &&
      !export.key?('continue-on-error'), "#{file} must export only from eligible tested runs")
    expected_retention = file == 'ci-images' ? IMAGE_RETAIN_IF : "success() && steps.native-image-export.outcome == 'success'"
    require_check(retain['if'].to_s.split.join(' ') == expected_retention &&
      !retain.key?('continue-on-error'), "#{file} must only retain a successful native image export")
  end
  jobs = documents.fetch('ci').fetch('jobs')
  selected = jobs.select { |_, config| config.dig('with', 'export_verification_image') }.keys.sort
  require_check(selected == %w[am-images eg-images omarchy runtime-images] &&
    jobs.dig('eg-images', 'with', 'export_verification_image') == "${{ needs.changes.outputs.two_am_features == 'true' }}",
    'develop verification must export the two-AM fixture images')
  images = documents.fetch('ci-images').fetch('jobs').fetch('images').fetch('steps')
  prepare = images.select { |s| s['name'] == 'Prepare API Cargo cache identity' }
  require_check(prepare.length == 1 && prepare.first['if'] == "#{STAGING_IMAGE_FALLBACK} && (#{API_CACHE_READERS})",
    'queue entries may restore, never save, the develop API Cargo archive')
end

def check_staging_image_adoption(documents)
  develop = "vars.CMA_STAGING_RELEASE_ARCHIVE_HANDOFF == 'true' && github.event_name == 'push' && " \
    "github.ref == 'refs/heads/develop' && github.repository == 'ChronoAIProject/cma' && "
  inputs = {
    'ci-images' => ['images', '${{ matrix.component }}',
      "((matrix.component == 'api' && contains(fromJSON(inputs.changed_deployables), 'cma-agent-manager')) || (matrix.component == 'runtime' && contains(fromJSON(inputs.changed_deployables), 'cma-runtime')))"],
    'ci-omarchy' => ['omarchy', 'runtime-omarchy', "(inputs.staging_lane_changed || github.workflow == 'Runtime Omarchy')"]
  }
  inputs.each do |file, (job, component, changed)|
    steps = documents.fetch(file).fetch('jobs').fetch(job).fetch('steps')
    adopters = steps.select { |step| step['id'] == 'staging-image' }
    require_check(adopters.length == 1, "#{file} must have one staging-image loader")
    loader = adopters.first
    require_check(loader['if'].gsub(/\s+/, ' ').strip == develop + changed &&
      loader['env'] == {'IMAGE_COMPONENT' => component, 'GH_TOKEN' => '${{ github.token }}'} &&
      loader['timeout-minutes'] == 32 && loader['run'] == 'python3 -B .github/scripts/ci_staging_image.py' &&
      !loader.key?('continue-on-error'), "#{file} may adopt only changed develop images with bounded read authority")
    builds = steps.select { |step| step.fetch('uses', '').start_with?('docker/build-push-action@') }
    require_check(!builds.empty? && builds.all? { |step| step['if'] == STAGING_IMAGE_FALLBACK &&
      steps.index(loader) < steps.index(step) }, "#{file} builds from source only when there is no verified adoption")
  end
end

# Only the read-only CI workflow may run on merge-queue refs: publication and
# deployment authority stays with develop pushes and explicit dispatches.
def check_merge_group_triggers(workflows)
  queued = workflows.select do |_name, workflow|
    events = workflow.fetch('on', workflow[true])
    events.is_a?(Hash) && events.key?('merge_group')
  end
  require_check(queued.keys == ['ci'], 'only CI may run on merge groups')
  ci = queued.fetch('ci')
  require_check(ci['permissions'] == STAGING_READ_ONLY &&
    ci.fetch('jobs').values.all? { |job| job.fetch('permissions', {}).values.all? { |grant| grant == 'read' } },
    'merge-group CI must remain read-only')
end

# Image builds pin cma-omarchy-packages by digest; the retention contract and
# its regressions refuse any .github path that deletes a version of it.
OMARCHY_RETENTION_STEPS = {
  'Omarchy package bundle retention regressions' => 'ruby .github/scripts/omarchy-package-retention-tests.rb',
  'Omarchy package bundle is never deleted' => 'ruby .github/scripts/omarchy-package-retention.rb'
}.freeze

# pacman refuses a pinned package once the key that signed it has expired, so
# each Omarchy build first warns before, and fails by name after, that date
# (#3040). The step stays offline, reading the committed signing-key record,
# and warn-only until expiry; the weekly omarchy-key-expiry workflow fails
# earlier and re-derives the record from the locked URLs.
KEY_EXPIRY_STEP = {
  'name' => 'Arch packager key expiry',
  'run' => "python3 -B .github/scripts/omarchy_key_expiry_tests.py\npython3 -B .github/scripts/omarchy_key_expiry.py\n"
}.freeze

def check_key_expiry(steps)
  found = steps.select { |s| s['name'] == KEY_EXPIRY_STEP['name'] }
  build = steps.index { |s| s['id'] == 'build' }
  require_check(found == [KEY_EXPIRY_STEP] && build && steps.index(found.first) < build,
    'Omarchy must check its packager key expiry offline, warn-only, unconditionally and before building')
end

# Owner-approved on 2026-11-24 as a deliberate gate change: merge-queue
# ordinary Agent Manager Mongo shards retry a failing test once, until one
# fixed UTC date. The policy step is the only source of a nonzero count; PR
# and push runs and the isolated shard skip it and pass an empty count. Every
# pass on retry is reported and retained for the tracker's single issue.
FLAKY_UNTIL = 'CMA_MONGO_FLAKY_RETRY_UNTIL'
FLAKY_REGRESSIONS = "python3 -B -m unittest discover -s .github/scripts -p 'mongo_flaky_retry_tests.py'"
FLAKY_POLICY_IF = "github.event_name == 'merge_group' && matrix.shard != 'isolated'"
FLAKY_STEPS = [
  {'name' => 'Merge-queue flaky retry policy', 'id' => 'flaky-retry', 'if' => FLAKY_POLICY_IF, 'working-directory' => '.',
   'env' => {'EVENT_NAME' => '${{ github.event_name }}', 'TRACK' => '${{ inputs.track }}', 'MONGO_SHARD' => '${{ matrix.shard }}'},
   'run' => 'python3 -B .github/scripts/mongo_flaky_retry.py policy'},
  {'name' => 'Disposable real Mongo persistence contracts', 'id' => 'contracts', 'timeout-minutes' => 75,
   'env' => {'MONGO_SHARD' => '${{ matrix.shard }}', 'CMA_MONGO_FLAKY_RETRIES' => '${{ steps.flaky-retry.outputs.retries }}',
             'CMA_MONGO_FLAKY_REPORT_DIR' => '${{ steps.flaky-retry.outputs.report-dir }}'},
   'run' => 'bash ../.github/scripts/mongo-contract-tests.sh --shard "$MONGO_SHARD"'},
  {'name' => 'Flag tests that passed only on their retry', 'id' => 'flaky-report',
   'if' => "always() && steps.flaky-retry.outputs.retries == '1'", 'timeout-minutes' => 5, 'working-directory' => '.',
   'env' => {'EVENT_NAME' => '${{ github.event_name }}', 'MONGO_SHARD' => '${{ matrix.shard }}',
             'TESTS_OUTCOME' => '${{ steps.contracts.outcome }}', 'CMA_MONGO_FLAKY_REPORT_DIR' => '${{ steps.flaky-retry.outputs.report-dir }}'},
   'run' => 'python3 -B .github/scripts/mongo_flaky_retry.py report'},
  {'name' => 'Retain the flaky test record for the tracking issue', 'if' => "always() && steps.flaky-report.outputs.upload == 'true'",
   'uses' => 'actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9',
   'with' => {'name' => '${{ steps.flaky-retry.outputs.artifact }}', 'path' => '${{ steps.flaky-retry.outputs.report-dir }}/flaky-tests.json',
              'retention-days' => 3, 'if-no-files-found' => 'error'}}
].freeze

def check_mongo_flaky_retry(documents, shards)
  date = shards.fetch('env')[FLAKY_UNTIL]
  valid = date.is_a?(String) && date.match?(/\A[0-9]{4}-[0-9]{2}-[0-9]{2}\z/) && (Date.iso8601(date) rescue false)
  require_check(valid, 'merge-queue flaky retry needs exactly one fixed YYYY-MM-DD expiry')
  require_check(shards['permissions'] == {'contents' => 'read', 'actions' => 'read'}, 'Mongo shards must stay read-only')
  steps = shards.fetch('steps')
  archive = steps.index { |s| s['uses'] == './.github/actions/rust-archive' }
  require_check(archive && steps.drop(archive + 1) == FLAKY_STEPS, 'Mongo shard execution or its merge-queue flaky retry changed')
  # The count and expiry belong to this job alone: no other job, nextest
  # environment or command line may retry a test.
  serialized = JSON.generate(documents)
  require_check(serialized.scan(FLAKY_UNTIL).length == 1 && serialized.scan('steps.flaky-retry.outputs.retries').length == 2 &&
    serialized.scan('CMA_MONGO_FLAKY_RETRIES').length == 1 && !serialized.match?(/NEXTEST_RETRIES|NEXTEST_FLAKY_RESULT|--retries|--flaky-result/),
    'only the merge-queue Mongo shards may retry a test')
  regressions = documents.dig('ci-repo', 'jobs', 'structure', 'steps').find { |s| s['name'] == 'Track selection and workflow regressions' }
  require_check(regressions && !regressions.key?('if') && !regressions.key?('continue-on-error') &&
    regressions['run'].lines.map(&:strip).include?(FLAKY_REGRESSIONS), 'merge-queue flaky retry regressions must run in Repository checks')
end

# CI keeps contents: read and issues: read, so the tracking issue is written
# only by this scheduled collector, from develop, never from PR code.
TRACKER = 'mongo-flaky-tracker'
TRACKER_JOB = {
  'name' => 'Track tests retried in the merge queue',
  'if' => "github.repository == 'ChronoAIProject/cma' && github.ref == 'refs/heads/develop'",
  'runs-on' => 'ubuntu-24.04', 'timeout-minutes' => 10,
  'permissions' => {'contents' => 'read', 'actions' => 'read', 'issues' => 'write'},
  'steps' => [
    {'uses' => 'actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09', 'with' => {'persist-credentials' => false}},
    {'name' => 'Collect retried tests into the tracking issue', 'timeout-minutes' => 8,
     'env' => {'GH_TOKEN' => '${{ github.token }}'}, 'run' => 'python3 -B .github/scripts/mongo_flaky_tracker.py'}
  ]
}.freeze

def check_flaky_tracker(workflows)
  tracker = workflows.fetch(TRACKER)
  events = tracker.fetch('on', tracker[true])
  require_check(events == {'schedule' => [{'cron' => '23 * * * *'}], 'workflow_dispatch' => {}},
    'flaky tracker runs only hourly or on dispatch')
  require_check(tracker['permissions'] == {} && tracker['concurrency'] == {'group' => TRACKER, 'cancel-in-progress' => false} &&
    tracker.fetch('jobs') == {'track' => TRACKER_JOB}, 'flaky tracker authority changed')
  writers = workflows.select do |_, workflow|
    [workflow['permissions'], *workflow.fetch('jobs').values.map { |job| job['permissions'] }].any? do |grant|
      grant == 'write-all' || (grant.is_a?(Hash) && grant['issues'] == 'write')
    end
  end
  require_check(writers.keys == [TRACKER], 'only the flaky tracker may write issues')
end

def validate(documents, actions)
  KeyProviderCIContract.check(documents)
  require_repo_secret_shards(documents)
  require_runtime_inventory(documents)
  require_image_secret_scan(documents)
  require_runtime_vulnerability_gate(documents)
  check_web_result(documents)
  check_web_jobs(documents)
  check_component_cache_writers(documents)
  check_merge_group_artifacts(documents)
  check_staging_image_adoption(documents)
  ci = documents.fetch('ci')
  events = ci.fetch('on', ci[true])
  require_check(events.keys.sort == %w[merge_group push schedule workflow_dispatch], 'post-merge CI must cover pushes and verification events')
  require_check(events['schedule'] == [{'cron' => '17 */2 * * *'}] && events['workflow_dispatch'] == {},
    'develop verification must use the scheduled or manually selected GitHub revision')
  require_check(events['push'] == {'branches' => ['develop', 'tl/release/**']}, 'CI push must cover develop and isolated candidates without path filters')
  # Without a branch or path filter every queue entry creates the required
  # checks; a non-develop queue is refused by the selector, not left pending.
  require_check(events['merge_group'] == {'types' => ['checks_requested']}, 'CI must report on every merge group')
  # Read-only: issues is read for the runtime vulnerability gate's tracking-issue age.
  require_check(ci['permissions'] == STAGING_READ_ONLY, 'CI permissions must remain read-only')
  %w[ci-images ci-omarchy].each do |called|
    require_check(documents.fetch(called)['permissions'] == STAGING_READ_ONLY,
                  "#{called} permissions must remain read-only and within the caller's")
  end
  # Queue entries run on refs/heads/gh-readonly-queue/develop/pr-N-SHA: their
  # own ref, so a PR (refs/pull/N/merge) or develop push run never cancels one.
  verification = "(github.event_name == 'schedule' || github.event_name == 'workflow_dispatch') && 'verification-' || ''"
  require_check(ci['concurrency'] == {
    'group' => "ci-${{ github.workflow }}-${{ #{verification} }}${{ github.ref }}",
    'cancel-in-progress' => "${{ github.ref != 'refs/heads/develop' }}"},
    'CI must protect active develop runs and isolate verification from pushes')
  jobs = ci.fetch('jobs')
  verification_ref = "(github.event_name != 'schedule' && github.event_name != 'workflow_dispatch') || github.ref == 'refs/heads/develop'"
  require_check(%w[changes repo].all? { |name| jobs.fetch(name)['if'] == verification_ref },
    'off-develop verification may not start selection or repo checks')
  require_check(jobs.keys.none? { |key| key.start_with?('trigger-') }, 'Trigger job retained')
  required = {
    'gate-repo' => ['CI / Repo', %w[changes repo version-label]],
    'gate-am' => ['CI / Agent Manager', %w[changes rust-test-build approval-contract am am-images runtime-images omarchy two-am-features]],
    'gate-eg' => ['CI / Event Gateway', %w[changes rust-test-build eg eg-images runtime-images omarchy]],
    'gate-web' => ['CI / Web', %w[changes web web-images]]
  }
  required.each do |key, (name, dependencies)|
    job = jobs.fetch(key)
    require_check(job.fetch('name') == name && job.fetch('if') == "always() && (#{verification_ref})", 'stable develop CI gate or ref guard missing')
    require_check(job.fetch('needs').sort == dependencies.sort, 'gate dependency omitted')
    require_check(job.fetch('steps').any? { |step| step.fetch('run', '').include?('ci_tracks/gate.py') }, 'gate validator missing')
  end
  %w[repo am eg web].each_with_index do |track, index|
    workflow = documents.fetch("ci-pr-#{track}")
    events = workflow.fetch('on', workflow[true])
    pr = events.fetch('pull_request')
    require_check(events.keys == ['pull_request'] && pr.keys.sort == %w[branches types],
      'PR product workflows must run without path or extra event filters')
    require_check(pr.fetch('branches') == %w[develop main], 'CI target differs')
    require_check(pr.fetch('types').sort == %w[opened reopened synchronize edited labeled unlabeled].sort,
      'label event coverage differs')
    number = index + 1
    track_jobs = workflow.fetch('jobs')
    gate_key = "gate-#{track}"
    check_name, prerequisites = required.fetch(gate_key)
    prerequisites -= %w[runtime-images omarchy] if track == 'eg'
    prerequisites += ['eg-fixture-image'] if track == 'am'
    selector = track_jobs.fetch('changes')
    require_check(track_jobs.keys.first == 'changes' && selector['name'] == "#{number}.1 Select changes" &&
      selector['uses'] == './.github/workflows/ci-track-selection.yml', 'each workflow selects before validating')
    require_check(track_jobs.keys.sort == (prerequisites + [gate_key]).sort,
      'product validation task missing or duplicated')
    gate = track_jobs.fetch(gate_key)
    require_check(gate['name'] == check_name && gate['if'] == 'always()' && gate['needs'].sort == prerequisites.sort &&
      gate.fetch('steps').last.fetch('run') == "python3 -B .github/scripts/ci_tracks/gate.py #{track} --split",
      'product result must use its original required context and check every prerequisite')
    if %w[am eg].include?(track)
      archive_condition = "needs.changes.outputs.#{track} == 'true'"
      archive_condition += " || needs.changes.outputs.approval_contract == 'true'" if track == 'am'
      require_check(track_jobs.fetch('rust-test-build')['if'] == archive_condition,
        'each Rust product archive must be built in its own selected workflow')
    end
    if track == 'am'
      %w[runtime-images omarchy].each do |key|
        require_check(track_jobs.fetch(key).fetch('if') == "needs.changes.outputs.am == 'true' || needs.changes.outputs.eg == 'true'",
          'Agent Manager must validate runtime images for changes to either Rust product')
      end
    end
  end
  select_workflow = documents.fetch('ci-track-selection')
  require_check(select_workflow.fetch('jobs').fetch('changes') == jobs.fetch('changes').reject { |key, _| key == 'if' },
    'pull requests and post-merge CI must use identical selection')
  two_am = documents.fetch('ci-two-am')
  two_am_permissions = {'contents' => 'read', 'actions' => 'read'}
  require_check(two_am['permissions'] == two_am_permissions &&
    two_am.fetch('on', two_am[true]).keys == ['workflow_call'], 'two-AM fixture must remain a read-only reusable workflow')
  feature_steps = two_am.fetch('jobs').fetch('features').fetch('steps')
  browser_build = feature_steps.find { |step| step['working-directory'] == 'web' }
  require_check(browser_build && !browser_build.key?('if') && !browser_build.key?('continue-on-error') &&
    browser_build.dig('env', 'CHRONO_BUILD_SHA') == '${{ github.sha }}' &&
    browser_build.fetch('run', '').lines.map(&:strip) == ['npm ci --no-audit --no-fund', 'npm run build',
      'npm exec -- playwright install --with-deps chromium'],
    'both two-AM fixtures must build the checked-out Web and install Chromium')
  startup = two_am.fetch('jobs').fetch('startup-controls')
  startup_steps = startup.fetch('steps')
  require_check(!startup.key?('if') && !startup.key?('continue-on-error') &&
    startup.dig('env', 'CMA_NEXTEST_REUSE') == '1' &&
    startup_steps[1]['uses'] == './.github/actions/rust-archive' &&
    startup_steps[2]['run'] == 'bash .github/scripts/mongo-contract-tests.sh --prepared-startup' &&
    !startup_steps[2].key?('if') && !startup_steps[2].key?('continue-on-error'),
    'two-AM startup controls must run from the shared Rust archive')
  [[jobs, 'eg-images'], [documents.fetch('ci-pr-am').fetch('jobs'), 'eg-fixture-image']].each do |caller, gateway_image|
    suite = caller.fetch('two-am-features')
    require_check(suite['uses'] == './.github/workflows/ci-two-am.yml' &&
      suite['permissions'] == two_am_permissions &&
      suite['needs'] == %w[changes rust-test-build am-images runtime-images omarchy] + [gateway_image] &&
      suite['if'] == "!cancelled() && needs.changes.outputs.two_am_features == 'true'" && !suite.key?('continue-on-error') &&
      suite['with'] == {'am_image_result' => '${{ needs.am-images.result }}',
                        'runtime_image_result' => '${{ needs.runtime-images.result }}',
                        'omarchy_image_result' => '${{ needs.omarchy.result }}',
                        'eg_image_result' => "\${{ needs.#{gateway_image}.result }}"},
                  'two-AM verification must use its own archive, exact images and trusted CI events')
  end
  check_version_label(ci, "!cancelled() && (#{verification_ref})")
  check_version_label(documents.fetch('ci-pr-repo'))
  selector = jobs.fetch('changes').fetch('steps').find { |step| step['id'] == 'select' }
  require_check(selector && selector['run'] == 'python3 -B .github/scripts/ci_tracks/selection.py --ci' &&
    !selector.key?('if') && !selector.key?('continue-on-error'), 'CI-specific selection must be required')
  %w[am eg web].each do |track|
    [track, "#{track}-images"].each do |key|
      expected = "needs.changes.outputs.#{track} == 'true'"
      expected += " || needs.changes.outputs.two_am_features == 'true'" if key == 'eg-images'
      require_check(jobs.fetch(key)['if'] == expected, 'product selection differs')
    end
  end
  %w[am eg].each do |track|
    require_check(jobs.fetch(track)['with'] == {'track' => track, 'packages' => "${{ needs.changes.outputs.#{track}_packages }}"},
      'selected Rust packages must reach the test workflow')
  end
  %w[deployables test_tracks approval_contract].each do |key|
    require_check(jobs.fetch('changes').fetch('outputs').key?(key), 'shared selector output absent')
    if key == 'approval_contract'
      require_check(select_workflow.dig('on', 'workflow_call', 'outputs', key, 'value') == '${{ jobs.changes.outputs.approval_contract }}' ||
        select_workflow.dig(true, 'workflow_call', 'outputs', key, 'value') == '${{ jobs.changes.outputs.approval_contract }}',
        'PR selection must expose the approval contract decision')
    end
  end
  %w[runtime-images omarchy].each do |key|
    require_check(jobs.fetch(key).fetch('if') == "needs.changes.outputs.am == 'true' || needs.changes.outputs.eg == 'true'", 'shared Rust prerequisite selection differs')
  end
  require_check(jobs.fetch('rust-test-build').fetch('if') ==
    "needs.changes.outputs.am == 'true' || needs.changes.outputs.eg == 'true' || needs.changes.outputs.approval_contract == 'true'",
    'approval contract must select the shared Rust test archive')
  [jobs, documents.fetch('ci-pr-am').fetch('jobs')].each do |workflow_jobs|
    contract = workflow_jobs.fetch('approval-contract')
    require_check(contract.fetch('needs') == %w[changes rust-test-build] &&
      contract['uses'] == './.github/workflows/ci-approval-contract.yml' &&
      contract['if'] == "needs.changes.outputs.approval_contract == 'true'" &&
      contract['permissions'] == {'contents' => 'read', 'actions' => 'read'} &&
      !contract.key?('continue-on-error'), 'approval contract must run once against the selected archive')
  end
  contract = documents.fetch('ci-approval-contract').fetch('jobs').fetch('contract')
  step = contract.fetch('steps').last
  require_check(contract.fetch('env')['CMA_NEXTEST_REUSE'] == '1' &&
    contract.fetch('steps').any? { |s| s['uses'] == './.github/actions/rust-archive' } &&
    step['working-directory'] == 'web' && step['run'] == "npm ci\nnpm run test:approval-contract\n" &&
    !step.key?('if') && !step.key?('continue-on-error'), 'archived producer-to-Web test required')
  rust = documents.fetch('ci-rust').fetch('jobs')
  require_check(rust.fetch('tests').fetch('strategy') == {'fail-fast' => false, 'matrix' => {'shard' => [1, 2, 3]}}, 'three test shards required')
  require_check(rust.fetch('result').fetch('if') == 'always()', 'reusable failure propagation missing')
  require_check(rust.fetch('result').fetch('needs').sort == %w[lint tests timing mongo isolation].sort, 'Rust child omitted')
  mongo = rust.fetch('mongo')
  require_check(mongo.fetch('name') == 'Mongo contracts' && mongo.fetch('if') == 'always()', 'stable Mongo gate missing')
  require_check(mongo.fetch('needs').sort == %w[mongo-harness mongo-shards mongo-eg].sort, 'Mongo prerequisite omitted')
  gate = mongo.fetch('steps').last
  require_check(gate['run'] == 'python3 -B .github/scripts/mongo-contract-gate.py' &&
    gate['env'] == {'TRACK' => '${{ inputs.track }}', 'NEEDS' => '${{ toJSON(needs) }}'} &&
    !gate.key?('if') && !gate.key?('continue-on-error'), 'Mongo gate bypass')
  shards = rust.fetch('mongo-shards')
  require_check(shards.fetch('strategy') == {'fail-fast' => false,
    'matrix' => {'shard' => ['isolated'] + (1..8).map { |i| "#{i}/8" }}}, 'complete Mongo matrix required')
  require_check(shards.fetch('env').reject { |key, _| key == FLAKY_UNTIL } == {'BASH_ENV' => '${{ github.workspace }}/.github/scripts/backend-resource-env.sh',
    'RUST_MIN_STACK' => 33554432, 'CMA_NEXTEST_REUSE' => '1'}, 'Mongo resource or archive guard omitted')
  require_check(shards.fetch('defaults') == {'run' => {'working-directory' => 'api'}}, 'Mongo workspace differs')
  require_check(shards.fetch('steps').any? { |s| s['uses'] == './.github/actions/rust-archive' }, 'Mongo archive reuse omitted')
  check_mongo_flaky_retry(documents, shards)
  %w[mongo mongo-harness mongo-shards mongo-eg].each do |id|
    require_check(rust[id]['runs-on'] == 'ubuntu-latest' && !rust[id].key?('continue-on-error') &&
      !rust[id].key?('environment'), 'Mongo runner or mandatory execution differs')
    require_pinned_actions(rust[id].fetch('steps'), actions)
  end
  %w[mongo-harness mongo-shards].each { |id| require_check(rust[id]['if'] == "inputs.track == 'am'", 'AM Mongo selection differs') }
  require_check(rust['mongo-eg']['if'] == "inputs.track == 'eg'", 'EG Mongo selection differs')
  require_check(rust['mongo-eg']['steps'].any? { |s| s['run'] == 'bash .github/scripts/authenticated-mongo-contract.sh eg' }, 'Gateway contracts omitted')
  require_check(rust['mongo-harness']['steps'].last['run'].include?('node --test .github/scripts/mongo-contract-tests.test.mjs') &&
    rust['mongo-harness']['steps'].last['run'].include?('python3 -B .github/scripts/mongo-contract-shards-tests.py'), 'Mongo regressions omitted')
  timing = rust.fetch('timing').fetch('steps')
  ['Image normalization budgets', 'Gateway socket latency budget', 'Enforce Gateway latency budgets'].each do |name|
    require_check(timing.find { |s| s['name'] == name }&.fetch('if') == "inputs.track == 'eg'", 'Gateway timing ownership differs')
  end
  budget = timing.select { |s| s['name'] == 'Enforce Gateway latency budgets' }
  require_check(budget.size == 1 && budget[0]['run'] == 'python3 -B ../.github/scripts/eg-latency-check.py' &&
    !budget[0].key?('continue-on-error'), 'Gateway latency must retain its first invocation result')
  regressions = timing.select { |s| s['run'] == 'ruby ../.github/scripts/eg-latency-check-tests.rb' }
  require_check(regressions.size == 1 && regressions[0]['if'] == "inputs.track == 'eg'" &&
    !regressions[0].key?('continue-on-error'), 'Gateway latency runner regressions must execute')
  reports = timing.select { |s| s.dig('with', 'name').to_s.start_with?('cmaeg-latency-') }
  require_check(reports.size == 1 && reports[0]['if'] == "always() && inputs.track == 'eg'" &&
    reports[0].dig('with', 'path') == 'api/target/cmaeg-latency-results/' && reports[0].dig('with', 'if-no-files-found') == 'error' &&
    reports[0].dig('with', 'name') == 'cmaeg-latency-${{ github.run_id }}-${{ github.run_attempt }}', 'Gateway attempt reports must be retained')
  allocator_selector = 'package(=cma) and binary(=allocator_cli) and test(=actual_api_allocator_completes_without_configuration)'
  allocator = timing.select { |s| s.fetch('run', '').include?(allocator_selector) }
  require_check(allocator.length == 1 && allocator[0]['if'] == "inputs.track == 'am'" &&
    allocator[0]['run'] == "bash ../.github/scripts/backend-nextest.sh run -E '#{allocator_selector}' --test-threads 1 --success-output final" &&
    !allocator[0].key?('continue-on-error'), 'API allocator must run exactly once in required sequential AM timing steps')
  require_check(rust.fetch('timing').fetch('env')['CMA_NEXTEST_REUSE'] == '1', 'Allocator must use the shared Rust archive')
  steps = documents.values.flat_map { |doc| doc.fetch('jobs').values.flat_map { |job| job.fetch('steps', []) } }
  archives = steps.select { |s| s.fetch('run', '').include?('cargo nextest archive') }
  require_check(archives.length == 1 && archives[0]['run'].include?('--workspace --locked'), 'one locked workspace archive required')
  uploads = steps.select { |s| s.fetch('with', {})['name'] == 'rust-tests-${{ github.sha }}' }
  require_check(uploads.length == 1 && uploads[0]['with']['retention-days'] == 1, 'event SHA archive retention differs')
  structure = documents.fetch('ci-repo').fetch('jobs').fetch('structure').fetch('steps')
  restore = structure.select { |s| s['name'] == 'Synthetic home restore evidence regressions' }
  require_check(restore.size == 1 && restore.first['run'] == 'node --test .github/scripts/home-backup/*.test.mjs' &&
    !restore.first.key?('if') && !restore.first.key?('continue-on-error') && !restore.first.key?('env'),
    'home restore evidence suites must run without credentials or optional failure')
  OMARCHY_RETENTION_STEPS.each do |name, command|
    found = structure.select { |s| s['name'] == name }
    require_check(found.size == 1 && found.first == {'name' => name, 'run' => command},
      "#{name} must run unconditionally in Repository structure")
  end
  node = structure.index { |s| s.fetch('uses', '').start_with?('actions/setup-node@') }
  require_check(node && node < structure.index(restore.first), 'home restore evidence requires Node setup')
  require_check(structure.none? { |s| s.fetch('name', '').match?(/Release |Skopeo|image reconciliation|image import/i) }, 'release tooling leaked into Repo')
  serialized = JSON.generate(documents)
  require_check(!serialized.include?('continue-on-error":true,"run'), 'product gate bypass')
  synthetic = structure.select { |s| s['name'] == 'Synthetic authority and observer regressions' }
  command = 'node --test .github/scripts/staging-authority.test.mjs .github/scripts/public-journey-evidence.test.mjs .github/scripts/connection-journey-evidence.test.mjs .github/scripts/connection-campaign.test.mjs .github/scripts/deployment-observer.test.mjs'
  require_check(synthetic.size == 1 && synthetic.first['run'] == command &&
    !synthetic.first.key?('if') && !synthetic.first.key?('continue-on-error'), 'synthetic authority and observer suites must run')
  repo_job = documents.fetch('ci-repo').fetch('jobs').fetch('structure')
  require_check(!repo_job.key?('if') && !repo_job.key?('continue-on-error'), 'Repository structure must be required')
  require_check(!serialized.match?(%r{ci-contract\.rb|staging-authority\.mjs|deployment-observer\.mjs|gh workflow run|gh api[^\n]*dispatches|kubectl}),
    'live authority, observer or mutation tooling retained')
  check_omarchy_archive(documents.fetch('ci-omarchy').fetch('jobs').fetch('omarchy').fetch('steps'))
  check_key_expiry(documents.fetch('ci-omarchy').fetch('jobs').fetch('omarchy').fetch('steps'))
  check_os_refresh(documents.fetch('ci-omarchy').fetch('jobs').fetch('omarchy').fetch('steps'), 'Omarchy')
  check_os_refresh(documents.fetch('ci-images').fetch('jobs').fetch('images').fetch('steps'), 'Images')
end

# Runtime images install from one pinned Debian snapshot, so their OS refresh is
# keyed on that snapshot and their package layers come from cache until it is
# bumped. A per-run key here would rebuild them against the archive every run.
def check_os_refresh(steps, label)
  key = steps.find { |s| s['id'] == 'os-refresh' }
  builds = steps.select { |s| s.fetch('uses', '').start_with?('docker/build-push-action@') }
  args = builds.map { |b| b.fetch('with').fetch('build-args', '').lines.map(&:strip).grep(/\AOS_SECURITY_REFRESH=/) }
  require_check(key && key['run'].to_s.start_with?('python3 .github/scripts/os_refresh_key.py ') &&
    key['if'] == STAGING_IMAGE_FALLBACK && !key.key?('continue-on-error') && !builds.empty? &&
    args.all? { |a| a == ['OS_SECURITY_REFRESH=${{ steps.os-refresh.outputs.value }}'] } &&
    steps.index(key) < steps.index(builds.first),
    "#{label} image builds must take OS_SECURITY_REFRESH from the snapshot-aware refresh key")
end

def check_omarchy_archive(steps)
  restore = steps.find { |s| s['id'] == 'omarchy-cache-restore' }
  build = steps.find { |s| s['id'] == 'build' }.fetch('with')
  epochs = build.fetch('build-args', '').lines.map(&:strip).grep(/\ASOURCE_DATE_EPOCH=/)
  require_check(build['outputs'] == 'type=docker,rewrite-timestamp=true' && epochs == ['SOURCE_DATE_EPOCH=0'],
    'Omarchy must rewrite layer timestamps to the fixed epoch')
  storage = {'path' => OmarchyCIContract::CACHE_PATH, 'key' => OmarchyCIContract::CACHE_KEY}
  require_check(restore && restore['if'] == STAGING_IMAGE_FALLBACK && restore['uses'].start_with?('actions/cache/restore@') &&
    restore['with'] == storage.merge('fail-on-cache-miss' => false), 'PRs must restore only an exact Omarchy archive')
  hit = "steps.omarchy-cache-restore.outcome == 'success' && steps.omarchy-cache-restore.outputs.cache-hit == 'true'"
  protected_cache = "${{ #{OmarchyCIContract::DEVELOP_PUSH} }}"
  {'actions/setup-node' => ['package-manager-cache', false],
   'docker/setup-buildx-action' => ['cache-binary', protected_cache],
   'aquasecurity/trivy-action' => ['cache', protected_cache]}.each do |action, (key, expected)|
    uses = steps.select { |s| s.fetch('uses', '').start_with?(action + '@') }
    require_check(!uses.empty? && uses.all? { |s| s.dig('with', key) == expected &&
      (action != 'actions/setup-node' || !s['with'].key?('cache')) }, "Omarchy #{action} may save a PR cache")
  end
  db = steps.find { |s| s['name'] == 'Restore Trivy databases' }
  require_check(db && db['uses'].start_with?('actions/cache/restore@') &&
    db['if'] == "github.event_name != 'push' || github.ref != 'refs/heads/develop'" &&
    db.dig('with', 'restore-keys') == 'cache-trivy-', 'Omarchy PR scans must restore the Trivy database')
  bounded = "${{ steps.layer-validate.outcome == 'success' && format('type=local,src={0}/cma-layer-cache/restored', runner.temp) || '' }}"
  require_check(build['cache-from'].to_s.lines.map(&:strip) == ["${{ #{hit} && format('type=local,src={0}/omarchy-layer-cache', runner.temp) || '' }}", bounded],
    'a partial restore must build Omarchy cold')
  require_check(!build.key?('cache-to'), 'Omarchy layers use the bounded publication archive')
  writers = steps.select { |s| s.fetch('uses', '').match?(%r{\Aactions/cache(?:/save)?@}) }
  require_check(writers.empty?, 'Omarchy CI may only restore its layer archive')
end

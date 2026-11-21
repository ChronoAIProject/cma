#!/usr/bin/env ruby
require 'yaml'
require 'json'
require_relative 'omarchy-ci-contract'
root = File.expand_path('../..', __dir__)


def require_check(value, message)
  raise message unless value
end

def require_runtime_inventory(documents)
  [['ci-images', 'images', 'runtime', 'cma-runtime:ci', "matrix.component == 'runtime'"],
   ['ci-omarchy', 'omarchy', 'runtime-omarchy', 'cma-runtime-omarchy:ci', nil]].each do |file, job, component, image, condition|
    steps = documents.fetch(file).fetch('jobs').fetch(job).fetch('steps')
    gates = steps.select { |step| step.fetch('run', '').include?('runtime_image_hygiene.py --image') }
    require_check(gates.length == 1, 'one mandatory runtime layer inventory required')
    gate = gates.first
    require_check(gate['if'] == condition && !gate['continue-on-error'], 'runtime layer inventory is optional')
    report = component == 'runtime' ? 'runtime-layer-hygiene.json' : 'omarchy-evidence/layer-hygiene.json'
    command = "python3 -B .github/scripts/runtime_image_hygiene.py --image #{image} --component #{component} --output \"$RUNNER_TEMP/#{report}\""
    require_check(gate['run'] == command && gate['timeout-minutes'] == 20, 'runtime layer inventory command differs')
    export = steps.index { |step| step.fetch('run', '').include?('export-ci-native-image.py') }
    require_check(export && steps.index(gate) < export, 'runtime image exported before inventory')
  end
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

def check_component_cache_writers(documents)
  develop = "github.event_name == 'push' && github.ref == 'refs/heads/develop'"
  writer = "${{ #{develop} }}"
  images = documents.fetch('ci-images').fetch('jobs').fetch('images').fetch('steps')
  node_setup = ->(s) { s.fetch('uses', '').start_with?('actions/setup-node@') }
  image_nodes = images.select(&node_setup)
  require_check(!image_nodes.empty? && image_nodes.all? { |s|
    s.dig('with', 'package-manager-cache') == false && !s.fetch('with').key?('cache')
  }, 'image Node setup must disable automatic and explicit dependency caching')
  scope = "matrix.component == 'api' && 'cma-am' || matrix.component == 'cmaeg' && 'cma-eg' || matrix.component == 'web' && 'cma-web' || matrix.component"
  builds = images.select { |s| s.fetch('uses', '').start_with?('docker/build-push-action@') }
  require_check(builds.size == 2 && builds.all? { |s| s.dig('with', 'cache-from') == "type=gha,version=2,scope=${{ #{scope} }}" },
    'both image builds must restore the component cache')
  exports = builds.map { |s| s.dig('with', 'cache-to') }.compact
  require_check(exports == ["${{ #{develop} && format('type=gha,version=2,mode=max,scope={0}', #{scope}) || '' }}"],
    'image layers may be exported only on develop pushes')
  {'docker/setup-qemu-action' => 'cache-image', 'docker/setup-buildx-action' => 'cache-binary',
   'aquasecurity/trivy-action' => 'cache'}.each do |action, option|
    step = images.find { |s| s.fetch('uses', '').start_with?(action + '@') }
    require_check(step&.dig('with', option) == writer, "#{action} may save a PR cache")
  end
  web = documents.fetch('ci-web').fetch('jobs').fetch('web').fetch('steps')
  web_nodes = web.select(&node_setup)
  require_check(web_nodes.size == 1 && web_nodes.first.dig('with', 'package-manager-cache') == false &&
    web_nodes.first.dig('with', 'cache') == "${{ #{develop} && 'npm' || '' }}", 'setup-node may save a PR cache')
  writers = (images + web).select { |s| s.fetch('uses', '').match?(%r{\Aactions/cache(?:/save)?@}) }
  require_check(writers.size == 1 && writers.first['uses'].start_with?('actions/cache/save@') &&
    writers.first['if'] == "success() && #{develop} && matrix.component == 'api' && steps.api-cargo-cache-export.outputs.ready == 'true'",
    'only the accepted develop Cargo archive may be saved')
  restore = 'actions/cache/restore@0057852bfaa89a56745cba8c7296529d2fc39830'
  npm = {'path' => '${{ steps.npm-cache.outputs.directory }}',
    'key' => "node-cache-${{ runner.os }}-${{ steps.npm-cache.outputs.architecture }}-npm-${{ hashFiles('web/package-lock.json') }}"}
  database = {'path' => '${{ github.workspace }}/.cache/trivy', 'key' => 'cache-trivy-readonly', 'restore-keys' => 'cache-trivy-'}
  [[web, 'Restore npm download cache', 'Install (lockfile integrity)', npm],
   [images, 'Restore Trivy databases', 'Vulnerability scan', database]].each do |steps, name, consumer, inputs|
    index = steps.index { |s| s['name'] == name }
    run = steps.index { |s| s['name'] == consumer }
    require_check(index && run && index < run && steps[index]['uses'] == restore && steps[index]['with'] == inputs &&
      steps[index]['if'] == "github.event_name != 'push' || github.ref != 'refs/heads/develop'",
      "#{name} must restore without saving before #{consumer}")
  end
end

MERGE_GROUP_EXPORT = "success() && (github.event_name == 'push' || github.event_name == 'merge_group' || " \
  "github.event.pull_request.head.repo.full_name == github.repository)"
API_CACHE_READERS = "matrix.component == 'api' && ((github.event_name == 'push' && github.ref == 'refs/heads/develop') || " \
  "(github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository) || " \
  "github.event_name == 'merge_group')"

# A queue entry tests the exact commit develop becomes, so its tested images
# are retained for exact-source CD reuse; fork PR runs still retain nothing.
def check_merge_group_artifacts(documents)
  [['ci-images', 'images', ['Export tested native image', 'Retain tested native image']],
   ['ci-omarchy', 'omarchy', ['Export tested Omarchy image', 'Retain tested Omarchy image']]].each do |file, job, names|
    steps = documents.fetch(file).fetch('jobs').fetch(job).fetch('steps')
    names.each do |name|
      found = steps.select { |s| s['name'] == name }
      require_check(found.length == 1 && found.first['if'] == MERGE_GROUP_EXPORT && !found.first.key?('continue-on-error'),
        "#{name} must retain push, same-repository PR and merge-group images only")
    end
  end
  images = documents.fetch('ci-images').fetch('jobs').fetch('images').fetch('steps')
  prepare = images.select { |s| s['name'] == 'Prepare API Cargo cache identity' }
  require_check(prepare.length == 1 && prepare.first['if'] == API_CACHE_READERS,
    'queue entries may restore, never save, the develop API Cargo archive')
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
  require_check(ci['permissions'] == {'contents' => 'read'} &&
    ci.fetch('jobs').values.all? { |job| job.fetch('permissions', {}).values.all? { |grant| grant == 'read' } },
    'merge-group CI must remain read-only')
end

def validate(documents, actions)
  require_runtime_inventory(documents)
  check_component_cache_writers(documents)
  check_merge_group_artifacts(documents)
  ci = documents.fetch('ci')
  events = ci.fetch('on', ci[true])
  require_check(events.keys.sort == %w[merge_group pull_request push], 'CI must cover PRs, develop pushes and merge groups')
  require_check(events['push'] == {'branches' => ['develop']}, 'CI push must cover develop without path filters')
  # Without a branch or path filter every queue entry creates the required
  # checks; a non-develop queue is refused by the selector, not left pending.
  require_check(events['merge_group'] == {'types' => ['checks_requested']}, 'CI must report on every merge group')
  require_check(ci['permissions'] == {'contents' => 'read'}, 'CI permissions must remain read-only')
  # Queue entries run on refs/heads/gh-readonly-queue/develop/pr-N-SHA: their
  # own ref, so a PR (refs/pull/N/merge) or develop push run never cancels one.
  # Develop pushes write the caches PRs restore, so they finish instead of
  # being cancelled; every other ref still cancels its superseded run.
  require_check(ci['concurrency'] == {'group' => 'ci-${{ github.workflow }}-${{ github.ref }}',
    'cancel-in-progress' => "${{ !(github.event_name == 'push' && github.ref == 'refs/heads/develop') }}"},
    'CI concurrency must remain per ref and keep develop cache writers running')
  pr = events.fetch('pull_request')
  require_check(pr.fetch('branches') == %w[develop main], 'CI target differs')
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
  require_check(!version.key?('continue-on-error'), 'version job must propagate failure')
  label_check = version.fetch('steps').last
  require_check(label_check['run'] == 'python3 -B .github/scripts/ci_tracks/version_label.py' &&
    !label_check.key?('if') && !label_check.key?('continue-on-error'), 'version validator missing or bypassed')
  require_check(label_check.fetch('env') == {
    'DEPLOYABLES' => '${{ needs.changes.outputs.deployables }}',
    'SELECTOR_RESULT' => '${{ needs.changes.result }}', 'GH_TOKEN' => '${{ github.token }}'
  }, 'version selector or live label context differs')
  selector = jobs.fetch('changes').fetch('steps').find { |step| step['id'] == 'select' }
  require_check(selector && selector['run'] == 'python3 -B .github/scripts/ci_tracks/selection.py --ci' &&
    !selector.key?('if') && !selector.key?('continue-on-error'), 'CI-specific selection must be required')
  %w[am eg web].each do |track|
    [track, "#{track}-images"].each do |key|
      require_check(jobs.fetch(key)['if'] == "needs.changes.outputs.#{track} == 'true'", 'product selection differs')
    end
  end
  %w[am eg].each do |track|
    require_check(jobs.fetch(track)['with'] == {'track' => track, 'packages' => "${{ needs.changes.outputs.#{track}_packages }}"},
      'selected Rust packages must reach the test workflow')
  end
  %w[deployables test_tracks].each { |key| require_check(jobs.fetch('changes').fetch('outputs').key?(key), 'shared selector output absent') }
  %w[runtime-images omarchy rust-test-build].each do |key|
    require_check(jobs.fetch(key).fetch('if') == "needs.changes.outputs.am == 'true' || needs.changes.outputs.eg == 'true'", 'shared Rust prerequisite selection differs')
  end
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
  require_check(shards.fetch('env') == {'BASH_ENV' => '${{ github.workspace }}/.github/scripts/backend-resource-env.sh',
    'RUST_MIN_STACK' => 33554432, 'CMA_NEXTEST_REUSE' => '1'}, 'Mongo resource or archive guard omitted')
  require_check(shards.fetch('defaults') == {'run' => {'working-directory' => 'api'}}, 'Mongo workspace differs')
  require_check(shards.fetch('steps').any? { |s| s['uses'] == './.github/actions/rust-archive' }, 'Mongo archive reuse omitted')
  execution = shards.fetch('steps').last
  require_check(execution['run'] == 'bash ../.github/scripts/mongo-contract-tests.sh --shard "$MONGO_SHARD"' &&
    execution['env'] == {'MONGO_SHARD' => '${{ matrix.shard }}'} && execution['timeout-minutes'] == 75 &&
    !execution.key?('if') && !execution.key?('continue-on-error'), 'Mongo shard execution changed')
  %w[mongo mongo-harness mongo-shards mongo-eg].each do |id|
    require_check(rust[id]['runs-on'] == 'ubuntu-latest' && !rust[id].key?('continue-on-error') &&
      !rust[id].key?('environment'), 'Mongo runner or mandatory execution differs')
    require_pinned_actions(rust[id].fetch('steps'), actions)
  end
  %w[mongo-harness mongo-shards].each { |id| require_check(rust[id]['if'] == "inputs.track == 'am'", 'AM Mongo selection differs') }
  require_check(rust['mongo-eg']['if'] == "inputs.track == 'eg'", 'EG Mongo selection differs')
  require_check(rust['mongo-eg']['steps'].any? { |s| s['run'] == 'bash .github/scripts/image-manager-mongo-contract.sh eg' }, 'Gateway contracts omitted')
  require_check(rust['mongo-harness']['steps'].last['run'].include?('node --test .github/scripts/mongo-contract-tests.test.mjs') &&
    rust['mongo-harness']['steps'].last['run'].include?('python3 -B .github/scripts/mongo-contract-shards-tests.py'), 'Mongo regressions omitted')
  timing = rust.fetch('timing').fetch('steps')
  ['Image normalization budgets', 'Gateway socket latency budget', 'Enforce the production budgets (one retry)'].each do |name|
    require_check(timing.find { |s| s['name'] == name }&.fetch('if') == "inputs.track == 'eg'", 'Gateway timing ownership differs')
  end
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
  node = structure.index { |s| s.fetch('uses', '').start_with?('actions/setup-node@') }
  require_check(node && node < structure.index(restore.first), 'home restore evidence requires Node setup')
  require_check(structure.none? { |s| s.fetch('name', '').match?(/Release |Skopeo|image reconciliation|image import/i) }, 'release tooling leaked into Repo')
  serialized = JSON.generate(documents)
  require_check(!serialized.include?('continue-on-error":true,"run'), 'product gate bypass')
  synthetic = structure.select { |s| s['name'] == 'Synthetic authority and observer regressions' }
  command = 'node --test .github/scripts/staging-authority.test.mjs .github/scripts/deployment-observer.test.mjs'
  require_check(synthetic.size == 1 && synthetic.first['run'] == command &&
    !synthetic.first.key?('if') && !synthetic.first.key?('continue-on-error'), 'synthetic authority and observer suites must run')
  repo_job = documents.fetch('ci-repo').fetch('jobs').fetch('structure')
  require_check(!repo_job.key?('if') && !repo_job.key?('continue-on-error'), 'Repository structure must be required')
  require_check(!serialized.match?(%r{ci-contract\.rb|staging-authority\.mjs|deployment-observer\.mjs|gh workflow run|gh api[^\n]*dispatches|kubectl}),
    'live authority, observer or mutation tooling retained')
  check_omarchy_archive(documents.fetch('ci-omarchy').fetch('jobs').fetch('omarchy').fetch('steps'))
end

def check_omarchy_archive(steps)
  restore = steps.find { |s| s['id'] == 'omarchy-cache-restore' }
  build = steps.find { |s| s['id'] == 'build' }.fetch('with')
  epochs = build.fetch('build-args', '').lines.map(&:strip).grep(/\ASOURCE_DATE_EPOCH=/)
  require_check(build['outputs'] == 'type=docker,rewrite-timestamp=true' && epochs == ['SOURCE_DATE_EPOCH=0'],
    'Omarchy must rewrite layer timestamps to the fixed epoch')
  save = steps.find { |s| s['name'] == 'Save qualified Omarchy layer cache' }
  storage = {'path' => OmarchyCIContract::CACHE_PATH, 'key' => OmarchyCIContract::CACHE_KEY}
  require_check(restore && !restore.key?('if') && restore['uses'].start_with?('actions/cache/restore@') &&
    restore['with'] == storage.merge('fail-on-cache-miss' => false), 'PRs must restore only an exact Omarchy archive')
  hit = "steps.omarchy-cache-restore.outcome == 'success' && steps.omarchy-cache-restore.outputs.cache-hit == 'true'"
  writer = OmarchyCIContract::CACHE_WRITE
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
  require_check(build['cache-from'].to_s.strip == "${{ #{hit} && format('type=local,src={0}/omarchy-layer-cache', runner.temp) || '' }}",
    'a partial restore must build Omarchy cold')
  require_check(build['cache-to'] == "${{ #{writer} && format('type=local,mode=min,dest={0}/omarchy-layer-cache', runner.temp) || '' }}",
    'Omarchy may export new layers only on a develop push with a cache miss')
  writers = steps.select { |s| s.fetch('uses', '').match?(%r{\Aactions/cache(?:/save)?@}) }
  require_check(save && writers == [save] && save['uses'].start_with?('actions/cache/save@') && save['with'] == storage &&
    save['if'] == "success() && #{writer}" && steps.last == save, 'only a qualified develop push may save the Omarchy archive')
end

documents = Dir.glob(File.join(root, '.github/workflows/ci*.yml')).to_h { |file| [File.basename(file, '.yml'), YAML.load_file(file)] }
actions = Dir.glob(File.join(root, '.github/actions/**/action.{yml,yaml}')).to_h do |file|
  ["./#{File.dirname(file).delete_prefix(root + '/')}", YAML.load_file(file)]
end
validate(documents, actions)
workflows = Dir.glob(File.join(root, '.github/workflows/*.{yml,yaml}')).to_h do |file|
  [File.basename(file, File.extname(file)), YAML.load_file(file)]
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
before_queue = "success() && (github.event_name == 'push' || github.event.pull_request.head.repo.full_name == github.repository)"
mutations = [
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].reject! { |s| s['name'] == 'API allocator qualification' } },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'] << d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == 'API allocator qualification' }.dup },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == 'API allocator qualification' }['continue-on-error'] = true },
  ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == 'API allocator qualification' }['run'].sub!('--test-threads 1', '--test-threads 2') },
  ->(d) { d['ci'][true].delete('merge_group') },
  ->(d) { d['ci'][true]['merge_group']['types'] = ['destroyed'] },
  ->(d) { d['ci'][true]['merge_group']['branches'] = ['main'] },
  ->(d) { d['ci'][true]['merge_group']['paths'] = ['web/**'] },
  ->(d) { d['ci']['concurrency']['group'] = 'ci-${{ github.workflow }}-${{ github.event.merge_group.base_ref || github.ref }}' },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Export tested native image' }['if'] = before_queue },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Retain tested native image' }['if'] = 'success()' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Export tested Omarchy image' }['if'] = 'success()' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Retain tested Omarchy image' }['if'] = before_queue },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Prepare API Cargo cache identity' }['if'] = "matrix.component == 'api'" },
  ->(d) { d['ci-repo']['jobs']['structure']['steps'].reject! { |s| s['name'] == 'Synthetic authority and observer regressions' } },
  ->(d) { d['ci-repo']['jobs']['structure']['if'] = 'false' },
  ->(d) { d['ci-repo']['jobs']['structure']['continue-on-error'] = true },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s.dig('with', 'cache-to') }['with']['cache-to'].sub!("|| ''", "|| 'type=gha'") },
  ->(d) { d['ci-web']['jobs']['web']['steps'].find { |s| s['name'] == 'Restore npm download cache' }['uses'] = 'actions/cache@v4' },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Restore Trivy databases' }['with'].delete('restore-keys') },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Vulnerability scan' }['with'].delete('cache') },
  ->(d) { d['ci'][true].delete('push') },
  ->(d) { d['ci'][true]['push']['paths'] = ['web/**'] },
  ->(d) { d['ci'][true]['pull_request']['branches'].delete('main') },
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
  ->(d) { d['ci']['jobs']['version-label']['name'] = 'Optional label' },
  ->(d) { d['ci'][true]['pull_request']['types'].delete('labeled') },
  ->(d) { d['ci'][true]['pull_request']['types'].delete('unlabeled') },
  ->(d) { d['ci'][true]['pull_request']['paths-ignore'] = ['README.md'] },
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
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].last['if'] = "success() && (#{OmarchyCIContract::CACHE_MISS})" },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'build' }['with']['cache-to'].sub!(OmarchyCIContract::CACHE_WRITE, OmarchyCIContract::CACHE_MISS) },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'build' }['with']['outputs'] = 'type=docker' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'build' }['with']['build-args'].sub!('SOURCE_DATE_EPOCH=0', 'SOURCE_DATE_EPOCH=1') },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'omarchy-cache-restore' }['if'] = "github.event_name == 'push'" },
]
synthetic_index = documents['ci-repo']['jobs']['structure']['steps'].index { |s| s['name'] == 'Synthetic authority and observer regressions' }
restore_index = documents['ci-repo']['jobs']['structure']['steps'].index { |s| s['name'] == 'Synthetic home restore evidence regressions' }
mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'].delete_at(restore_index) }
[{'if' => 'false'}, {'continue-on-error' => true}, {'env' => {'CLOUDSDK_CORE_PROJECT' => 'fixture'}},
 {'run' => 'node .github/scripts/home-backup/inventory.mjs fixture'}].each do |bypass|
  mutations << ->(d) { d['ci-repo']['jobs']['structure']['steps'][restore_index].merge!(bypass) }
end
%w[staging-authority deployment-observer].each do |suite|
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
mutations.each do |mutate|
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
archive = './.github/actions/rust-archive'
%w[dtolnay/rust-toolchain@stable taiki-e/install-action@v2 actions/download-artifact@v4].each do |mutable|
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

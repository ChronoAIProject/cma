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

IMAGE_SCAN_IF = "success() && (github.event_name == 'merge_group' || (github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository))"

# Staging CD refuses an image whose secret scan leaves a blocking finding. CI
# runs the same gate on its exported image before retaining it, so a PR or
# merge group fails first instead of the release after the merge.
def require_image_secret_scan(documents)
  [['ci-images', 'images', '"$IMAGE_COMPONENT"', 'Scan tested image as staging publication does', 'Retain tested native image'],
   ['ci-omarchy', 'omarchy', 'runtime-omarchy', 'Scan tested Omarchy image as staging publication does', 'Retain tested Omarchy image']].each do |file, job, component, name, retain|
    steps = documents.fetch(file).fetch('jobs').fetch(job).fetch('steps')
    install = steps.index { |step| step['name'] == 'Install pinned image secret scanner' }
    scan = steps.index { |step| step['name'] == name }
    export = steps.index { |step| step.fetch('run', '').include?('export-ci-native-image.py') }
    keep = steps.index { |step| step['name'] == retain }
    require_check(install && scan && export && keep && export < install && install < scan && scan < keep,
                  "#{file} must scan the exported image before retaining it")
    [steps[install], steps[scan]].each do |step|
      require_check(step['if'] == IMAGE_SCAN_IF && !step.key?('continue-on-error'), "#{file} image secret scan is optional")
    end
    require_check(steps[install]['run'] == "bash .github/scripts/staging_cd_transport.sh\nbash .github/scripts/install-image-gitleaks.sh\n" &&
                  steps[install]['timeout-minutes'] == 5, "#{file} image scanner install differs")
    command = "python3 -B .github/scripts/ci_image_secret_scan.py #{component} \"$RUNNER_TEMP/ci-native-image\" \"$GITHUB_SHA\""
    require_check(steps[scan]['run'] == command && steps[scan]['timeout-minutes'] == 30, "#{file} image secret scan command differs")
  end
end

RUNTIME_VULNERABILITY_BASE = "${{ github.event_name == 'pull_request' && github.event.pull_request.base.sha || github.event_name == 'merge_group' && github.event.merge_group.base_sha || github.event.before }}"

# Pinned runtime inputs keep building the same packages while the advisory
# database changes. Their trivy scans report everything, and the gate fails
# exactly the findings a change owns or that stopped being fresh. Every other
# image keeps the blanket exit-code gate.
def require_runtime_vulnerability_gate(documents)
  images = documents.fetch('ci-images').fetch('jobs').fetch('images').fetch('steps')
  blanket = images.find { |s| s['name'] == 'Vulnerability scan' }
  require_check(blanket && blanket['if'] == "matrix.component != 'runtime'" && blanket.dig('with', 'exit-code') == '1',
                'non-runtime images must keep the blocking vulnerability scan')
  [['ci-images', 'images', 'Runtime vulnerability scan', 'Runtime vulnerability gate', 'runtime',
    '"$RUNNER_TEMP/runtime-vulnerabilities.json"', "matrix.component == 'runtime'"],
   ['ci-omarchy', 'omarchy', 'Scan tested Omarchy image', 'Omarchy vulnerability gate', 'runtime-omarchy',
    '"$RUNNER_TEMP/omarchy-evidence/vulnerabilities.json"', nil]].each do |file, job, scan_name, gate_name, component, report, condition|
    steps = documents.fetch(file).fetch('jobs').fetch(job).fetch('steps')
    scan = steps.index { |s| s['name'] == scan_name }
    gate = steps.index { |s| s['name'] == gate_name }
    require_check(scan && gate && gate == scan + 1, "#{file} must gate its runtime vulnerability report right after the scan")
    require_check(steps[scan].dig('with', 'exit-code') == '0' && steps[scan].dig('with', 'format') == 'json' &&
                  steps[scan].dig('with', 'severity') == 'HIGH,CRITICAL', "#{file} runtime scan must report JSON for the gate")
    command = "python3 -B .github/scripts/image_vulnerability_gate.py --component #{component} --report #{report} --base \"$BASE_SHA\""
    require_check(steps[gate]['run'] == command && !steps[gate].key?('continue-on-error') && steps[gate]['if'] == condition &&
                  steps[gate].dig('env', 'BASE_SHA') == RUNTIME_VULNERABILITY_BASE, "#{file} runtime vulnerability gate differs")
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
  require_check(exports == ["${{ #{develop} && format('type=gha,version=2,mode=max,ignore-error=true,scope={0}', #{scope}) || '' }}"],
    'image layers may be exported only on develop pushes, and a failed export must never fail the build')
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
  require_check(ci['permissions'] == {'contents' => 'read', 'issues' => 'read'} &&
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

def validate(documents, actions)
  require_runtime_inventory(documents)
  require_image_secret_scan(documents)
  require_runtime_vulnerability_gate(documents)
  check_component_cache_writers(documents)
  check_merge_group_artifacts(documents)
  ci = documents.fetch('ci')
  events = ci.fetch('on', ci[true])
  require_check(events.keys.sort == %w[merge_group pull_request push], 'CI must cover PRs, develop pushes and merge groups')
  require_check(events['push'] == {'branches' => ['develop']}, 'CI push must cover develop without path filters')
  # Without a branch or path filter every queue entry creates the required
  # checks; a non-develop queue is refused by the selector, not left pending.
  require_check(events['merge_group'] == {'types' => ['checks_requested']}, 'CI must report on every merge group')
  # Read-only: issues is read for the runtime vulnerability gate's tracking-issue age.
  require_check(ci['permissions'] == {'contents' => 'read', 'issues' => 'read'}, 'CI permissions must remain read-only')
  %w[ci-images ci-omarchy].each do |called|
    require_check(documents.fetch(called)['permissions'] == {'contents' => 'read', 'issues' => 'read'},
                  "#{called} permissions must remain read-only and within the caller's")
  end
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
    !key.key?('if') && !key.key?('continue-on-error') && !builds.empty? &&
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
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Vulnerability scan' }['if'] = 'true' },
  ->(d) { d['ci-images']['jobs']['images']['steps'].reject! { |s| s['name'] == 'Runtime vulnerability gate' } },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Runtime vulnerability gate' }['continue-on-error'] = true },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Runtime vulnerability gate' }['run'] += ' || true' },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].reject! { |s| s['name'] == 'Omarchy vulnerability gate' } },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Omarchy vulnerability gate' }['env']['BASE_SHA'] = '' },
  ->(d) { d['ci-images']['jobs']['images']['steps'].reject! { |s| s['name'] == 'Scan tested image as staging publication does' } },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Scan tested image as staging publication does' }['continue-on-error'] = true },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Scan tested image as staging publication does' }['if'] = "github.event_name == 'merge_group'" },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Scan tested image as staging publication does' }['run'] += ' || true' },
  ->(d) { steps = d['ci-images']['jobs']['images']['steps']; scan = steps.index { |s| s['name'] == 'Scan tested image as staging publication does' }; steps.insert(steps.index { |s| s['name'] == 'Retain tested native image' } + 1, steps.delete_at(scan)) },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].reject! { |s| s['name'] == 'Install pinned image secret scanner' } },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Scan tested Omarchy image as staging publication does' }['timeout-minutes'] = 120 },
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
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s.dig('with', 'cache-to') }['with']['cache-to'].sub!('ignore-error=true,', '') },
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
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'build' }['with']['build-args'].sub!('${{ steps.os-refresh.outputs.value }}', '${{ github.run_id }}-${{ github.run_attempt }}') },
  ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].reject! { |s| s['id'] == 'os-refresh' } },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Load native image for scanning' }['with']['build-args'].sub!('${{ steps.os-refresh.outputs.value }}', '${{ github.run_id }}-${{ github.run_attempt }}') },
  ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['id'] == 'os-refresh' }['continue-on-error'] = true },
]
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

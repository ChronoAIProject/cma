#!/usr/bin/env ruby
# These are authority/concurrency regressions, not snapshots of YAML layout.
require 'minitest/autorun'
require_relative 'workflow-yaml'
require 'json'
require 'open3'

class StagingWorkflowTest < Minitest::Test
  ROOT = File.expand_path('..', __dir__)

  def workflow(name)
    WorkflowYAML.load_file(File.join(ROOT, 'workflows', name + '.yml'))
  end

  def test_allocator_reads_controller_outcomes_and_only_the_controller_can_emit_the_proof
    jobs = workflow('staging-cd')['jobs']
    refute jobs.fetch('plan').fetch('permissions').key?('deployments')
    assert_equal 'read', jobs.fetch('allocate').dig('permissions', 'deployments')
    allocate = jobs['allocate']['steps'].find { |step| step['id'] == 'allocate' }
    assert_equal 'chrono-ai-staging', allocate.dig('env', 'CD_STAGING_ENVIRONMENT')
    steps = workflow('staging-deploy')['jobs']['deploy']['steps']
    writer = steps.index { |step| step['id'] == 'coordinate' }
    proof = steps.index { |step| step['name'] == 'Verified staging receipts ${{ steps.coordinate.outputs.controller_proof }}' }
    assert_operator writer, :<, proof
    assert_equal "always() && !cancelled() && steps.coordinate.outputs.controller_proof != ''", steps[proof]['if']
  end

  def test_a_new_push_can_allocate_while_an_earlier_release_is_building
    %w[staging-cd staging-release staging-promote staging-deploy redeploy-staging].each do |name|
      refute workflow(name).key?('concurrency'), "#{name} must not have workflow concurrency"
    end
    jobs = workflow('staging-cd')['jobs']
    %w[plan allocate].each { |id| refute jobs.fetch(id).key?('concurrency'), "#{id} must accept every merge" }
    refute jobs.fetch('plan').key?('needs')
    assert_equal 'plan', jobs.fetch('allocate').fetch('needs')
    # Releases also wait for the scanner-review preflight. It needs only
    # allocation and holds no lock, so a new push still never waits on a build.
    assert_equal 'allocate', jobs.fetch('infrastructure').fetch('needs')
    refute jobs.fetch('infrastructure').key?('concurrency')
    %w[web am eg runtime runtime_omarchy].each do |component|
      assert_equal %w[allocate infrastructure], jobs.fetch("release_#{component}").fetch('needs')
    end
    review = jobs.fetch('infrastructure').fetch('steps').index { |step| step['run'].to_s.include?('staging_cd_scan_review.py') }
    resolve = jobs.fetch('infrastructure').fetch('steps').index { |step| step['id'] == 'private' }
    refute_nil review
    assert_operator resolve, :<, review
    assert_equal '${{ steps.private.outputs.revision }}', jobs['infrastructure']['steps'][review].dig('env', 'CD_INFRA_REVISION')
    workflow('staging-release').fetch('jobs').each_value { |job| refute job.key?('concurrency') }
    refute jobs['allocate']['steps'].any? { |step| step['run'].to_s.match?(/buildx|skopeo|deploy\.py/) }
  end

  def test_each_lane_runs_one_release_at_a_time_and_queued_ones_skip_restores
    jobs = workflow('staging-cd')['jobs']
    groups = jobs.values.map { |job| job.dig('concurrency', 'group') }.compact
    %w[web am eg runtime runtime-omarchy].each do |component|
      lock = "cma-staging-#{component}-release"
      assert_equal({ 'group' => lock, 'queue' => 'max', 'cancel-in-progress' => false },
                   jobs.fetch("release_#{component.tr('-', '_')}").fetch('concurrency'))
      assert_equal 1, groups.count(lock)
    end

    steps = workflow('staging-release')['jobs']['publish']['steps']
    index = ->(name) { steps.index { |step| step['name'] == name } }
    ownership = steps[index.call('Check release ownership')]
    assert_equal 'ownership', ownership['id']
    refute ownership.key?('if'), 'every component checks ownership before its restores'
    assert_equal true, ownership['continue-on-error']
    assert_equal 'python3 -B .github/scripts/staging_cd_release_ownership.py', ownership['run']
    restores = ['Restore compatible Cargo mounts', 'Restore authenticated layer detector evidence']
    restores.each do |name|
      assert_operator index.call('Check release ownership'), :<, index.call(name)
      assert_includes steps[index.call(name)]['if'], "steps.ownership.outputs.superseded != 'true'"
    end
    publication = steps[index.call('Build once, scan every layer, and publish exact bytes')]
    refute publication.key?('if'), 'publication must always run and decide the outcome'
  end

  def test_independent_runtime_promotions_wait_for_verified_am_when_selected
    jobs = workflow('staging-cd').fetch('jobs')
    %w[runtime runtime_omarchy].each do |lane|
      release = "release_#{lane}"
      assert_equal %w[allocate infrastructure], jobs.fetch(release).fetch('needs')
      promotion = jobs.fetch("promote_#{lane}")
      assert_equal ['allocate', 'infrastructure', release, 'promote_am'], promotion.fetch('needs')
      condition = promotion.fetch('if')
      assert_includes condition, '!cancelled()'
      assert_includes condition, "needs.#{release}.result == 'success'"
      assert_includes condition, "needs.#{release}.outputs.finalized == 'success'"
      assert_includes condition, "fromJSON(needs.allocate.outputs.lanes)['am'] != 'am' ||"
      assert_includes condition, "needs.promote_am.result == 'success' &&"
      assert_includes condition, "fromJSON(needs.promote_am.outputs.component_result || '{}').components.am.state == 'controller-verified'"

      runtime = lane.tr('_', '-')
      successful_job = {result: 'success', outputs: {finalized: 'success'}}
      verified_result = JSON.generate(components: {am: {state: 'controller-verified'}})
      # Admission can obtain the exact AM resume proof under the lock while
      # this independent Runtime lane has just published a new release.
      resume = {admit: 'success', admit_disposition: 'already-verified', admitted: '["am"]'}
      cases = [
        ['new Runtime and resumed AM', 'am', 'success', resume, true],
        ['fresh verified AM', 'am', 'success', {admit: 'success', component_result: verified_result}, true],
        ['empty AM membership', 'am', 'success', resume.merge(admitted: '[]'), false],
        ['overtaken AM', 'am', 'success', resume.merge(admit_disposition: 'superseded_by_newer_final'), false],
        ['failed admission', 'am', 'success', resume.merge(admit: 'failure'), false],
        ['failed promotion', 'am', 'failure', resume, false],
        ['Runtime-only recovery', nil, 'skipped', {}, true]
      ]
      # GitHub's dereference of a missing property is empty. Use optional
      # chaining for that dereference while running the workflow predicate.
      expression = condition.sub('.components.am.state', '.components?.am?.state')
      script = <<~JS
        const fromJSON = JSON.parse;
        const cancelled = () => false;
        const cases = JSON.parse(process.argv[1]);
        for (const [name, needs, expected] of cases) {
          if (Boolean(#{expression}) !== expected) throw new Error(name);
        }
      JS
      inputs = cases.map do |name, am, result, outputs, expected|
        lanes = {runtime => runtime, 'am' => am}
        needs = {
          allocate: {result: 'success', outputs: {changed: 'true', lanes: JSON.generate(lanes)}},
          infrastructure: successful_job, release => successful_job,
          promote_am: {result: result, outputs: outputs}
        }
        [name, needs, expected]
      end
      output, status = Open3.capture2e('node', '-e', script, JSON.generate(inputs))
      assert status.success?, output
    end
    refute_includes jobs.fetch('promote_contract').fetch('needs'), 'promote_am'
  end

  def test_omarchy_releases_check_pinned_signing_keys_before_any_build_work
    steps = workflow('staging-release')['jobs']['publish']['steps']
    index = ->(name) { steps.index { |step| step['name'] == name } }
    check = steps.fetch(index.call('Check pinned Omarchy signing keys'))
    # The check blocks: an expired key must stop the release with its named cause.
    refute check.key?('continue-on-error'), 'an expired signing key must fail the release'
    assert_equal 'python3 -B .github/scripts/omarchy_key_expiry.py --warn-days 14', check['run']
    assert_equal "inputs.component == 'runtime-omarchy' && steps.ownership.outputs.superseded != 'true'", check['if']
    assert_includes 1..5, check['timeout-minutes'].to_i
    assert_operator index.call('Check release ownership'), :<, index.call('Check pinned Omarchy signing keys')
    assert_operator index.call('Check pinned Omarchy signing keys'), :<,
                    index.call('Build once, scan every layer, and publish exact bytes')
    assert File.file?(File.join(ROOT, 'scripts/omarchy_key_expiry.py'))
  end

  def test_release_evidence_precedes_optional_cache_retention
    steps = workflow('staging-release')['jobs']['publish']['steps']
    index = ->(name) { steps.index { |step| step['name'] == name } }
    publication = index.call('Build once, scan every layer, and publish exact bytes')
    receipt = index.call('Retain only public publication receipts')
    receipt_retry = index.call('Retry retaining only public publication receipts')
    policy = index.call('Retain verified package decisions')
    assert_operator publication, :<, receipt
    assert_operator receipt, :<, receipt_retry
    assert_operator receipt_retry, :<, policy
    refute steps[publication].key?('continue-on-error')
    refute steps[receipt_retry].key?('continue-on-error')
    refute steps[policy].key?('continue-on-error')
    assert_equal "always() && steps.receipts.outcome == 'failure'", steps[receipt_retry]['if']

    ['Seal accepted layer archive', 'Retain sealed layer archive for admission',
     'Retain complete layer detector evidence', 'Retain accepted Cargo mounts',
     'Index the exact graph of the retained Cargo mounts'].each do |name|
      assert_operator policy, :<, index.call(name)
      assert_equal true, steps[index.call(name)]['continue-on-error'], name
    end
    ['Prepare component Cargo cache', 'Restore compatible Cargo mounts',
     'Restore authenticated layer detector evidence'].each do |name|
      assert_equal true, steps[index.call(name)]['continue-on-error'], name
    end
    assert_includes steps[index.call('Restore compatible Cargo mounts')]['if'], "steps.build-cache.outcome == 'success'"
    assert_equal 2, steps[index.call('Retain complete layer detector evidence')]['timeout-minutes']
  end

  # A setup step with no bound once held the staging queue for five hours: the
  # publish job allows 300 minutes for builds, and a stalled apt mirror used it all.
  def test_every_release_step_is_bounded_well_inside_its_job
    workflow('staging-release')['jobs'].each do |name, job|
      limit = job.fetch('timeout-minutes').to_i
      job.fetch('steps').each do |step|
        label = step['name'] || step['uses']
        bound = step['timeout-minutes']
        refute_nil bound, "#{name}: step without a bound: #{label}"
        assert_operator bound.to_i, :<, limit, "#{name}: #{label} may use the whole job"
      end
      release_steps = %w[producer publication]
      setup = job['steps'].reject { |step| release_steps.include?(step['id']) }
      assert setup.all? { |s| s['timeout-minutes'].to_i <= 15 }, "#{name}: setup and upload steps must fail within 15 minutes"
      next unless name == 'publish'

      producer, publication = release_steps.map { |id| job['steps'].find { |step| step['id'] == id } }
      assert_equal 'python3 -B .github/scripts/staging_cd_produce.py', producer.fetch('run')
      assert_includes producer.fetch('if'), "vars.CMA_STAGING_RELEASE_ARCHIVE_HANDOFF == 'true'"
      assert_operator producer.fetch('timeout-minutes'), :<=, 240
      assert_operator job['steps'].index(producer), :<, job['steps'].index(publication)
      assert_equal '${{ steps.producer.outputs.prepared }}', publication.fetch('env').fetch('CD_PREPARED_ARCHIVE')
    end
    transport = File.read(File.join(ROOT, 'scripts/staging_cd_transport.sh'))
    %w[Acquire::http::Timeout DPkg::Lock::Timeout].each { |option| assert_includes transport, option }
    assert_match(/timeout --kill-after=\d+ \d+ sudo apt-get/, transport)
    assert_includes File.read(File.join(ROOT, 'scripts/install-image-gitleaks.sh')), '--max-time'
  end

  def test_all_mutation_paths_share_one_set_lock_without_serializing_publication
    manual = workflow('redeploy-staging')['jobs']['deploy']
    automatic = workflow('staging-promote')['jobs']['deploy']
    assert_equal manual['uses'], automatic['uses']
    refute manual.key?('strategy')
    refute workflow('staging-release')['jobs'].key?('deploy')
    deploy = workflow('staging-deploy')['jobs']['deploy']
    refute deploy.key?('concurrency'), 'a nested acquisition of the caller lock deadlocks the writer'
    assert_equal 'chrono-ai-staging', deploy['environment']
    assert_equal "${{ inputs.mode == 'release' && 50 || 20 }}", deploy['timeout-minutes']
    assert_equal 'release', automatic['with']['mode']
    verification = deploy['steps'].index { |step| step.dig('env', 'CD_RESOLVE_MODE') == 'verify' }
    writer = deploy['steps'].index { |step| step['run'].to_s.include?('staging_cd_invoke.py') }
    assert verification
    assert_operator verification, :<, writer
    assert_equal 'admit', automatic['needs']
    %w[release_set mode started_at infra_ref].each do |key|
      assert manual['with'].key?(key)
      assert automatic['with'].key?(key)
    end
  end

  def test_simultaneous_finalizers_wait_before_starting_the_automatic_admission_clock
    lock = {'group' => 'cma-staging-deploy-set', 'queue' => 'max', 'cancel-in-progress' => false}
    promote = './.github/workflows/staging-promote.yml'
    writer = './.github/workflows/staging-deploy.yml'
    calls = []
    Dir.glob(File.join(ROOT, 'workflows', '*.{yml,yaml}')).each do |file|
      name = File.basename(file, File.extname(file))
      WorkflowYAML.load_file(file)['jobs'].each do |id, job|
        next unless [promote, writer].include?(job['uses'])
        if name == 'staging-promote' && %w[deploy redeploy].include?(id)
          # Jobs inside one reusable call share the caller's slot; a nested
          # acquisition of the same group would deadlock the retry.
          refute job.key?('concurrency'), 'the outer promotion already owns the writer lock'
        else
          assert_equal lock, job['concurrency'], "#{name}/#{id} must own the writer lock"
          calls << [name, id]
        end
      end
    end
    assert_equal 7, calls.size # six automatic scopes and manual redeploy
    automatic = workflow('staging-cd')['jobs']
    # Both components finished at t=0. A 90s writer exceeds the original 45s
    # admission window. With the lock around the reusable call, the second
    # resolver can first run at t=90; the deadline is issued there exactly once.
    finish = 0
    admissions = %w[promote_web promote_eg].map do |id|
      job = automatic.fetch(id)
      starts = job['concurrency'] == lock ? finish : 0
      assert_equal promote, job['uses']
      finish = starts + 90
      starts
    end
    assert_equal [0, 90], admissions
    admission = workflow('staging-promote')['jobs']['admit']
    assert admission['steps'].any? { |step| step.dig('env', 'CD_RESOLVE_MODE') == 'automatic' }
    assert_equal '${{ needs.admit.outputs.started_at }}', workflow('staging-promote').dig('jobs', 'deploy', 'with', 'started_at')
  end

  def test_credentials_and_write_permissions_cannot_reach_build_or_redeploy
    image = workflow('staging-release')['jobs']['publish']
    final = workflow('staging-release')['jobs']['finalize']
    assert_equal 'read', image['permissions']['contents']
    assert_equal 'write', image['permissions']['packages']
    # Publication lists the Actions caches to verify its layer export.
    assert_equal 'read', image['permissions']['actions']
    publication = image['steps'].find { |step| step['id'] == 'publication' }
    assert_equal '${{ github.token }}', publication.dig('env', 'GH_TOKEN')
    assert_equal 'write', final['permissions']['contents']
    refute final['permissions'].key?('packages')
    refute workflow('redeploy-staging').to_s.include?('packages')
    refute workflow('redeploy-staging').to_s.include?('contents"=>"write')
    deploy = workflow('staging-deploy')['jobs']['deploy']
    keyed = deploy['steps'].select { |step| step.fetch('env', {}).key?('CD_NYXID_CLIENT_SECRET') }
    assert_equal 1, keyed.length
    assert_equal '${{ secrets.CD_NYXID_CLIENT_ID }}', keyed.first['env']['CD_NYXID_CLIENT_ID']
    assert_equal '${{ secrets.CD_NYXID_CLIENT_SECRET }}', keyed.first['env']['CD_NYXID_CLIENT_SECRET']
    pointer_credentials = deploy['steps'].select { |step| step.fetch('env', {}).key?('CD_RUNTIME_POINTER_TOKEN') }
    assert_equal keyed, pointer_credentials
    assert_equal '${{ secrets.CD_RUNTIME_POINTER_TOKEN }}', keyed.first['env']['CD_RUNTIME_POINTER_TOKEN']
    refute deploy.to_s.include?('NYXID_CHRONO_CD_KEY')
    assert_equal '"$RUNNER_TEMP/cma-deploy-tools/venv/bin/python" -B .github/scripts/staging_cd_invoke.py', keyed.first['run']
    setup = deploy['steps'].find { |step| step['run'] == 'python3 -B .github/scripts/staging_cd_tools.py install' }
    assert setup
    assert_operator deploy['steps'].index(setup), :<, deploy['steps'].index(keyed.first)
    assert_empty setup.fetch('env', {})
    refute deploy.to_s.include?('install-image-gitleaks')
    cache = deploy['steps'].find { |step| step['uses'].to_s.start_with?('actions/cache/restore@') }
    assert cache
    refute cache['with'].key?('restore-keys')
    assert_equal '${{ runner.temp }}/cma-public-tool-cache', cache['with']['path']
    requirements = deploy['steps'].find { |step| step['run'] == 'python3 -B .github/scripts/staging_cd_tools.py requirements' }
    checkout = deploy['steps'].find { |step| step.fetch('with', {})['repository'] }
    assert_operator deploy['steps'].index(checkout), :<, deploy['steps'].index(requirements)
    assert_operator deploy['steps'].index(requirements), :<, deploy['steps'].index(keyed.first)
    assert_empty requirements.fetch('env', {})
    %w[staging-cd staging-release staging-promote redeploy-staging].each do |name|
      workflow(name)['jobs'].each_value do |job|
        if %w[./.github/workflows/staging-promote.yml ./.github/workflows/staging-deploy.yml].include?(job['uses'])
          job = job.reject { |field, _| field == 'secrets' }
        end
        %w[NYXID_CHRONO_CD_KEY CD_NYXID_CLIENT_ID CD_NYXID_CLIENT_SECRET].each { |field| refute_includes job.to_s, field }
        refute job.key?('secrets')
      end
    end
  end

  def test_secret_forwarding_is_explicit_at_each_promotion_hop
    keys = %w[CMA_INFRA_APP_PRIVATE_KEY CD_NYXID_CLIENT_ID CD_NYXID_CLIENT_SECRET]
    expected = keys.to_h { |key| [key, "${{ secrets.#{key} }}"] }
    %w[staging-promote staging-deploy].each do |name|
      config = workflow(name)
      assert_equal keys.to_h { |key| [key, {'required' => true}] }, (config['on'] || config[true])['workflow_call']['secrets']
      reads = config.fetch('jobs').to_s.scan(/secrets\.([A-Za-z0-9_]+)/).flatten.uniq.sort
      permitted = name == 'staging-deploy' ? keys + ['CD_RUNTIME_POINTER_TOKEN'] : keys
      assert_equal permitted.sort, reads
    end
    %w[staging-cd staging-promote redeploy-staging].each do |name|
      workflow(name)['jobs'].each_value do |job|
        next unless %w[./.github/workflows/staging-promote.yml ./.github/workflows/staging-deploy.yml].include?(job['uses'])
        assert_equal expected, job['secrets']
      end
    end
    workflow('staging-release')['jobs'].each_value { |job| keys.each { |key| refute_includes job.to_s, key } }
  end

  def test_web_can_deploy_while_am_is_still_publishing
    jobs = workflow('staging-cd')['jobs']
    # Advance only the jobs that have completed in this scenario. Evaluate
    # dependencies from the loaded workflows, including reusable job calls.
    completed = %w[plan allocate infrastructure release_web]
    runnable = ->(job) { (Array(job['needs']) - completed).empty? }
    assert runnable.call(jobs['promote_web'])
    refute runnable.call(jobs['promote_am'])
    refute runnable.call(jobs['recheck'])
    %w[web am eg runtime runtime-omarchy].each do |component|
      id = component.tr('-', '_')
      publisher = jobs.fetch("release_#{id}")
      promote = jobs.fetch("promote_#{id}")
      assert_equal %w[allocate infrastructure], publisher['needs']
      assert_equal './.github/workflows/staging-release.yml', publisher['uses']
      assert_equal component, publisher.dig('with', 'component')
      assert_equal "Release #{component}", publisher['name'] # detector-cache producer identity
      promotion_needs = %W[allocate infrastructure release_#{id}]
      promotion_needs << 'promote_am' if component.start_with?('runtime')
      assert_equal promotion_needs, promote['needs']
      assert_includes promote['if'], "needs.release_#{id}.outputs.finalized == 'success'"
      assert_includes promote['if'], "outputs.lanes)['#{component}'] == '#{component}'"
      assert_equal './.github/workflows/staging-promote.yml', promote['uses']
      assert_equal component, promote.dig('with', 'lane')
      assert_includes promote.dig('with', 'publication_matrix'), "outputs.batches)['#{component}']"
      assert_includes jobs['recheck']['needs'], "promote_#{id}"
    end
    release = workflow('staging-release')['jobs']
    assert_equal 'publish', release['finalize']['needs']
    assert_equal '${{ steps.finalize.outcome }}', release['finalize'].dig('outputs', 'finalized')
    pipeline = workflow('staging-promote')['jobs']
    assert_nil pipeline['admit']['needs']
    assert_equal 'admit', pipeline['deploy']['needs']
    assert_includes pipeline['deploy']['if'], "needs.admit.result == 'success'"
    assert_includes pipeline['deploy']['if'], "needs.admit.outputs.changed == 'true'"
    assert_equal './.github/workflows/staging-deploy.yml', pipeline['deploy']['uses']
    assert_equal 'Publish', release['publish']['name']
    assert_equal 'Finalize', release['finalize']['name']
  end

  def test_joint_transition_waits_only_for_selected_contract_publications
    jobs = workflow('staging-cd')['jobs']
    joint = jobs['promote_contract']
    assert_equal %w[allocate infrastructure release_am release_runtime release_runtime_omarchy], joint['needs']
    assert_includes joint['if'], '!cancelled()'
    assert_includes joint['if'], "needs.infrastructure.result == 'success'"
    %w[am runtime runtime-omarchy].each do |c|
      id = c.tr('-', '_')
      assert_includes joint['if'], "outputs.lanes)['#{c}'] != 'contract' ||"
      assert_includes joint['if'], "needs.release_#{id}.result == 'success'"
      assert_includes joint['if'], "needs.release_#{id}.outputs.finalized == 'success'"
    end
    assert_includes jobs['recheck']['needs'], 'promote_contract'
    assert_includes jobs['recheck']['if'], 'always()'
    assert_equal 'Report component outcomes', jobs['recheck']['name']
    report = jobs['recheck']['steps'].last
    assert_equal 'always()', report['if']
    assert_includes report['run'], 'staging_cd_component_report.py'
    evidence = workflow('staging-promote')['jobs']['admit']['steps'].last
    assert_equal 'always()', evidence['if']
    assert_equal 'staging-admission-${{ github.run_id }}-${{ github.run_attempt }}-${{ inputs.lane }}', evidence['with']['name']
    assert_equal '${{ runner.temp }}/public-admission/admission.json', evidence['with']['path']
  end

  def test_an_overtaken_set_skips_every_deployment_step_and_reaches_the_component_report
    deploy = workflow('staging-deploy')
    job = deploy['jobs']['deploy']
    # Verification or, after it, the coordinator under the set lock may find the set overtaken.
    assert_equal '${{ steps.release.outputs.disposition || steps.coordinate.outputs.disposition }}',
                 job.dig('outputs', 'disposition')
    trigger = deploy['on'] || deploy.fetch(true)
    assert_equal '${{ jobs.deploy.outputs.disposition }}', trigger.dig('workflow_call', 'outputs', 'disposition', 'value')
    steps = job['steps']
    verify = steps.index { |step| step['id'] == 'release' }
    assert_equal 'verify', steps[verify].dig('env', 'CD_RESOLVE_MODE')
    gate = "steps.release.outputs.disposition == ''"
    steps.drop(verify + 1).each do |step|
      condition = step['if'].to_s
      # Cleanup always runs; the proof echo needs a coordinator result; every other step is gated.
      next if condition.start_with?('always()')

      assert_includes condition, gate, step['name']
    end
    assert_equal ["Verified staging receipts ${{ steps.coordinate.outputs.controller_proof }}",
                  'Remove private checkout', 'Remove isolated deployment tools'],
                 steps.drop(verify + 1).select { |step| step['if'].to_s.start_with?('always()') }.map { |s| s['name'] }
    promote = workflow('staging-promote')
    trigger = promote['on'] || promote.fetch(true)
    assert_equal "${{ fromJSON(jobs.readmit.result == 'skipped' && toJSON(jobs.deploy.outputs.disposition) || " \
                 'toJSON(jobs.redeploy.outputs.disposition)) }}',
                 trigger.dig('workflow_call', 'outputs', 'deploy_disposition', 'value')
    assert_equal "${{ fromJSON(jobs.readmit.result == 'skipped' && toJSON(jobs.admit.outputs.disposition) || " \
                 'toJSON(jobs.readmit.outputs.disposition)) }}',
                 trigger.dig('workflow_call', 'outputs', 'admit_disposition', 'value')
    admit = promote['jobs']['admit']
    assert_equal '${{ steps.set.outputs.disposition }}', admit.dig('outputs', 'disposition')
    assert_equal 'automatic', admit['steps'].find { |step| step['id'] == 'set' }.dig('env', 'CD_RESOLVE_MODE')
  end

  def test_only_the_locked_staging_promotion_admission_reads_prior_proofs
    callers = workflow('staging-cd')['jobs'].values.select { |job| job['uses'] == './.github/workflows/staging-promote.yml' }
    assert_equal 6, callers.length
    callers.each do |job|
      assert_equal 'cma-staging-deploy-set', job.dig('concurrency', 'group')
      assert_equal 'read', job.dig('permissions', 'deployments')
    end
    promote = workflow('staging-promote')
    %w[admit readmit].each do |job|
      admission = promote['jobs'].fetch(job)
      assert_equal 'read', admission.dig('permissions', 'deployments')
      assert_equal 'chrono-ai-staging', admission['steps'].find { |step| step['id'] == 'set' }.dig('env', 'CD_STAGING_ENVIRONMENT')
    end
    %w[deploy redeploy].each { |job| refute promote['jobs'][job]['permissions'].key?('deployments') }
  end

  def test_one_automatic_retry_re_admits_after_a_transient_refusal_and_its_outcome_counts
    promote = workflow('staging-promote')
    jobs = promote['jobs']
    assert_equal %w[admit deploy readmit redeploy], jobs.keys
    deploy = workflow('staging-deploy')
    inputs = (deploy['on'] || deploy.fetch(true))['workflow_call']['inputs']
    assert_equal({'type' => 'boolean', 'default' => false}, inputs['retry_allowed'])
    assert_equal true, jobs['deploy'].dig('with', 'retry_allowed')
    # The manual redeploy and the retry itself never hand over again.
    refute workflow('redeploy-staging')['jobs']['deploy']['with'].key?('retry_allowed')
    assert_equal false, jobs['redeploy'].dig('with', 'retry_allowed')
    writer = deploy['jobs']['deploy']
    assert_equal '${{ steps.coordinate.outputs.retryable }}', writer.dig('outputs', 'retryable')
    assert_equal '${{ jobs.deploy.outputs.retryable }}',
                 (deploy['on'] || deploy.fetch(true)).dig('workflow_call', 'outputs', 'retryable', 'value')
    coordinate = writer['steps'].find { |step| step['id'] == 'coordinate' }
    assert_equal '${{ inputs.retry_allowed }}', coordinate.dig('env', 'CD_RETRY_ALLOWED')

    readmit = jobs['readmit']
    assert_equal 'deploy', readmit['needs']
    assert_equal "always() && !cancelled() && needs.deploy.outputs.retryable == 'true'", readmit['if']
    # Re-admission is the reviewed admission, unchanged, with its own evidence name.
    first, again = jobs['admit']['steps'], readmit['steps']
    assert_equal first[0..-2], again[0..-2]
    assert_equal first.last.merge('with' => first.last['with'].merge(
      'name' => 'staging-admission-${{ github.run_id }}-${{ github.run_attempt }}-${{ inputs.lane }}-retry')), again.last
    assert_equal jobs['admit']['outputs'], readmit['outputs']

    redeploy = jobs['redeploy']
    assert_equal 'readmit', redeploy['needs']
    assert_includes redeploy['if'], "needs.readmit.result == 'success'"
    assert_includes redeploy['if'], "needs.readmit.outputs.changed == 'true'"
    assert_equal jobs['deploy']['uses'], redeploy['uses']
    assert_equal jobs['deploy']['secrets'], redeploy['secrets']
    assert_equal '${{ needs.readmit.outputs.release_set }}', redeploy.dig('with', 'release_set')
    assert_equal '${{ needs.readmit.outputs.started_at }}', redeploy.dig('with', 'started_at')
    assert_equal "${{ format('-{0}-retry', inputs.lane) }}", redeploy.dig('with', 'evidence_suffix')
    refute redeploy.key?('concurrency')

    outputs = (promote['on'] || promote.fetch(true))['workflow_call']['outputs']
    {'component_result' => %w[deploy redeploy component_result], 'deploy' => %w[deploy redeploy status],
     'deploy_disposition' => %w[deploy redeploy disposition], 'admit' => %w[admit readmit status],
     'admit_disposition' => %w[admit readmit disposition], 'admitted' => %w[admit readmit admitted],
     'failures' => %w[admit readmit failures]}.each do |name, (initial, second, field)|
      assert_equal "${{ fromJSON(jobs.readmit.result == 'skipped' && toJSON(jobs.#{initial}.outputs.#{field}) || " \
                   "toJSON(jobs.#{second}.outputs.#{field})) }}", outputs[name]['value'], name
    end
    assert_equal "${{ jobs.readmit.result != 'skipped' }}", outputs['retried']['value']
  end

  def test_a_production_set_a_newer_release_owns_deploys_and_reports_nothing
    deploy = workflow('prod-deploy')
    job = deploy['jobs']['deploy']
    assert_equal 'production', job.dig('env', 'CD_TARGET')
    assert_equal 'release', job.dig('env', 'CD_DEPLOY_MODE')
    # Outlives the 3600 s production release total and its 15 s grace, which start after selection.
    assert_equal 75, job['timeout-minutes']
    # Reproducing the approved set never hands over; only the coordinator can.
    assert_equal '${{ steps.coordinate.outputs.disposition }}', job.dig('outputs', 'disposition')
    trigger = deploy['on'] || deploy.fetch(true)
    assert_equal '${{ jobs.deploy.outputs.disposition }}', trigger.dig('workflow_call', 'outputs', 'disposition', 'value')
    steps = job['steps']
    # release_commit reads this merge's parents; contract checks read release sources locally.
    assert_equal 0, steps.first.dig('with', 'fetch-depth')
    assert_equal 'python3 -B .github/scripts/prod_cd_select.py', steps.find { |step| step['id'] == 'release' }['run']
    coordinate = steps.index { |step| step['id'] == 'coordinate' }
    # A handover writes no controller proof or component result, so nothing after it reports a release.
    assert_equal ["always() && !cancelled() && steps.coordinate.outputs.controller_proof != ''", 'always()', 'always()'],
                 steps.drop(coordinate + 1).map { |step| step['if'] }
    assert_equal "always() && needs.deploy.outputs.component_result != ''", deploy.dig('jobs', 'report', 'if')
  end

  def test_proven_takeover_in_any_lane_skips_finalize_and_reaches_the_component_report
    release = workflow('staging-release')
    assert_equal '${{ steps.publication.outputs.disposition }}', release.dig('jobs', 'publish', 'outputs', 'disposition')
    trigger = release['on'] || release.fetch(true)
    assert_equal '${{ jobs.publish.outputs.disposition }}', trigger.dig('workflow_call', 'outputs', 'disposition', 'value')
    # Any skip disposition, including an older run's same-source final, skips finalize.
    assert_includes release.dig('jobs', 'finalize', 'if'), "needs.publish.outputs.disposition == ''"
    report = workflow('staging-cd').dig('jobs', 'recheck', 'steps').last.fetch('env')
    refute report.key?('CD_AM_DISPOSITION')
    %w[web am eg runtime runtime_omarchy].each do |key|
      assert_equal "${{ needs.release_#{key}.outputs.disposition }}", report["CD_DISPOSITION_#{key.upcase}"]
      assert_equal "${{ needs.release_#{key}.outputs.finalized }}", report["CD_FINALIZED_#{key.upcase}"]
    end
  end

  def test_each_package_write_grant_delegates_only_to_the_publisher
    require_relative 'ci-contract'
    inputs = CIContract.load_inputs
    CIContract.staging_package_authority(inputs)
    %w[web am eg runtime runtime_omarchy].each do |id|
      job = inputs['staging-cd']['jobs']["release_#{id}"]
      original = job['uses']
      job['uses'] = './.github/workflows/staging-promote.yml'
      assert_raises(CIContract::Refusal) { CIContract.staging_package_authority(inputs) }
      job['uses'] = original
    end
  end

  def test_package_bundle_writes_only_on_a_develop_dispatch_with_pinned_actions
    require_relative 'ci-contract'
    inputs = CIContract.load_inputs
    CIContract.staging_package_authority(inputs)
    [->(w) { w[true]['push'] = { 'branches' => ['develop'] } },
     ->(w) { w['permissions'] = { 'packages' => 'write' } },
     ->(w) { w['jobs']['bundle']['permissions']['contents'] = 'write' },
     ->(w) { w['jobs']['bundle']['if'] = "github.repository == 'ChronoAIProject/cma'" },
     ->(w) { w['jobs']['bundle']['runs-on'] = 'ubuntu-latest-16-cores' },
     ->(w) { w['jobs']['bundle']['env']['REPOSITORY'] = 'chronoaiproject/cma-runtime' },
     ->(w) { w['jobs']['bundle']['steps'].find { |s| s['uses'].to_s.start_with?('docker/login-action') }['uses'] = 'docker/login-action@v3' },
     ->(w) { w['jobs']['bundle']['steps'].reject! { |s| s['name'] == 'Refuse an existing tag before building' } },
     ->(w) { w['jobs']['bundle']['steps'].find { |s| s['name'] == 'Publish the exact verified layout' }['run'].sub!('--preserve-digests ', '') },
     ->(w) { w['jobs']['extra'] = { 'permissions' => { 'contents' => 'read' }, 'runs-on' => 'ubuntu-24.04', 'steps' => [] } }].each do |mutate|
      copy = Marshal.load(Marshal.dump(inputs))
      mutate.call(copy['omarchy-package-bundle'])
      assert_raises(CIContract::Refusal) { CIContract.staging_package_authority(copy) }
    end
  end

  def test_package_bundle_proves_its_tag_absent_immediately_before_its_only_push
    require_relative 'ci-contract'
    inputs = CIContract.load_inputs
    CIContract.staging_package_authority(inputs)
    step = ->(w, name) { w['jobs']['bundle']['steps'].find { |s| s['name'] == name } }
    refuse = ->(w) { step.call(w, 'Refuse an existing tag before building') }
    publish = ->(w) { step.call(w, 'Publish the exact verified layout') }
    {
      'step-level REPOSITORY on the push' => ->(w) { publish.call(w)['env']['REPOSITORY'] = 'chronoaiproject/cma-runtime' },
      'REPOSITORY reassigned in the push script' =>
        ->(w) { publish.call(w)['run'].sub!("set -euo pipefail\n", "set -euo pipefail\nREPOSITORY=chronoaiproject/cma-runtime\n") },
      'second absence check after the push' => lambda do |w|
        lines = publish.call(w)['run'].lines
        check = lines.delete_at(lines.index { |line| line.include?('omarchy-package-bundle.py absent') })
        lines.insert(lines.index { |line| line.include?('"docker://ghcr.io/$REPOSITORY:$TAG"') } + 1, check)
        publish.call(w)['run'] = lines.join
      end,
      'second absence check allowed to fail' => ->(w) { publish.call(w)['run'].sub!(%(--tag "$TAG"\n), %(--tag "$TAG" || true\n)) },
      'first absence check after the build' => lambda do |w|
        steps = w['jobs']['bundle']['steps']
        moved = steps.delete(refuse.call(w))
        steps.insert(steps.index { |s| s['name'] == 'Download, verify and lay out the bundle' } + 1, moved)
      end,
      'hard-coded push destination' =>
        ->(w) { publish.call(w)['run'].sub!('"docker://ghcr.io/$REPOSITORY:$TAG"', '"docker://ghcr.io/chronoaiproject/cma-runtime:latest"') },
      'optional first absence check' => ->(w) { refuse.call(w)['continue-on-error'] = true },
      'skipped first absence check' => ->(w) { refuse.call(w)['if'] = 'false' },
      'optional bundle job' => ->(w) { w['jobs']['bundle']['continue-on-error'] = true },
    }.each do |label, mutate|
      copy = Marshal.load(Marshal.dump(inputs))
      mutate.call(copy['omarchy-package-bundle'])
      refute_equal inputs['omarchy-package-bundle'], copy['omarchy-package-bundle'], label
      assert_raises(CIContract::Refusal, label) { CIContract.staging_package_authority(copy) }
    end
  end

  def test_key_expiry_check_stays_weekly_read_only_and_required
    require_relative 'ci-contract'
    inputs = CIContract.load_inputs
    CIContract.omarchy_key_expiry_authority(inputs)
    expiry = ->(w) { w['jobs']['expiry'] }
    check = ->(w) { expiry.call(w)['steps'].find { |s| s['name'] == 'Check Arch packager key expiry' } }
    {
      'schedule removed' => ->(w) { w[true].delete('schedule') },
      'runs on pull requests' => ->(w) { w[true]['pull_request_target'] = { 'branches' => ['develop'] } },
      'runs daily' => ->(w) { w[true]['schedule'] = [{ 'cron' => '17 3 * * *' }] },
      'second schedule' => ->(w) { w[true]['schedule'] << { 'cron' => '17 4 * * 1' } },
      'workflow write grant' => ->(w) { w['permissions'] = { 'contents' => 'write' } },
      'job package grant' => ->(w) { expiry.call(w)['permissions']['packages'] = 'read' },
      'job disabled' => ->(w) { expiry.call(w)['if'] = 'false' },
      'optional job' => ->(w) { expiry.call(w)['continue-on-error'] = true },
      'self-hosted runner' => ->(w) { expiry.call(w)['runs-on'] = 'self-hosted' },
      'unbounded job' => ->(w) { expiry.call(w).delete('timeout-minutes') },
      'job secret' => ->(w) { expiry.call(w)['env'] = { 'GH_TOKEN' => '${{ secrets.GITHUB_TOKEN }}' } },
      'mutable checkout' => ->(w) { expiry.call(w)['steps'].first['uses'] = 'actions/checkout@v5' },
      'persisted checkout credentials' => ->(w) { expiry.call(w)['steps'].first['with'].delete('persist-credentials') },
      'check removed' => ->(w) { expiry.call(w)['steps'].delete(check.call(w)) },
      'optional check' => ->(w) { check.call(w)['continue-on-error'] = true },
      'skipped check' => ->(w) { check.call(w)['if'] = 'false' },
      'check allowed to fail' => ->(w) { check.call(w)['run'] += ' || true' },
      'check warns later' => ->(w) { check.call(w)['run'] += ' --warn-days 0' },
      'check no longer fails ahead of expiry' => ->(w) { check.call(w)['run'].sub!(' --fail-days 30', '') },
      'check fails later' => ->(w) { check.call(w)['run'].sub!('--fail-days 30', '--fail-days 7') },
      'record drift unchecked' => ->(w) { check.call(w)['run'].sub!(' --verify-record', '') },
      'record rewritten instead of verified' => ->(w) { check.call(w)['run'].sub!('--verify-record', '--refresh') },
      'extra step' => ->(w) { expiry.call(w)['steps'] << { 'run' => 'true' } },
      'extra job' => ->(w) { w['jobs']['extra'] = { 'permissions' => { 'contents' => 'read' }, 'runs-on' => 'ubuntu-24.04', 'steps' => [] } },
    }.each do |label, mutate|
      copy = Marshal.load(Marshal.dump(inputs))
      mutate.call(copy['omarchy-key-expiry'])
      refute_equal inputs['omarchy-key-expiry'], copy['omarchy-key-expiry'], label
      assert_raises(CIContract::Refusal, label) { CIContract.omarchy_key_expiry_authority(copy) }
    end
  end

  def test_publication_signal_waits_for_every_publisher_and_isolates_dispatch_credentials
    jobs = workflow('staging-cd')['jobs']
    signal = jobs.fetch('publication_signal')
    publishers = %w[web am eg runtime runtime_omarchy]
    assert_equal ['allocate', *publishers.map { |name| "release_#{name}" }], signal['needs']
    assert_includes signal['if'], "vars.CMA_PUBLICATION_SIGNAL_ENABLED == 'true'"
    assert_equal({ 'contents' => 'read', 'actions' => 'read' }, signal['permissions'])
    refute signal.key?('environment')
    checkout, verify, retain, notify = signal['steps']
    assert_equal 0, checkout['with']['fetch-depth']
    publishers.each do |name|
      assert_equal "$" + "{{ needs.release_#{name}.outputs.finalized }}", verify['env']["CD_FINALIZED_#{name.upcase}"]
      assert_equal "$" + "{{ needs.release_#{name}.result }}", verify['env']["CD_PUBLISH_#{name.upcase}"]
    end
    assert_equal '${{ runner.temp }}/publication-signal.json', retain['with']['path']
    assert_equal 'error', retain['with']['if-no-files-found']
    refute signal['steps'][0...-1].to_s.include?('secrets.')
    assert_equal({
      'PUBLICATION_CONSUMER' => '${{ secrets.PUBLICATION_CONSUMER }}',
      'PUBLICATION_SIGNAL_TOKEN' => '${{ secrets.PUBLICATION_SIGNAL_TOKEN }}'
    }, notify['env'])
  end

  def test_all_external_actions_are_pinned_and_only_sanitized_deployment_outcomes_upload
    %w[staging-cd staging-release staging-promote staging-deploy redeploy-staging].each do |name|
      workflow(name)['jobs'].each_value do |job|
        job.fetch('steps', []).each do |step|
          next unless step['uses'] && !step['uses'].start_with?('./')
          assert_match(/\A[^@]+@[a-f0-9]{40}\z/, step['uses'])
        end
      end
    end
    deploy = workflow('staging-deploy')
    refute deploy.to_s.match?(/set -x|pull_request_target/)
    uploads = deploy['jobs']['deploy']['steps'].select { |step| step['uses'].to_s.start_with?('actions/upload-artifact@') }
    assert_empty uploads, 'the credentialed deployment job must never upload artifacts'
    report = deploy['jobs']['report']
    refute report.key?('environment')
    refute report.to_s.include?('secrets.')
    refute report.to_s.include?('.github/.cd-private')
    assert_equal({'contents' => 'read'}, report['permissions'])
    assert_equal 'deploy', report['needs']
    assert_equal "always() && needs.deploy.outputs.component_result != ''", report['if']
    assert_equal '${{ steps.coordinate.outputs.component_result }}', deploy['jobs']['deploy']['outputs']['component_result']
    projection = report['steps'].find { |step| step['run'].to_s.include?('staging_cd_deploy_result.py') }
    assert_equal '${{ needs.deploy.outputs.component_result }}', projection['env']['CD_COMPONENT_RESULT']
    assert_equal '${{ inputs.release_set }}', projection['env']['CD_RELEASE_SET']
    upload = report['steps'].find { |step| step['uses'].to_s.start_with?('actions/upload-artifact@') }
    refute upload.key?('if'), 'invalid projections must not upload'
    assert_equal '${{ runner.temp }}/public-deployment/deployment.json', upload['with']['path']
    assert_equal 'staging-deployment-${{ github.run_id }}-${{ github.run_attempt }}${{ inputs.evidence_suffix }}', upload['with']['name']
    assert_operator report['steps'].index(projection), :<, report['steps'].index(upload)
    checkout = deploy['jobs']['deploy']['steps'].find { |step| step.fetch('with', {})['repository'] }
    assert_equal false, checkout['with']['persist-credentials']
    assert_equal '${{ steps.private.outputs.revision }}', checkout['with']['ref']
  end

  def test_dispatch_cannot_supply_admission_clock_or_publish_anything
    manual = workflow('redeploy-staging')
    inputs = manual.fetch('on', manual[true])['workflow_dispatch']['inputs']
    refute inputs.key?('started_at')
    assert_equal 'release', inputs['mode']['default']
    assert_equal %w[release bootstrap], inputs['mode']['options']
    assert inputs.key?('versions')
    refute manual.to_s.match?(/staging_cd_(allocate|publish|finalize)|staging-release/)
    assert_equal '${{ inputs.started_at }}', workflow('staging-deploy')['jobs']['deploy']['env']['CD_STARTED_AT']
    assert_equal '${{ inputs.mode }}', workflow('staging-deploy')['jobs']['deploy']['env']['CD_DEPLOY_MODE']
    assert manual['jobs']['report']['if'].include?('always()')
  end

  def test_final_ruleset_pattern_excludes_deletable_pending_tags
    assert File.fnmatch?('*/v*', 'cma-web/v0.10.1', File::FNM_PATHNAME)
    refute File.fnmatch?('*/v*', 'cma-web/pending/v0.10.1', File::FNM_PATHNAME)
    assert File.fnmatch?('refs/tags/*/v*', 'refs/tags/cma-web/v0.10.1', File::FNM_PATHNAME)
    refute File.fnmatch?('refs/tags/*/v*', 'refs/tags/cma-web/pending/v0.10.1', File::FNM_PATHNAME)
  end

  def test_exact_artifact_download_layout_matches_the_finalizer
    steps = workflow('staging-release')['jobs']['finalize']['steps']
    downloads = steps.select { |step| step['uses'].to_s.start_with?('actions/download-artifact@') }
    finalizer = steps.find { |step| step['run'].to_s.include?('staging_cd_finalize.py') }
    assert_equal %w[receipts-download receipts-download-retry], downloads.map { |step| step['id'] }
    # Both attempts execute the same pinned selection and destination logic.
    downloads.each do |download|
      assert_equal '${{ needs.publish.outputs.artifact }}', download.dig('with', 'artifact-ids')
      refute download['with'].key?('pattern')
      refute download['with'].key?('name')
      config = {action: download['uses'], inputs: download['with'],
                receiptPath: finalizer.dig('env', 'CD_RECEIPTS_PATH')}
      output, status = Open3.capture2e('node', File.join(__dir__, 'staging_cd_download_layout_tests.cjs'),
                                      stdin_data: JSON.generate(config))
      assert status.success?, output
    end
  end

  def test_receipt_download_attempts_are_bounded_inside_the_finalize_budget
    job = workflow('staging-release')['jobs']['finalize']
    steps = job['steps']
    first, repeat = %w[receipts-download receipts-download-retry].map { |id| steps.find { |step| step['id'] == id } }
    assert_equal [2, 2], [first['timeout-minutes'], repeat['timeout-minutes']]
    explain = steps[steps.index(repeat) + 1]
    assert_equal "failure() && steps.receipts-download-retry.outcome == 'failure'", explain['if']
    assert_equal({ 'RECEIPT_ARTIFACT' => '${{ needs.publish.outputs.artifact }}' }, explain['env'])
    assert_includes explain['run'], '[[ "$artifact" =~ ^[0-9]{1,20}$ ]] || artifact=\'?\''
    assert_includes explain['run'], 'Receipt artifact $artifact was not downloaded: both attempts failed.'
    assert_includes explain['run'], 'finalize/artifact_unavailable'
    assert_equal 'exit 1', explain['run'].strip.lines.last
    assert_operator steps.index(explain), :<, steps.index { |step| step['id'] == 'finalize' }
    # A slow repeat that succeeds must still leave the finalizer its whole read budget.
    budget = File.read(File.join(__dir__, 'staging_cd_finalize.py'))[/^FINALIZE_READ_SECONDS = (\d+)$/, 1].to_i
    assert_operator budget, :>, 0
    attempts = [first, repeat].sum { |step| step['timeout-minutes'] * 60 }
    assert_operator attempts + budget, :<, job['timeout-minutes'] * 60
  end

  def test_runtime_canary_interpreter_runs_only_in_changed_runtime_publication
    steps = workflow('staging-release')['jobs']['publish']['steps']
    setup = steps.find { |step| step['name'] == 'Install runtime qualification interpreter' }
    assert_equal "inputs.component == 'runtime' || inputs.component == 'runtime-omarchy'", setup['if']
    assert_equal 26, setup.dig('with', 'node-version')
    assert_operator steps.index(setup), :<, steps.index { |step| step['run'].to_s.include?('staging_cd_publish.py') }
    refute workflow('staging-deploy').to_s.include?('CMA_SMOKE_CLIENT')
    retained = steps.find { |step| step['id'] == 'receipts' }
    assert_equal 'always()', retained['if']
    assert_includes retained.dig('with', 'path'), '/public-receipts/runtime-cleanup.json'
  end

  def test_selector_uses_python312_and_the_shared_pinned_requirements
    steps = workflow('staging-cd')['jobs']['plan']['steps']
    setup = steps.find { |step| step['uses'].to_s.start_with?('actions/setup-python@') }
    assert_equal '3.12', setup.dig('with', 'python-version')
    install = steps.find { |step| step['name'] == 'Install shared selector parsers' }
    assert_includes install['run'], '-r .github/scripts/ci_tracks/requirements.txt'
    assert_operator steps.index(setup), :<, steps.index(install)
    assert_operator steps.index(install), :<, steps.index { |step| step['id'] == 'plan' }
  end

  def test_publication_contracts_use_standard_python_without_hidden_parsers
    steps = workflow('ci-repo')['jobs']['structure']['steps']
    contracts = steps.find { |step| step['name'] == 'Staging publication and deployment contracts' }['run']
    assert_includes contracts, 'python3 -m venv "$RUNNER_TEMP/staging-cd-tests"'
    assert_includes contracts, 'install --disable-pip-version-check PyYAML==6.0.3'
    refute contracts.include?('ci_tracks/requirements.txt')
    refute contracts.include?('--system-site-packages')
    publisher = workflow('staging-release')['jobs']['publish']['steps']
    assert_equal 'python3 -B .github/scripts/staging_cd_publish.py', publisher.find { |step| step['id'] == 'publication' }['run']
    refute publisher.to_s.include?('cma-publication-python')
  end

  def test_image_publication_gate_receipts_are_checked_in_required_ci
    job = workflow('ci-repo')['jobs']['structure']
    refute job.key?('if')
    refute job.key?('continue-on-error')
    step = job['steps'].find { |item| item['name'] == 'Staging publication and deployment contracts' }
    refute step.key?('if')
    refute step.key?('continue-on-error')
    assert_includes step['run'].lines.map(&:strip),
                    '"$RUNNER_TEMP/staging-cd-tests/bin/python" -B .github/scripts/image-secret-gate-tests.py'
  end

  # Safe-to-repeat third-party steps: workflow => job => ids of the optional first attempt.
  REPEATED = {
    'redeploy-staging' => { 'resolve' => %w[infra-token] },
    'staging-cd' => { 'infrastructure' => %w[infra-token] },
    'staging-deploy' => { 'deploy' => %w[infra-token infra-checkout], 'report' => %w[evidence] },
    'staging-release' => { 'publish' => %w[receipts], 'finalize' => %w[receipts-download] }
  }.freeze
  # Optional steps whose failure only costs time; publication decides the outcome later.
  OPTIONAL = ['Expose cache runtime to the Buildx client', 'Check release ownership',
              'Prepare component Cargo cache', 'Restore authenticated layer detector evidence',
              'Retain complete layer detector evidence',
              'Restore compatible Cargo mounts', 'Retain accepted Cargo mounts',
              'Index the exact graph of the retained Cargo mounts',
              'Restore bounded layer archive', 'Validate restored layer archive',
              'Seal accepted layer archive', 'Retain sealed layer archive for admission'].freeze
  OPTIONAL_CACHES = {
    ['staging-cd', 'plan'] => ['Restore previous trusted develop comparisons', 'Save trusted develop comparisons'],
    ['staging-cd', 'infrastructure'] => ['Restore public tool download cache', 'Save verified public tool downloads'],
    ['staging-deploy', 'deploy'] => ['Restore public tool download cache'],
  }.freeze

  def test_safe_actions_repeat_once_and_only_the_repeat_can_fail_the_job
    optional = []
    REPEATED.each do |name, jobs|
      jobs.each do |job_id, ids|
        steps = workflow(name)['jobs'].fetch(job_id)['steps']
        ids.each do |id|
          index = steps.index { |step| step['id'] == id }
          first, repeat = steps[index], steps[index + 1]
          optional << first
          assert_equal true, first['continue-on-error'], "#{name}/#{id}"
          assert_equal first['uses'], repeat['uses']
          assert_equal first['with'], repeat['with']
          refute repeat.key?('continue-on-error'), "#{name}/#{id}: the repeat must be able to fail the job"
          condition = "steps.#{id}.outcome == 'failure'"
          assert_equal first.key?('if') ? "#{first['if']} && #{condition}" : condition, repeat['if']
          if first['uses'].start_with?('actions/upload-artifact@')
            assert_equal true, first['with']['overwrite'], 'a repeat replaces a partial first upload'
          end
        end
      end
    end
    assert_equal 7, optional.size
    # A job is never repeated or made optional, and no other step may hide its failure.
    REPEATED.each_key do |name|
      workflow(name)['jobs'].each do |job_id, job|
        refute job.key?('continue-on-error'), "#{name}/#{job_id}"
        refute job.key?('strategy'), "#{name}/#{job_id}"
        job.fetch('steps', []).select { |step| step['continue-on-error'] }.each do |step|
          cache = OPTIONAL_CACHES.fetch([name, job_id], []).include?(step['name']) &&
                  step['uses'].to_s.match?(%r{\Aactions/cache/(?:restore|save)@[0-9a-f]{40}\z})
          assert(optional.include?(step) || OPTIONAL.include?(step['name']) || cache, "#{name}/#{job_id}: #{step['name']}")
        end
      end
    end
    # Consumers accept the output of whichever attempt succeeded.
    %w[redeploy-staging staging-cd staging-deploy].each do |name|
      text = File.read(File.join(ROOT, 'workflows', name + '.yml'))
      uses = text.scan(/steps\.infra-token\.outputs\.token[^}]*/)
      refute_empty uses
      uses.each { |use| assert_equal 'steps.infra-token.outputs.token || steps.infra-token-retry.outputs.token ', use }
    end
    # An overwriting repeat replaces the first attempt's artifact, so its id wins.
    assert_equal '${{ steps.receipts-retry.outputs.artifact-id || steps.receipts.outputs.artifact-id }}',
                 workflow('staging-release').dig('jobs', 'publish', 'outputs', 'artifact')
  end

  def test_release_artifact_handoffs_use_verified_zip_archives
    upload = 'actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9'
    download = 'actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333'
    %w[staging-cd staging-release staging-promote staging-deploy cache-retention publish prod-deploy].each do |name|
      steps = workflow(name).fetch('jobs').values.flat_map { |job| job.fetch('steps', []) }
      artifacts = steps.select { |step| step['uses'].to_s.match?(%r{\Aactions/(upload|download)-artifact@}) }
      refute_empty artifacts, name
      artifacts.each do |step|
        if step['uses'].start_with?('actions/upload-artifact@')
          assert_equal upload, step['uses'], name
          assert_equal true, step.fetch('with').fetch('archive', true), name
        else
          assert_equal download, step['uses'], name
          assert_equal 'error', step.dig('with', 'digest-mismatch'), name
          assert_equal false, step.fetch('with').fetch('skip-decompress', false), name
        end
      end
    end
  end

  def test_infra_reader_token_authority_is_identical_in_every_release_workflow
    action = 'actions/create-github-app-token@f418cff399f7aef4c2f0ad47e25ca7a6abbdd6fc'
    inputs = {
      'app-id' => '${{ vars.CMA_INFRA_APP_ID }}',
      'private-key' => '${{ secrets.CMA_INFRA_APP_PRIVATE_KEY }}',
      'owner' => 'ChronoAIProject',
      'repositories' => 'cma-infra',
      'permission-contents' => 'read',
    }
    jobs = {
      'staging-cd' => 'infrastructure', 'staging-deploy' => 'deploy', 'redeploy-staging' => 'resolve',
      'prod-cd' => 'resolve', 'prod-deploy' => 'deploy',
    }
    jobs.each do |workflow_name, job|
      steps = workflow(workflow_name).fetch('jobs').fetch(job).fetch('steps')
      tokens = steps.select { |step| step['uses'].to_s.start_with?('actions/create-github-app-token@') }
      assert_equal %w[infra-token infra-token-retry], tokens.map { |step| step['id'] }, workflow_name
      tokens.each do |step|
        assert_equal action, step['uses'], workflow_name
        assert_equal inputs, step['with'], workflow_name
      end
      assert_equal true, tokens.first['continue-on-error'], workflow_name
      refute tokens.last.key?('continue-on-error'), workflow_name
      assert_includes tokens.last['if'], "steps.infra-token.outcome == 'failure'", workflow_name
    end
  end

  def test_catch_up_only_re_runs_the_develop_push_run_and_staging_stays_push_only
    staging = workflow('staging-cd')
    assert_equal 'Staging CD', staging['name'], 'the catch-up trigger names this workflow'
    assert_equal({ 'push' => { 'branches' => ['develop'] } }, staging['on'] || staging[true])
    staging['jobs'].each do |id, job|
      refute_equal 'write', job.dig('permissions', 'actions'), "#{id} must not re-run or dispatch staging"
    end
    check = workflow('staging-cd-catch-up')
    events = check['on'] || check[true]
    assert_equal %w[schedule workflow_run], events.keys.sort
    assert_equal({ 'workflows' => ['Staging CD'], 'types' => ['completed'], 'branches' => ['develop'] },
                 events['workflow_run'])
    assert_equal [{ 'cron' => '3-59/10 * * * *' }], events['schedule']
    assert_equal({}, check['permissions'])
    refute check.key?('concurrency')
    assert_equal %w[catch_up settle recheck], check['jobs'].keys
    job = check['jobs']['catch_up']
    settle = check['jobs']['settle']
    recheck = check['jobs']['recheck']
    # A completion can request one delayed check, using a timer with no repository access.
    assert_equal({ 'settle_until' => '${{ steps.check.outputs.settle_until }}' }, job['outputs'])
    assert_equal 'check', job['steps'].last['id']
    assert_equal 'catch_up', settle['needs']
    assert_equal "github.event_name == 'workflow_run' && needs.catch_up.outputs.settle_until != ''", settle['if']
    assert_equal({}, settle['permissions'])
    assert_equal({ 'group' => 'cma-staging-cd-settle', 'cancel-in-progress' => true }, settle['concurrency'])
    assert_equal 12, settle['timeout-minutes']
    assert_equal 1, settle['steps'].size
    refute settle['steps'].first.key?('uses')
    assert_equal({ 'SETTLE_UNTIL' => '${{ needs.catch_up.outputs.settle_until }}' }, settle['steps'].first['env'])
    assert_equal 'settle', recheck['needs']
    refute recheck.key?('outputs')
    refute recheck.key?('if') # A skipped or cancelled timer never calls a writer.
    %w[github.repository\ ==\ 'ChronoAIProject/cma' github.ref\ ==\ 'refs/heads/develop'
       github.event.workflow_run.event\ ==\ 'push' github.event.workflow_run.head_branch\ ==\ 'develop'
       github.event.workflow_run.head_repository.full_name\ ==\ github.repository].each do |guard|
      assert_includes job['if'], guard
    end
    # The check judges finals by the same controller outcomes the allocator reads.
    allocate = staging.dig('jobs', 'allocate', 'steps').find { |step| step['id'] == 'allocate' }
    [job, recheck].each do |writer|
      assert_equal({ 'contents' => 'read', 'actions' => 'write', 'deployments' => 'read' }, writer['permissions'])
      assert_equal({ 'group' => 'cma-staging-cd-catch-up', 'queue' => 'max',
                     'cancel-in-progress' => false }, writer['concurrency'])
      refute writer.key?('environment')
      refute writer.to_s.include?('secrets.')
      writer['steps'].each do |step|
        refute step.key?('continue-on-error')
        assert_match(/\A[^@]+@[a-f0-9]{40}\z/, step['uses']) if step['uses']
      end
      assert_equal false, writer['steps'].first.dig('with', 'persist-credentials')
      assert_equal ['python3 -B .github/scripts/staging_cd_catch_up.py'], writer['steps'].map { |step| step['run'] }.compact
      assert_equal({ 'GH_TOKEN' => '${{ github.token }}',
                     'CD_STAGING_ENVIRONMENT' => allocate.dig('env', 'CD_STAGING_ENVIRONMENT') }, writer['steps'].last['env'])
    end
  end

  def test_catch_up_reads_the_plan_job_and_its_transient_gate_by_their_exact_names
    source = File.read(File.join(__dir__, 'staging_cd_catch_up.py'))
    job = source.scan(/^PLAN_JOB = "([^"]+)"$/).flatten
    gate = source.scan(/^PLAN_GATE = "([^"]+)"$/).flatten
    assert_equal [1, 1], [job.size, gate.size]
    jobs = workflow('staging-cd')['jobs']
    matching = jobs.select { |id, value| value.fetch('name', id) == job.first }
    assert_equal ['plan'], matching.keys
    gates = matching['plan']['steps'].select { |step| step['name'] == gate.first }
    assert_equal 1, gates.size
    # Renaming the gate, or letting a refusal fail at it, would change what catch-up re-runs.
    assert_includes gates.first['if'], "steps.plan.outputs.transient == 'true'"
  end

  def test_catch_up_reads_the_allocation_job_by_its_exact_display_name
    source = File.read(File.join(__dir__, 'staging_cd_catch_up.py'))
    names = source.scan(/^ALLOCATION_JOB = "([^"]+)"$/).flatten
    assert_equal 1, names.size
    assert_includes source, 'job.get("name") == ALLOCATION_JOB'
    # A job without a name displays its id. Renaming either side would make
    # every head look unallocated, so catch-up would silently never re-run.
    jobs = workflow('staging-cd')['jobs']
    matching = jobs.select { |id, job| job.fetch('name', id) == names.first }
    assert_equal ['allocate'], matching.keys
    assert_includes matching['allocate']['steps'].map { |step| step['run'] },
                    'python3 -B .github/scripts/staging_cd_allocate.py'
  end

  def test_catch_up_reads_the_allocation_gate_by_its_exact_name
    source = File.read(File.join(__dir__, 'staging_cd_catch_up.py'))
    gate = source.scan(/^ALLOCATION_GATE = "([^"]+)"$/).flatten
    assert_equal 1, gate.size
    steps = workflow('staging-cd').dig('jobs', 'allocate', 'steps')
    allocate = steps.index { |step| step['id'] == 'allocate' }
    routes = steps.index { |step| step['id'] == 'routes' }
    gates = steps.each_index.select { |index| steps[index]['name'] == gate.first }
    assert_equal 1, gates.size
    # The gate follows allocation directly and precedes routing, which its failure skips.
    assert_equal [allocate + 1, allocate + 2], [gates.first, routes]
    step = steps[gates.first]
    # Renaming the gate, or letting a refusal that needs a person fail at it,
    # would change what catch-up re-runs.
    assert_equal "steps.allocate.outputs.transient == 'true'", step['if']
    assert_equal({ 'CD_ALLOCATION_REFUSED' => '${{ steps.allocate.outputs.refused }}',
                   'CD_ALLOCATION_SITE' => '${{ steps.allocate.outputs.site }}' }, step['env'])
    assert_equal 'python3 -B .github/scripts/staging_cd_allocate.py --require-settled', step['run']
    [steps[allocate], step, steps[routes]].each { |item| refute item.key?('continue-on-error') }
    refute steps[routes].key?('if'), 'routing must run only after a settled allocation'
  end

  def test_package_and_download_tools_repeat_transient_network_failures
    env = workflow('staging-cd').fetch('env')
    %w[CARGO_NET_RETRY PIP_RETRIES RUSTUP_MAX_RETRIES].each { |key| assert_operator env.fetch(key).to_i, :>=, 3 }
    transport = File.read(File.join(__dir__, 'staging_cd_transport.sh'))
    # One bounded apt invocation, used for both the index update and the install.
    assert_equal 1, transport.scan('sudo apt-get -o Acquire::Retries=3 ').size
    assert_equal %w[update install], transport.scan(/^apt_bounded (update|install)\b/).flatten
    refute_match(/apt-get (?!-o Acquire::Retries)/, transport)
    gitleaks = workflow('ci-repo')['jobs']['secrets']['steps'].find { |step| step['name'] == 'gitleaks' }['run']
    [File.read(File.join(__dir__, 'install-image-gitleaks.sh')), gitleaks].each do |script|
      assert_equal 1, script.scan(/\bcurl\b/).size
      %w[--fail --location --retry --retry-all-errors].each { |flag| assert_includes script, " #{flag} " }
      assert_includes script, '551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb'
      # Only the pinned bytes are ever unpacked.
      assert_operator script.index('sha256sum --check --status'), :<, script.index(/\btar /)
    end
  end

  def test_selection_cache_has_only_a_develop_push_writer_after_successful_selection
    document = workflow('staging-cd')
    trigger = document.fetch('on', document[true])
    assert_equal({'push' => {'branches' => ['develop']}}, trigger)
    plan = document['jobs']['plan']
    assert_includes plan['if'], "github.repository == 'ChronoAIProject/cma'"
    assert_includes plan['if'], "github.ref == 'refs/heads/develop'"
    steps = plan['steps']
    checkout = steps.find { |step| step['uses'].to_s.start_with?('actions/checkout@') }
    assert_equal 2001, checkout.dig('with', 'fetch-depth')
    assert_equal false, checkout.dig('with', 'persist-credentials')
    prepare = steps.index { |step| step['id'] == 'selection-cache' }
    assert_equal '"$RUNNER_TEMP/selector-venv/bin/python" -B .github/scripts/staging_cd_selection_cache.py', steps[prepare]['run']
    restore = steps.index { |step| step['uses'].to_s.start_with?('actions/cache/restore@') }
    discard = steps.index { |step| step['name'] == 'Discard an incomplete comparison cache restore' }
    save = steps.index { |step| step['uses'].to_s.start_with?('actions/cache/save@') }
    select = steps.index { |step| step['id'] == 'plan' }
    gate = steps.index { |step| step['name'] == 'Require a complete release plan' }
    proof = steps.index { |step| step['name'] == 'Require a provable release plan' }
    assert_operator prepare, :<, restore
    assert_operator restore, :<, discard
    assert_operator discard, :<, select
    assert_operator select, :<, save
    assert_equal 'comparison-cache', steps[restore]['id']
    assert_equal "steps.comparison-cache.outcome != 'success' || steps.comparison-cache.outputs.cache-matched-key == ''", steps[discard]['if']
    assert_equal 'rm -rf -- "${RUNNER_TEMP:?}/cma-selection-cache"', steps[discard]['run']
    refute steps[save].key?('if'), 'the default success condition must guard cache writes'
    # A budget-stopped plan saves its analysed revisions before the job fails,
    # and a later step's failure cannot discard them.
    assert_operator save, :<, gate
    assert_operator gate, :<, proof
    # Exactly one gate fails a stopped plan: the first only for a transient
    # stop, which catch-up may re-run; each names its reason.
    assert_equal "steps.plan.outputs.complete != 'true' && steps.plan.outputs.transient == 'true'", steps[gate]['if']
    assert_equal "steps.plan.outputs.complete != 'true' && steps.plan.outputs.transient != 'true'", steps[proof]['if']
    [gate, proof].each do |index|
      assert_equal({ 'CD_PLAN_REFUSED' => '${{ steps.plan.outputs.refused }}' }, steps[index]['env'])
      assert_equal '"$RUNNER_TEMP/selector-venv/bin/python" -B .github/scripts/staging_cd_queue.py --require-complete',
                   steps[index]['run']
    end
    # actions/cache/save already annotates a refused write with its cause, so the
    # plan waits on no cache verification of its own.
    refute plan.to_s.include?('staging_cd_cache_save.py'), 'the plan must not poll the Actions cache'
    [steps[restore], steps[save]].each do |step|
      assert_equal '${{ steps.selection-cache.outputs.directory }}', step['with']['path']
      assert_equal '${{ steps.selection-cache.outputs.key }}', step['with']['key']
      assert_equal true, step['continue-on-error']
      assert_equal 2, step['timeout-minutes']
    end
    assert_equal '${{ steps.selection-cache.outputs.prefix }}', steps[restore]['with']['restore-keys']
    assert_equal 'ubuntu-24.04', plan['runs-on']
    refute plan.to_s.include?('secrets.')
  end

  def test_deploy_tools_start_cold_after_an_unconfirmed_cache_restore
    workflows = [
      ['staging-cd', 'infrastructure', 'Verify and warm public tool downloads before admission'],
      ['staging-deploy', 'deploy', 'Prepare isolated deployment tools'],
      ['prod-deploy', 'deploy', 'Prepare isolated deployment tools'],
    ]
    workflows.each do |name, job, consumer|
      steps = workflow(name).fetch('jobs').fetch(job).fetch('steps')
      restore = steps.index { |step| step['name'] == 'Restore public tool download cache' }
      discard = steps.index { |step| step['name'] == 'Discard an incomplete public tool cache restore' }
      required = steps.index { |step| step['name'] == consumer }
      assert_operator restore, :<, discard
      assert_operator discard, :<, required
      assert_equal 'public-tools-cache', steps[restore]['id']
      assert_equal true, steps[restore]['continue-on-error']
      assert_equal 1, steps[restore]['timeout-minutes']
      miss = "steps.public-tools-cache.outcome != 'success' || steps.public-tools-cache.outputs.cache-hit != 'true'"
      condition = name == 'staging-deploy' ? "steps.release.outputs.disposition == '' && (#{miss})" : miss
      assert_equal condition, steps[discard]['if']
      assert_equal 'rm -rf -- "${RUNNER_TEMP:?}/cma-public-tool-cache"', steps[discard]['run']
      refute steps[required].key?('continue-on-error'), 'pinned tool verification must remain required'
    end
    infrastructure = workflow('staging-cd').fetch('jobs').fetch('infrastructure').fetch('steps')
    save = infrastructure.find { |step| step['uses'].to_s.start_with?('actions/cache/save@') }
    assert_equal true, save['continue-on-error']
    assert_equal 2, save['timeout-minutes']
  end

  def test_planner_reads_final_release_tags_with_only_the_read_only_job_token
    plan = workflow('staging-cd')['jobs']['plan']
    step = plan['steps'].find { |candidate| candidate['id'] == 'plan' }
    assert_equal({ 'CD_ADOPTION_SHA' => '${{ vars.CMA_CD_ADOPTION_SHA }}', 'GH_TOKEN' => '${{ github.token }}' },
                 step['env'])
    assert_equal 'read', plan.dig('permissions', 'contents')
    refute_includes plan['permissions'].values, 'write'
  end
end

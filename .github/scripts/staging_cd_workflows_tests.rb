#!/usr/bin/env ruby
# These are authority/concurrency regressions, not snapshots of YAML layout.
require 'minitest/autorun'
require 'yaml'
require 'json'
require 'open3'

class StagingWorkflowTest < Minitest::Test
  ROOT = File.expand_path('..', __dir__)

  def workflow(name)
    YAML.load_file(File.join(ROOT, 'workflows', name + '.yml'))
  end

  def test_planner_reads_controller_outcomes_and_only_the_controller_can_emit_the_proof
    jobs = workflow('staging-cd')['jobs']
    plan = jobs.fetch('plan')
    assert_equal 'read', plan.dig('permissions', 'deployments')
    evidence = plan['steps'].find { |step| step['id'] == 'promotions' }
    assert_equal 'python3 -B .github/scripts/staging_cd_promotion.py', evidence['run']
    assert_equal '${{ steps.promotions.outputs.verified }}', plan.dig('outputs', 'controller_proofs')
    allocate = jobs['allocate']['steps'].find { |step| step['id'] == 'allocate' }
    assert_equal '${{ needs.plan.outputs.controller_proofs }}', allocate.dig('env', 'CD_CONTROLLER_PROOFS')
    steps = workflow('staging-deploy')['jobs']['deploy']['steps']
    writer = steps.index { |step| step['id'] == 'coordinate' }
    proof = steps.index { |step| step['name'] == 'Verified staging receipts ${{ steps.coordinate.outputs.controller_proof }}' }
    assert_operator writer, :<, proof
    assert_equal "always() && !cancelled() && steps.coordinate.outputs.controller_proof != ''", steps[proof]['if']
  end

  def test_publication_has_no_workflow_or_job_lock
    %w[staging-cd staging-release staging-promote staging-deploy redeploy-staging].each do |name|
      refute workflow(name).key?('concurrency'), "#{name} must not have workflow concurrency"
    end
    jobs = workflow('staging-cd')['jobs']
    assert_equal({'group' => 'cma-version-allocation', 'cancel-in-progress' => false}, jobs['allocate']['concurrency'])
    assert jobs.select { |key, _| key.start_with?('release_') }.values.none? { |job| job.key?('concurrency') }
    refute jobs['allocate']['steps'].any? { |step| step['run'].to_s.match?(/buildx|skopeo|deploy\.py/) }
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
    assert_equal 12, deploy['timeout-minutes']
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
      YAML.load_file(file)['jobs'].each do |id, job|
        next unless [promote, writer].include?(job['uses'])
        if name == 'staging-promote' && id == 'deploy'
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
    assert_equal 'write', final['permissions']['contents']
    refute final['permissions'].key?('packages')
    refute workflow('redeploy-staging').to_s.include?('packages')
    refute workflow('redeploy-staging').to_s.include?('contents"=>"write')
    deploy = workflow('staging-deploy')['jobs']['deploy']
    keyed = deploy['steps'].select { |step| step.fetch('env', {}).key?('CD_NYXID_CLIENT_SECRET') }
    assert_equal 1, keyed.length
    assert_equal '${{ secrets.CD_NYXID_CLIENT_ID }}', keyed.first['env']['CD_NYXID_CLIENT_ID']
    assert_equal '${{ secrets.CD_NYXID_CLIENT_SECRET }}', keyed.first['env']['CD_NYXID_CLIENT_SECRET']
    refute deploy.to_s.include?('NYXID_CHRONO_CD_KEY')
    assert_equal '"$RUNNER_TEMP/cma-deploy-tools/venv/bin/python" -B .github/scripts/staging_cd_invoke.py', keyed.first['run']
    setup = deploy['steps'].find { |step| step['run'] == 'python3 -B .github/scripts/staging_cd_tools.py install' }
    assert setup
    assert_operator deploy['steps'].index(setup), :<, deploy['steps'].index(keyed.first)
    assert_equal({'CD_VENV_SHA256' => '${{ inputs.venv_sha256 }}'}, setup.fetch('env'))
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

  def test_deploy_uses_only_scoped_checkouts_and_finishes_public_setup_before_token_creation
    steps = workflow('staging-deploy').dig('jobs', 'deploy', 'steps')
    checkouts = steps.select { |step| step['uses'].to_s.start_with?('actions/checkout@') }
    assert_equal 2, checkouts.size
    public_paths = checkouts.first.dig('with', 'sparse-checkout').lines.map(&:strip)
    assert_equal %w[/.github/scripts/staging_cd_*.py !/.github/scripts/*_tests.py
                    /.github/scripts/staging_cd_wheels.json /.github/scripts/deployables.py
                    /.github/scripts/deployables.json /.github/scripts/ci_tracks/deployable_manifest.py], public_paths
    assert_equal '/staging/', checkouts.last.dig('with', 'sparse-checkout')
    checkouts.each do |step|
      assert_equal 1, step.dig('with', 'fetch-depth')
      assert_equal false, step.dig('with', 'persist-credentials')
      assert_equal false, step.dig('with', 'sparse-checkout-cone-mode')
      refute step.dig('with', 'filter'), 'checkout filter overrides sparse-checkout'
    end
    assert_equal '${{ steps.private.outputs.revision }}', checkouts.last.dig('with', 'ref')
    token = steps.index { |step| step['id'] == 'infra-token' }
    public_setup = steps.index { |step| step['run'].to_s.end_with?('staging_cd_tools.py install') }
    clock = steps.index { |step| step['run'].to_s.end_with?('staging_cd_clock.py') }
    verify = steps.index { |step| step.dig('env', 'CD_RESOLVE_MODE') == 'verify' }
    assert_operator steps.index(checkouts.first), :<, clock
    assert_operator clock, :<, verify
    assert_operator verify, :<, steps.index { |step| step['id'] == 'tools-cache' }
    assert_equal public_setup + 1, token
    assert_equal 'read', steps[token].dig('with', 'permission-contents')
    assert_equal checkouts.last.dig('with', 'repository').split('/').last, steps[token].dig('with', 'repositories')
    assert_equal 'private', steps[token + 1]['id']
    assert_equal checkouts.last, steps[token + 2]
    assert_equal 'python3 -B .github/scripts/staging_cd_tools.py requirements', steps[token + 3]['run']
    assert_equal 'coordinate', steps[token + 4]['id']
  end

  def test_deploy_cache_uses_the_digest_from_the_trusted_preparation_job
    jobs = workflow('staging-cd').fetch('jobs')
    producer = jobs.fetch('infrastructure')
    steps = producer.fetch('steps')
    prepare = steps.index { |step| step['run'].to_s.end_with?('staging_cd_tools.py prepare-venv-cache') }
    saves = steps.select { |step| step['uses'].to_s.start_with?('actions/cache/save@') }
    inputs_key = saves.first.dig('with', 'key')
    assert_includes inputs_key, "hashFiles('.github/scripts/staging_cd_tools.py', '.github/scripts/staging_cd_wheels.json')"
    %w[steps.python.outputs.python-version steps.python.outputs.python-path runner.temp].each do |binding|
      assert_includes inputs_key, binding
    end
    assert_operator steps.index(saves.first), :<, prepare
    assert_operator prepare, :<, steps.index(saves.last)
    assert_operator steps.index(saves.last), :<, steps.index { |step| step['id'] == 'infra-token' }
    assert_equal inputs_key.sub('downloads', '${{ steps.venv.outputs.sha256 }}'), saves.last.dig('with', 'key')
    assert_equal '${{ steps.venv.outputs.sha256 }}', producer.dig('outputs', 'venv_sha256')
    jobs.values.select { |job| job['uses'] == './.github/workflows/staging-promote.yml' }.each do |job|
      assert_equal '${{ needs.infrastructure.outputs.venv_sha256 }}', job.dig('with', 'venv_sha256')
    end
    assert_equal '${{ inputs.venv_sha256 }}', workflow('staging-promote').dig('jobs', 'deploy', 'with', 'venv_sha256')
    deploy = workflow('staging-deploy').dig('jobs', 'deploy', 'steps')
    restore = deploy.find { |step| step['uses'].to_s.start_with?('actions/cache/restore@') }
    assert_equal inputs_key.sub('downloads', "${{ inputs.venv_sha256 || 'downloads' }}"), restore.dig('with', 'key')
    fallback = deploy.fetch(deploy.index(restore) + 1)
    assert_equal "inputs.venv_sha256 != '' && steps.tools-cache.outputs.cache-hit != 'true'", fallback['if']
    assert_equal inputs_key, fallback.dig('with', 'key')
    assert_equal saves.last.dig('with', 'path'), restore.dig('with', 'path')
    assert_equal restore.dig('with', 'path'), fallback.dig('with', 'path')
    refute fallback['with'].key?('restore-keys')
    refute deploy.any? { |step| step['uses'].to_s.start_with?('actions/cache/save@') }
    refute workflow('redeploy-staging').dig('jobs', 'deploy', 'with').key?('venv_sha256')
  end

  def test_secret_forwarding_is_explicit_at_each_promotion_hop
    keys = %w[CMA_INFRA_APP_PRIVATE_KEY CD_NYXID_CLIENT_ID CD_NYXID_CLIENT_SECRET]
    expected = keys.to_h { |key| [key, "${{ secrets.#{key} }}"] }
    %w[staging-promote staging-deploy].each do |name|
      config = workflow(name)
      assert_equal keys.to_h { |key| [key, {'required' => true}] }, (config['on'] || config[true])['workflow_call']['secrets']
      reads = config.fetch('jobs').to_s.scan(/secrets\.([A-Za-z0-9_]+)/).flatten.uniq.sort
      assert_equal keys.sort, reads
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
      assert_equal 'allocate', publisher['needs']
      assert_equal './.github/workflows/staging-release.yml', publisher['uses']
      assert_equal component, publisher.dig('with', 'component')
      assert_equal "Release #{component}", publisher['name'] # detector-cache producer identity
      assert_equal %W[allocate infrastructure release_#{id}], promote['needs']
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
    download = steps.find { |step| step['uses'].to_s.start_with?('actions/download-artifact@') }
    finalizer = steps.find { |step| step['run'].to_s.include?('staging_cd_finalize.py') }
    assert_equal '${{ needs.publish.outputs.artifact }}', download.dig('with', 'artifact-ids')
    refute download['with'].key?('pattern')
    refute download['with'].key?('name')
    config = {action: download['uses'], inputs: download['with'],
              receiptPath: finalizer.dig('env', 'CD_RECEIPTS_PATH')}
    output, status = Open3.capture2e('node', File.join(__dir__, 'staging_cd_download_layout_tests.cjs'),
                                    stdin_data: JSON.generate(config))
    assert status.success?, output
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
    restore = steps.index { |step| step['uses'].to_s.start_with?('actions/cache/restore@') }
    save = steps.index { |step| step['uses'].to_s.start_with?('actions/cache/save@') }
    select = steps.index { |step| step['id'] == 'plan' }
    assert_operator prepare, :<, restore
    assert_operator restore, :<, select
    assert_operator select, :<, save
    refute steps[save].key?('if'), 'the default success condition must guard cache writes'
    [steps[restore], steps[save]].each do |step|
      assert_equal '${{ steps.selection-cache.outputs.directory }}', step['with']['path']
      assert_equal '${{ steps.selection-cache.outputs.key }}', step['with']['key']
    end
    assert_equal '${{ steps.selection-cache.outputs.prefix }}', steps[restore]['with']['restore-keys']
    assert_equal 'ubuntu-24.04', plan['runs-on']
    refute plan.to_s.include?('secrets.')
  end
end

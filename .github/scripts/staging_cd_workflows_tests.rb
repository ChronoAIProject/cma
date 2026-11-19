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

  def test_planning_and_allocation_cannot_replace_pending_merge_jobs
    %w[staging-cd staging-release staging-deploy redeploy-staging].each do |name|
      refute workflow(name).key?('concurrency'), "#{name} must not have workflow concurrency"
    end
    jobs = workflow('staging-cd')['jobs']
    assert jobs.values.none? { |job| job.key?('concurrency') }
    refute jobs['allocate']['steps'].any? { |step| step['run'].to_s.match?(/buildx|skopeo|deploy\.py/) }
  end

  def test_all_mutation_paths_share_one_set_lock_without_serializing_publication
    manual = workflow('redeploy-staging')['jobs']['deploy']
    automatic = workflow('staging-cd')['jobs']['deploy']
    assert_equal manual['uses'], automatic['uses']
    refute manual.key?('strategy')
    refute workflow('staging-release')['jobs'].key?('deploy')
    assert_equal false, workflow('staging-cd')['jobs']['release']['strategy']['fail-fast']
    deploy = workflow('staging-deploy')['jobs']['deploy']
    assert_equal({'group' => 'cma-staging-deploy-set', 'queue' => 'max'}, deploy['concurrency'])
    assert_equal 'chrono-ai-staging', deploy['environment']
    assert_equal 12, deploy['timeout-minutes']
    assert_equal 'release', automatic['with']['mode']
    verification = deploy['steps'].index { |step| step.dig('env', 'CD_RESOLVE_MODE') == 'verify' }
    writer = deploy['steps'].index { |step| step['run'].to_s.include?('staging_cd_invoke.py') }
    assert verification
    assert_operator verification, :<, writer
    assert_includes workflow('staging-cd')['jobs']['admit']['needs'], 'release'
    refute workflow('staging-cd')['jobs']['release']['needs'].include?('deploy')
    %w[release_set mode started_at infra_ref].each do |key|
      assert manual['with'].key?(key)
      assert automatic['with'].key?(key)
    end
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
    %w[staging-cd staging-release redeploy-staging].each do |name|
      %w[NYXID_CHRONO_CD_KEY CD_NYXID_CLIENT_ID CD_NYXID_CLIENT_SECRET].each do |field|
        refute workflow(name).to_s.include?(field)
      end
    end
  end

  def test_partial_publication_reaches_admission_but_failure_still_reaches_report
    jobs = workflow('staging-cd')['jobs']
    admission = jobs['admit']
    assert_includes admission['if'], 'always()'
    assert_includes admission['if'], '!cancelled()'
    assert_includes admission['if'], "needs.allocate.result == 'success'"
    assert_includes admission['if'], "needs.infrastructure.result == 'success'"
    refute_includes admission['if'], 'needs.release.result'
    assert_includes admission['needs'], 'release'
    assert_includes jobs['deploy']['if'], "needs.admit.result == 'success'"
    assert_includes jobs['deploy']['if'], "needs.admit.outputs.changed == 'true'"
    assert_equal '${{ steps.set.outputs.failures }}', admission['outputs']['failures']
    evidence = admission['steps'].find { |step| step['uses'].to_s.start_with?('actions/upload-artifact@') }
    assert_equal 'always()', evidence['if']
    assert_equal 'staging-admission-${{ github.run_id }}-${{ github.run_attempt }}', evidence['with']['name']
    assert_equal '${{ runner.temp }}/public-admission/admission.json', evidence['with']['path']
    report = jobs['recheck']['steps'].find { |step| step['run'].to_s.include?('staging_cd_set_report.py') }
    assert_equal 'always()', report['if']
    assert_equal '${{ needs.admit.outputs.failures }}', report['env']['CD_COMPONENT_FAILURES']
    assert_equal '${{ needs.release.result }}', report['env']['CD_PUBLICATION_JOB_RESULT']
    assert_equal '${{ needs.admit.outputs.release_set }}', report['env']['CD_RELEASE_SET']
    assert_equal 'Release ${{ matrix.component }}', jobs['release']['name']
    assert_equal 'Publish', workflow('staging-release')['jobs']['publish']['name']
    assert_equal 'Finalize', workflow('staging-release')['jobs']['finalize']['name']
    assert_equal 'Admit qualified components', admission['name']
    assert_equal 'Deploy admitted components', jobs['deploy']['name']
    assert_equal 'Deploy set', workflow('staging-deploy')['jobs']['deploy']['name']
    assert_equal 'Report component outcomes', jobs['recheck']['name']
  end

  def test_all_external_actions_are_pinned_and_only_sanitized_deployment_outcomes_upload
    %w[staging-cd staging-release staging-deploy redeploy-staging].each do |name|
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
    assert_equal 'staging-deployment-${{ github.run_id }}-${{ github.run_attempt }}', upload['with']['name']
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

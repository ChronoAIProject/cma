#!/usr/bin/env ruby
# These are authority/concurrency regressions, not snapshots of YAML layout.
require 'minitest/autorun'
require 'yaml'

class StagingWorkflowTest < Minitest::Test
  ROOT = File.expand_path('..', __dir__)

  def workflow(name)
    YAML.load_file(File.join(ROOT, 'workflows', name + '.yml'))
  end

  def test_no_workflow_lock_and_only_short_allocation_has_global_group
    %w[staging-cd staging-release staging-deploy redeploy-staging].each do |name|
      refute workflow(name).key?('concurrency'), "#{name} must not have workflow concurrency"
    end
    jobs = workflow('staging-cd')['jobs']
    assert_equal({'group' => 'cma-version-allocation', 'cancel-in-progress' => false}, jobs['allocate']['concurrency'])
    assert jobs.reject { |key, _| key == 'allocate' }.values.none? { |job| job.key?('concurrency') }
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
    assert_equal({'group' => 'cma-staging-deploy-set', 'cancel-in-progress' => false}, deploy['concurrency'])
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

  def test_all_external_actions_are_commit_pinned_and_private_steps_upload_nothing
    %w[staging-cd staging-release staging-deploy redeploy-staging].each do |name|
      workflow(name)['jobs'].each_value do |job|
        job.fetch('steps', []).each do |step|
          next unless step['uses'] && !step['uses'].start_with?('./')
          assert_match(/\A[^@]+@[a-f0-9]{40}\z/, step['uses'])
        end
      end
    end
    deploy = workflow('staging-deploy')
    refute deploy.to_s.match?(/upload-artifact|set -x|pull_request_target/)
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
end

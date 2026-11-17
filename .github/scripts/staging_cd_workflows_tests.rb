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

  def test_parallel_redeploy_and_automatic_deploy_share_exact_per_app_lock
    manual = workflow('redeploy-staging')['jobs']['deploy']
    automatic = workflow('staging-release')['jobs']['deploy']
    assert_equal manual['uses'], automatic['uses']
    assert_equal false, manual['strategy']['fail-fast']
    deploy = workflow('staging-deploy')['jobs']['deploy']
    assert_equal({'group' => 'cma-staging-deploy-${{ inputs.component }}', 'cancel-in-progress' => false}, deploy['concurrency'])
    assert_equal 'chrono-ai-staging', deploy['environment']
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
    keyed = deploy['steps'].select { |step| step.fetch('env', {}).key?('NYXID_CHRONO_CD_KEY') }
    assert_equal 1, keyed.length
    assert_equal 'python3 -B .github/scripts/staging_cd_invoke.py', keyed.first['run']
    %w[staging-cd staging-release redeploy-staging].each do |name|
      refute workflow(name).to_s.include?('NYXID_CHRONO_CD_KEY')
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

  def test_final_ruleset_pattern_excludes_deletable_pending_tags
    assert File.fnmatch?('*/v*', 'cma-web/v0.10.1', File::FNM_PATHNAME)
    refute File.fnmatch?('*/v*', 'cma-web/pending/v0.10.1', File::FNM_PATHNAME)
    assert File.fnmatch?('refs/tags/*/v*', 'refs/tags/cma-web/v0.10.1', File::FNM_PATHNAME)
    refute File.fnmatch?('refs/tags/*/v*', 'refs/tags/cma-web/pending/v0.10.1', File::FNM_PATHNAME)
  end
end

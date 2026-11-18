#!/usr/bin/env ruby
# Authority and publication-order regressions across the public workflow boundary.
require 'minitest/autorun'
require 'yaml'

class StagingWorkflowTest < Minitest::Test
  ROOT = File.expand_path('..', __dir__)

  def workflow(name)
    YAML.load_file(File.join(ROOT, 'workflows', name + '.yml'))
  end

  def test_only_allocation_serializes_and_publication_remains_independent
    %w[staging-cd staging-release].each { |name| refute workflow(name).key?('concurrency') }
    jobs = workflow('staging-cd')['jobs']
    assert_equal({'group' => 'cma-version-allocation', 'cancel-in-progress' => false}, jobs['allocate']['concurrency'])
    assert jobs.reject { |key, _| key == 'allocate' }.values.none? { |job| job.key?('concurrency') }
    assert_equal false, jobs['release']['strategy']['fail-fast']
    assert_equal %w[allocate release], jobs['notify']['needs']
    assert_includes jobs['notify']['if'], "needs.release.result == 'success'"
    assert_includes jobs['recheck']['needs'], 'notify'
  end

  def test_public_workflows_have_no_deployment_or_private_checkout_authority
    %w[staging-deploy redeploy-staging].each do |name|
      refute File.exist?(File.join(ROOT, 'workflows', name + '.yml'))
    end
    %w[staging-cd staging-release].each do |name|
      value = workflow(name)
      refute value.to_s.match?(/NYXID|CMA_INFRA|staging_cd_invoke|staging_cd_checkout/)
      refute value['jobs'].key?('deploy')
      value['jobs'].each_value do |job|
        job.fetch('steps', []).each do |step|
          next unless step['uses'] && !step['uses'].start_with?('./')
          assert_match(/\A[^@]+@[a-f0-9]{40}\z/, step['uses'])
          refute step.fetch('with', {}).key?('repository')
        end
      end
    end
  end

  def test_signal_credentials_are_isolated_from_builds_and_artifact_precedes_delivery
    image = workflow('staging-release')['jobs']['publish']
    final = workflow('staging-release')['jobs']['finalize']
    assert_equal 'read', image['permissions']['contents']
    assert_equal 'write', image['permissions']['packages']
    assert_equal 'write', final['permissions']['contents']
    refute final['permissions'].key?('packages')
    refute workflow('staging-release').to_s.include?('PUBLICATION_SIGNAL_TOKEN')
    notify = workflow('staging-cd')['jobs']['notify']
    assert_equal 'image-publication', notify['environment']
    assert_equal({'contents' => 'read', 'actions' => 'read'}, notify['permissions'])
    steps = notify['steps']
    freeze = steps.index { |s| s['run'].to_s.end_with?('publication_signal.py freeze') }
    artifact = steps.index { |s| s['uses'].to_s.start_with?('actions/upload-artifact@') }
    send = steps.index { |s| s['run'].to_s.end_with?('publication_signal.py send') }
    assert_operator freeze, :<, artifact
    assert_operator artifact, :<, send
    assert_equal [send], steps.each_index.select { |i| steps[i].fetch('env', {}).key?('PUBLICATION_SIGNAL_TOKEN') }
    assert_equal '${{ secrets.PUBLICATION_CONSUMER }}', steps[send]['env']['PUBLICATION_CONSUMER']
    refute notify.to_s.match?(/set -x|pull_request_target/)
  end

  def test_final_ruleset_pattern_excludes_deletable_pending_tags
    assert File.fnmatch?('refs/tags/*/v*', 'refs/tags/cma-web/v0.10.1', File::FNM_PATHNAME)
    refute File.fnmatch?('refs/tags/*/v*', 'refs/tags/cma-web/pending/v0.10.1', File::FNM_PATHNAME)
  end

  def test_selector_keeps_shared_pinned_requirements
    steps = workflow('staging-cd')['jobs']['plan']['steps']
    setup = steps.find { |s| s['uses'].to_s.start_with?('actions/setup-python@') }
    assert_equal '3.12', setup.dig('with', 'python-version')
    install = steps.find { |s| s['name'] == 'Install shared selector parsers' }
    assert_includes install['run'], '-r .github/scripts/ci_tracks/requirements.txt'
    assert_operator steps.index(setup), :<, steps.index(install)
    assert_operator steps.index(install), :<, steps.index { |s| s['id'] == 'plan' }
  end
end

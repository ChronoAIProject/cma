# frozen_string_literal: true

require "minitest/autorun"
require "tmpdir"
require "fileutils"
require_relative "check-public-tree"

class PublicTreeTests < Minitest::Test
  def test_forbidden_directories_and_state_never_use_fixture_exceptions
    %w[infra/api/file docs/note.md sdk/client.ts infra/tests/fixtures/example.yaml cache/prod.tfstate backup.tfstate.backup].each do |path|
      refute_nil PublicTree.violation(path, ""), path
    end
  end

  def test_repository_evidence_path_is_forbidden_even_with_fixture_names
    %w[.github/evidence .github/evidence/captured.json .github/evidence/fixtures/example.json].each do |path|
      assert_equal "private repository path", PublicTree.violation(path, "{}"), path
    end
    assert_nil PublicTree.violation(".github/evidence-check.json", "{}")
    assert_nil PublicTree.violation(".github/scripts/fixtures/evidence/example.json", "{}")
  end

  def test_component_build_recipes_are_permitted
    PublicTree::DOCKERFILES.each { |path| assert_nil PublicTree.violation(path, "FROM scratch\n") }
    refute_nil PublicTree.violation("docker/api/Dockerfile", "FROM scratch\n")
    refute_nil PublicTree.violation("web/Dockerfile.production", "FROM scratch\n")
    assert_nil PublicTree.violation(".github/scripts/fixtures/previous.Dockerfile", "FROM scratch\n")
  end

  def test_workflows_and_real_test_fixtures_are_permitted
    assert_nil PublicTree.violation(".github/workflows/ci.yml", "name: Checks\non: [push]\njobs: {}\n")
    assert_nil PublicTree.violation("api/crates/example/tests/fixtures/workload.yaml", "kind: Deployment\n")
    assert_nil PublicTree.violation("web/src/example/fixtures/workload.yaml", "kind: Deployment\n")
    refute_nil PublicTree.violation("fixtures/prod.yaml", "kind: Deployment\n")
  end

  def test_block_flow_multidocument_and_custom_resources_are_rejected
    ["kind: Deployment\n", "{kind: Service}\n", "name: workflow\n---\nkind: ConfigMap\n",
     "apiVersion: custom.example/v1\nkind: ExampleResource\n", "items: [{kind: Secret}]\n",
     "kind: [broken"].each do |content|
      refute_nil PublicTree.violation("configuration/workload.yaml", content)
    end
  end

  def test_nondeployment_application_data_is_permitted
    assert_nil PublicTree.violation("web/settings.yaml", "theme: dark\n")
    assert_nil PublicTree.violation("web/settings.json", '{"kind":"Service"}')
  end

  def test_tracked_additions_are_checked_without_reading_deleted_files
    Dir.mktmpdir("public-tree-") do |root|
      _out, error, status = Open3.capture3("git", "init", "--quiet", root)
      assert status.success?, error
      File.write(File.join(root, "workload.yaml"), "kind: Deployment\n")
      _out, error, status = Open3.capture3("git", "-C", root, "add", "workload.yaml")
      assert status.success?, error
      assert_equal [["workload.yaml", "deployment manifest outside a test fixture"]], PublicTree.check(root)
      File.unlink(File.join(root, "workload.yaml"))
      assert_empty PublicTree.check(root)

      evidence = ".github/evidence/fixtures/example.json"
      FileUtils.mkdir_p(File.dirname(File.join(root, evidence)))
      File.write(File.join(root, evidence), "{}\n")
      _out, error, status = Open3.capture3("git", "-C", root, "add", evidence)
      assert status.success?, error
      assert_equal [[evidence, "private repository path"]], PublicTree.check(root)
    end
  end
end

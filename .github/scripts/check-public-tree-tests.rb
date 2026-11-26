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

  def test_generated_python_bytecode_is_rejected_everywhere
    %w[
      .github/scripts/ci_tracks/__pycache__/selection.cpython-314.pyc
      .github/scripts/__pycache__/ci_retry.cpython-312.pyc
      api/tools/legacy.pyc
      web/scripts/old.pyo
      .github/scripts/fixtures/__pycache__/fixture.cpython-312.pyc
    ].each do |path|
      assert_equal "generated Python bytecode is never tracked", PublicTree.violation(path, ""), path
    end
    %w[.github/scripts/ci_retry.py .github/scripts/pycache_notes.md].each do |path|
      assert_nil PublicTree.violation(path, ""), path
    end
  end

  def test_private_github_paths_are_rejected_before_fixture_exceptions
    %w[.github/evidence .github/scripts/cma_scale].each do |path|
      assert_equal "private repository path", PublicTree.violation(path, ""), path
      assert_equal "private repository path", PublicTree.violation("#{path}/fixtures/example.json", "{}"), path
      assert_nil PublicTree.violation("#{path}-check.json", "{}"), path
    end
    assert_nil PublicTree.violation(".github/scripts/fixtures/evidence/example.json", "{}")
  end

  def test_staged_private_github_fixtures_are_rejected
    Dir.mktmpdir("public-tree-") do |root|
      _out, error, status = Open3.capture3("git", "init", "--quiet", root)
      assert status.success?, error
      paths = %w[.github/evidence/fixtures/example.json .github/scripts/cma_scale/fixtures/example.json]
      paths.each do |path|
        FileUtils.mkdir_p(File.dirname(File.join(root, path)))
        File.write(File.join(root, path), "{}\n")
      end
      _out, error, status = Open3.capture3("git", "-C", root, "add", *paths)
      assert status.success?, error
      assert_equal paths.map { |path| [path, "private repository path"] }, PublicTree.check(root)
    end
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
    end
  end
end

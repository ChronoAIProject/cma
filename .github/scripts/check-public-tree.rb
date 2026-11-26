#!/usr/bin/env ruby
# frozen_string_literal: true

require "open3"
require "pathname"
require "yaml"

module PublicTree
  DOCKERFILES = %w[
    web/Dockerfile
    api/crates/cma/Dockerfile
    api/crates/cmaeg/Dockerfile
    api/crates/layer4-runtime/Dockerfile
  ].freeze

  def self.fixture?(path)
    path.match?(%r{\A(?:api/crates/[^/]+/(?:tests|src)/|web/(?:src|e2e)/|\.github/(?:scripts/)?).*\bfixtures/})
  end

  def self.manifest?(node)
    return false unless node.respond_to?(:children) && node.children
    if node.is_a?(Psych::Nodes::Mapping)
      fields = node.children.each_slice(2).to_a
      keys = fields.map { |key, _| key.value if key.is_a?(Psych::Nodes::Scalar) }.compact
      return true if keys.include?("apiVersion") && keys.include?("kind")
      return true if fields.any? do |key, value|
        key.is_a?(Psych::Nodes::Scalar) && key.value == "kind" &&
          value.is_a?(Psych::Nodes::Scalar) && value.value.match?(/\A(?:Deployment|Service|ConfigMap|Secret|Pod|Job|CronJob|StatefulSet|DaemonSet|Ingress|Namespace|ServiceAccount|Role|RoleBinding|ClusterRole|ClusterRoleBinding|PersistentVolume|PersistentVolumeClaim|StorageClass|HTTPRoute|Gateway|HorizontalPodAutoscaler|PodDisruptionBudget|List)\z/)
      end
    end
    node.children.any? { |child| manifest?(child) }
  end

  def self.violation(path, content)
    return "private repository path" if path.match?(%r{\A(?:infra|docs|sdk|\.github/(?:evidence|scripts/cma_scale))(?:/|\z)})
    return "Terraform state" if File.basename(path).match?(/\.tfstate(?:\..*)?\z/i)
    # Bytecode is generated (CLAUDE.md section 8) and embeds the absolute path
    # of the tree that compiled it, which publishes a local home directory.
    if path.match?(%r{(?:\A|/)__pycache__/}) || path.match?(/\.py[co]\z/i)
      return "generated Python bytecode is never tracked"
    end
    recipe = File.basename(path).match?(/\A(?:Dockerfile(?:\..*)?|.+\.Dockerfile)\z/)
    if recipe && !fixture?(path) && !DOCKERFILES.include?(path)
      return "Dockerfile outside its owning component"
    end
    return nil unless path.match?(/\.ya?ml\z/i) && !fixture?(path)
    return "deployment manifest outside a test fixture" if manifest?(Psych.parse_stream(content))
    nil
  rescue Psych::SyntaxError
    "invalid YAML cannot be checked safely"
  end

  def self.check(root)
    files, error, status = Open3.capture3("git", "-C", root, "ls-files", "-z")
    raise "cannot enumerate tracked paths: #{error}" unless status.success?
    failures = []
    files.split("\0").each do |path|
      source = File.join(root, path)
      next unless File.exist?(source) || File.symlink?(source)
      reason = violation(path, "")
      if !reason && path.match?(/\.ya?ml\z/i)
        reason = if File.symlink?(source)
          "YAML symlinks cannot be checked safely"
        else
          violation(path, File.read(source, encoding: "UTF-8"))
        end
      end
      failures << [path, reason] if reason
    end
    failures
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    failures = PublicTree.check(File.expand_path("../..", __dir__))
    failures.each { |path, reason| warn "::error file=#{path}::#{reason}" }
    abort "Public repository boundary rejected #{failures.length} path(s)" unless failures.empty?
    puts "Public repository boundary passed"
  rescue StandardError => error
    warn "Public repository boundary failed: #{error.class}"
    exit 1
  end
end

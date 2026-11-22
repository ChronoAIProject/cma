#!/usr/bin/env ruby
# frozen_string_literal: true

require "open3"
require "yaml"

# The public GHCR package cma-omarchy-packages holds the reviewed Omarchy Arch
# package bundle. Image builds mount it by digest (OMARCHY_PACKAGES_IMAGE in
# api/crates/layer4-runtime/Dockerfile), so deleting any version breaks every
# build, and every rollback, that pins it. The package stays public and
# outside every version cleanup.
#
# A deliberate snapshot bump publishes a new version with the bundle workflow
# and repins the Dockerfile; older versions stay for rollback. No file under
# .github may delete a version, or the package itself: not through the
# packages REST API (curl, gh api, Octokit), and not through a cleanup action
# pointed at the bundle, at every package or at a name computed at run time.
module OmarchyPackageRetention
  PACKAGE = "cma-omarchy-packages"
  # The contract and its tests hold the refused forms as data and never call
  # the API. No other path is exempt.
  EXEMPT = %w[
    .github/scripts/omarchy-package-retention.rb
    .github/scripts/omarchy-package-retention-tests.rb
  ].freeze
  # Cleanup actions and the inputs naming the packages they delete. A step
  # that sets none of them is treated as targeting every package.
  CLEANUP_ACTIONS = {
    "actions/delete-package-versions" => %w[package-name],
    "snok/container-retention-policy" => %w[image-names],
    "dataaxiom/ghcr-cleanup-action" => %w[package packages],
    "vlaurin/action-ghcr-prune" => %w[container],
    "bots-house/ghcr-delete-image-action" => %w[name],
  }.freeze
  PACKAGE_TYPES = %w[container docker npm maven rubygems nuget].freeze
  DELETE_VERB = /(?<![A-Za-z])DELETE\b|-XDELETE\b/
  DELETE_FLAG = /(?:-X|--request|--method)[\s=]*["']?delete\b/i
  OCTOKIT_DELETE = /\bdeletePackage(?:Version)?For(?:Org|User|AuthenticatedUser)\b/
  # The packages/{type}/{name} REST segment, whatever owner prefix precedes it.
  REST_PATH = %r{(?<![\w.-])packages/([^/\s"'`]+)/([^/\s"'`?#),;]*)}
  REST_LISTING = %r{(?<![\w.-])packages\?[^\s"'`]*package_type=}
  PACKAGE_NAME_ARG = /\bpackage_name["']?\s*[:=]\s*(?:"([^"\n]*)"|'([^'\n]*)'|(\S+))/
  COMPUTED = /[$%{}<>:`]/
  # A workflow expression may contain spaces; it is one computed value.
  EXPRESSION = /\$\{\{.*?\}\}/m
  WORKFLOW = %r{\A\.github/(?:workflows/[^/]+|actions/.+/action)\.ya?ml\z}

  # True when a targeted name is the bundle, a glob matching it, empty, or
  # only known at run time.
  def self.reaches?(name)
    name = name.to_s.strip
    name.empty? || name.match?(COMPUTED) || File.fnmatch?(name, PACKAGE, File::FNM_CASEFOLD)
  end

  def self.rest_targets(content)
    targets = content.scan(REST_PATH).select do |type, _name|
      PACKAGE_TYPES.include?(type.downcase) || type.match?(COMPUTED)
    end.map(&:last)
    content.match?(REST_LISTING) ? targets + ["*"] : targets
  end

  def self.octokit_targets(content)
    content.scan(PACKAGE_NAME_ARG).map { |double, single, bare| bare ? "$#{bare}" : double || single }
  end

  def self.cleanup_action(reference)
    reference = reference.downcase
    CLEANUP_ACTIONS.keys.find { |name| reference.match?(%r{(?:\A|/)#{Regexp.escape(name)}(?:[@/:]|\z)}) }
  end

  def self.cleanup_steps(node, found = [])
    case node
    when Hash
      found << node if node["uses"].is_a?(String) && cleanup_action(node["uses"])
      node.each_value { |value| cleanup_steps(value, found) }
    when Array
      node.each { |value| cleanup_steps(value, found) }
    end
    found
  end

  def self.action_violations(content)
    cleanup_steps(YAML.safe_load(content, aliases: true)).select do |step|
      inputs = step["with"].is_a?(Hash) ? step["with"] : {}
      names = CLEANUP_ACTIONS.fetch(cleanup_action(step["uses"])).flat_map do |key|
        inputs.key?(key) ? inputs[key].to_s.scan(/[^\s,]+/) : []
      end
      names.empty? || names.any? { |name| reaches?(name) }
    end.map { |step| "#{step['uses']} can delete versions of #{PACKAGE}" }
  rescue Psych::Exception
    ["workflow YAML cannot be checked safely"]
  end

  def self.violations(path, content)
    return [] if EXEMPT.include?(path)
    reasons = []
    text = content.gsub(EXPRESSION, "${{}}")
    if text.match?(DELETE_VERB) || text.match?(DELETE_FLAG)
      if rest_targets(text).any? { |name| reaches?(name) }
        reasons << "an HTTP DELETE can reach #{PACKAGE} through the packages REST API"
      end
    end
    if text.match?(OCTOKIT_DELETE)
      names = octokit_targets(text)
      if names.empty? || names.any? { |name| reaches?(name) }
        reasons << "an Octokit package deletion can reach #{PACKAGE}"
      end
    end
    reasons.concat(action_violations(content)) if path.match?(WORKFLOW)
    reasons
  end

  def self.check(root)
    files, error, status = Open3.capture3("git", "-C", root, "ls-files", "-z", "--", ".github")
    raise "cannot enumerate tracked paths: #{error}" unless status.success?
    files.split("\0").flat_map do |path|
      source = File.join(root, path)
      next [] unless File.file?(source)
      content = File.binread(source).force_encoding(Encoding::UTF_8).scrub
      violations(path, content).map { |reason| [path, reason] }
    end
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    failures = OmarchyPackageRetention.check(File.expand_path("../..", __dir__))
    failures.each { |path, reason| warn "::error file=#{path}::#{reason}" }
    abort "Omarchy package bundle retention rejected #{failures.length} finding(s)" unless failures.empty?
    puts "Omarchy package bundle retention passed"
  rescue StandardError => error
    warn "Omarchy package bundle retention failed: #{error.class}: #{error.message}"
    exit 1
  end
end

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
# packages REST API (curl, gh api, Octokit, fetch, urllib, requests, httpx,
# Net::HTTP), not through a registry client (skopeo, crane, oras, regctl), and
# not through a cleanup action pointed at the bundle, at every package or at a
# name computed at run time.
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
  # HTTP DELETE forms in any language. Any one of them makes the file's
  # packages targets deletion targets.
  DELETE_FORMS = [
    /(?<![A-Za-z])DELETE\b|-XDELETE\b/, # the verb as curl, Octokit and urllib spell it
    /(?:-X|--request|--method)[\s=]*["']?delete\b/i, # curl and gh api flags in any case
    /["'`]delete["'`]/i, # a quoted method in any case: fetch, axios, urllib, requests.request
    /\bNet::HTTP::Delete\b/, # Ruby's request class
    /\b(?:requests|httpx|axios)\.delete\s*\(/, # client module helpers
    %r{\.delete\s*\(\s*[bfr]{0,2}["'`](?:https?:|/)}i, # any client called with a literal URL or path
  ].freeze
  # A file using requests, httpx or Net::HTTP can delete through a session or
  # client of any name, so any delete call in it counts.
  HTTP_CLIENT = %r{\b(?:import|from)\s+(?:requests|httpx)\b|\brequire\s*\(?\s*["']net/https?["']}
  CLIENT_DELETE = /\bdelete\s*\(/
  OCTOKIT_DELETE = /\bdeletePackage(?:Version)?For(?:Org|User|AuthenticatedUser)\b/
  # The packages/{type}/{name} REST segment, whatever owner prefix precedes it.
  REST_PATH = %r{(?<![\w.-])packages/([^/\s"'`]+)/([^/\s"'`?#),;]*)}
  REST_LISTING = %r{(?<![\w.-])packages\?[^\s"'`]*package_type=}
  # The packages collection (orgs/x/packages, user/packages) also lists every
  # package when package_type arrives as a gh api field (-f/-F) or a client
  # parameter instead of in the query string.
  COLLECTION = %r{/packages(?=[\s"'`?,)\]]|\z)}
  PACKAGE_TYPE = /\bpackage_type\b/
  # Registry clients that delete a manifest or tag. Every argument after
  # "delete" that is not an option, or an option's value, is read as an image
  # reference; a command with none takes its reference from elsewhere.
  REGISTRY_CLIENT = %r{(?<![\w./-])(?:skopeo|g?crane|oras|regctl)(?![\w.-])([^\n;|&]*)}
  VALUE_OPTIONS = %w[--authfile --creds --cert-dir --registry-token --username --password -u -p
                     --registries-conf --policy --tmpdir --command-timeout --retry-times --retry-delay
                     --platform --config].freeze
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
    listing = content.match?(REST_LISTING) || (content.match?(COLLECTION) && content.match?(PACKAGE_TYPE))
    listing ? targets + ["*"] : targets
  end

  def self.http_delete?(content)
    DELETE_FORMS.any? { |form| content.match?(form) } ||
      (content.match?(HTTP_CLIENT) && content.match?(CLIENT_DELETE))
  end

  # The package a registry reference names: the last repository segment,
  # without transport, digest or tag.
  def self.image_name(reference)
    reference.sub(%r{\A[a-z][\w+.-]*://}i, "").split("@").first.to_s.split("/").last.to_s.split(":").first.to_s
  end

  def self.registry_targets(content)
    content.gsub(/\\\r?\n/, " ").scan(REGISTRY_CLIENT).flat_map do |(rest)|
      words = rest.split(/[\s,]+/).map { |word| word.delete("\"'\\\\[]()") }.reject(&:empty?)
      next [] unless (start = words.index("delete"))
      arguments = words.drop(start + 1)
      references = arguments.each_with_index.reject do |word, index|
        word.start_with?("-") || (index.positive? && VALUE_OPTIONS.include?(arguments[index - 1]))
      end.map(&:first)
      references.empty? ? [""] : references.map { |reference| image_name(reference) }
    end
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
    if http_delete?(text) && rest_targets(text).any? { |name| reaches?(name) }
      reasons << "an HTTP DELETE can reach #{PACKAGE} through the packages REST API"
    end
    if registry_targets(text).any? { |name| reaches?(name) }
      reasons << "a registry client deletion can reach #{PACKAGE}"
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

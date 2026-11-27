#!/usr/bin/env bash
set -euo pipefail

repo_root=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
ruby - "$repo_root" <<'RUBY'
require "shellwords"

EXPECTED = {
  "org.opencontainers.image.source" => "https://github.com/ChronoAIProject/cma",
  "org.opencontainers.image.revision" => "${CHRONO_BUILD_SHA}",
  "org.opencontainers.image.version" => "${CHRONO_BUILD}",
}.freeze

# This is a source regression guard, not installed-image provenance. Delivery
# must also verify the labels in each exact built platform manifest.
def validate(source)
  args = {}
  labels = {}
  source.gsub(/\\\r?\n/, " ").each_line do |line|
    instruction, body = line.strip.split(/\s+/, 2)
    case instruction&.upcase
    when "FROM"
      args = {}
      labels = {}
    when "ARG"
      name, default = body.split("=", 2)
      args[name] = default
    when "LABEL"
      Shellwords.split(body).each do |assignment|
        name, value = assignment.split("=", 2)
        raise "LABEL must use key=value syntax" unless value
        labels[name] = value
      end
    end
  end
  %w[CHRONO_BUILD CHRONO_BUILD_SHA].each do |name|
    raise "missing final-stage #{name} default" if args[name].to_s.empty?
  end
  EXPECTED.each do |name, value|
    raise "incorrect final-stage #{name}" unless labels[name] == value
  end
end

{
  "api" => "api/crates/cma",
  "web" => "web",
  "runtime" => "api/crates/layer4-runtime",
  "cmaeg" => "api/crates/cmaeg",
}.each do |name, directory|
  source = File.read(File.join(ARGV.fetch(0), directory, "Dockerfile"))
  validate(source)
  EXPECTED.each_key do |key|
    begin
      validate(source + "\nLABEL #{key}=incorrect\n")
    rescue RuntimeError
      next
    end
    abort "#{name}: accepted overridden #{key}"
  end
  begin
    validate(source + "\nFROM scratch\n")
  rescue RuntimeError
    puts "#{name}: final-stage metadata declarations and four rejection cases passed"
    next
  end
  abort "#{name}: accepted metadata only in an earlier stage"
end
RUBY

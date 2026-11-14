#!/usr/bin/env ruby
# Exercise Job dispatch with a local fake CLI; never contact Kubernetes/MongoDB.
require 'yaml'
require 'open3'
require 'tmpdir'
require 'shellwords'
require 'fileutils'

root = File.expand_path('../..', __dir__)
paths = Dir[File.join(root, 'infra/api/*index*job.yaml')]
jobs = paths.map { |path| YAML.load_file(path) }
raise 'duplicate Job names' unless jobs.map { |job| job.fetch('metadata').fetch('name') }.uniq.size == 4
scratch = File.join(root, 'api/target')
FileUtils.mkdir_p(scratch)
cases = 0
Dir.mktmpdir('index-job-contract-', scratch) do |directory|
  fake = File.join(directory, 'cma')
  File.write(fake, <<~SH)
    #!/bin/sh
    printf '%s\n' "$1" >> "$CALL_LOG"
    if [ "$1" = '--index-preflight' ]; then exit "$PREFLIGHT_EXIT"; fi
  SH
  File.chmod(0o700, fake)
  jobs.each do |job|
    spec = job.fetch('spec')
    raise 'automatic Job retry' unless spec.fetch('backoffLimit').zero?
    container = spec.fetch('template').fetch('spec').fetch('containers').first
    raise 'shell must stop on preflight failure' unless container.fetch('command') == ['/bin/sh', '-ec']
    script = container.fetch('args').join("\n")
    raise 'preflight must run first' unless script.lines.first.include?('--index-preflight')
    script = script.gsub('/usr/local/bin/cma', Shellwords.escape(fake))
    modes = if job.fetch('metadata').fetch('name') == 'cma-index-inspection'
              %w[preflight size build status verify abort]
            elsif script.include?('--activity-index-build')
              ['activity-index-build']
            else
              ['activity-index-size']
            end
    modes.each do |mode|
      [0, 1].each do |failure|
        calls = File.join(directory, 'calls')
        File.write(calls, '')
        env = {
          'CALL_LOG' => calls, 'PREFLIGHT_EXIT' => failure.to_s,
          'INDEX_OPERATION' => mode, 'INDEX_QUERIES' => 'activity-first-page',
          'INDEX_MANIFEST_CHECKSUM' => 'fixture', 'INDEX_RELEASE_IMAGE' => 'fixture',
          'INDEX_BUILD_TIMEOUT_SECONDS' => '1800', 'INDEX_PREVIOUS_FENCE' => '1',
          'ACTIVITY_INDEX_MANIFEST_CHECKSUM' => 'fixture', 'ACTIVITY_INDEX_RELEASE_IMAGE' => 'fixture',
          'ACTIVITY_INDEX_BUILD_TIMEOUT_SECONDS' => '1800'
        }
        _out, error, result = Open3.capture3(env, 'sh', '-ec', script)
        expected = ['--index-preflight']
        expected << (mode.start_with?('activity-') ? "--#{mode}" : "--index-#{mode}") if failure.zero? && mode != 'preflight'
        raise "unexpected CLI dispatch for #{mode}" unless File.readlines(calls, chomp: true) == expected
        raise "wrong exit for #{mode}: #{error}" unless result.exitstatus == failure
        cases += 1
      end
    end
  end
end
puts "PASS: #{cases} Job dispatch cases; failed preflight blocks every subsequent operation"

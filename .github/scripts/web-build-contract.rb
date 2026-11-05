#!/usr/bin/env ruby
require "yaml"

root = File.expand_path("../..", __dir__)

def validate(ci, publish, dockerfile)
  images = ci.fetch("jobs").fetch("images").fetch("steps")
  candidate = images.find { |step| step["id"] == "build-identity" }
  raise "candidate identity missing" unless candidate && candidate["run"] == 'echo "version=git-${GITHUB_SHA:0:12}" >> "$GITHUB_OUTPUT"'
  [images, publish.fetch("jobs").fetch("images").fetch("steps")].each_with_index do |steps, index|
    builds = steps.select { |step| step["uses"].to_s.start_with?("docker/build-push-action@") }
    raise "two build passes required" unless builds.length == 2
    version = index.zero? ? "${{ steps.build-identity.outputs.version }}" : "${{ needs.plan.outputs.version }}"
    builds.each do |build|
      args = build.fetch("with").fetch("build-args").lines.map(&:strip)
      raise "immutable build inputs missing" unless args.include?("CHRONO_BUILD=#{version}") && args.include?("CHRONO_BUILD_SHA=${{ github.sha }}")
    end
    gate = steps.find { |step| step["name"] == "Verify web bundle identity against image metadata" }
    raise "web image proof missing" unless gate && gate["run"].start_with?("python3 .github/scripts/web-build-identity.py ")
    expected_if = index.zero? ? "matrix.component == 'web'" : "success() && steps.reconcile.outputs.reuse == 'false' && matrix.component == 'web'"
    raise "web image proof bypassed" unless gate["if"] == expected_if && !gate.key?("continue-on-error")
    after = index.zero? ? "Export tested native image" : "Publish"
    raise "proof must precede transfer/publication" unless steps.index(gate) < steps.index { |step| step["name"] == after }
  end
  build_stage = dockerfile.split(/^FROM /)[1]
  %w[CHRONO_BUILD CHRONO_BUILD_SHA].each do |name|
    raise "missing build-stage #{name}" unless build_stage.include?("ARG #{name}=") && build_stage.include?("#{name}=$#{name}")
  end
  raise "image must require promotable metadata" unless build_stage.include?("CHRONO_BUILD_ARTIFACT=promotable")
end

ci = YAML.load_file(File.join(root, ".github/workflows/ci.yml"))
publish = YAML.load_file(File.join(root, ".github/workflows/publish.yml"))
dockerfile = File.read(File.join(root, "infra/web/Dockerfile"))
validate(ci, publish, dockerfile)
mutations = [
  ->(c, _p, _d) { c["jobs"]["images"]["steps"].each { |s| s.dig("with", "build-args")&.gsub!("CHRONO_BUILD=", "OMITTED_BUILD=") } },
  ->(_c, p, _d) { p["jobs"]["images"]["steps"].reject! { |s| s["name"] == "Verify web bundle identity against image metadata" } },
  ->(_c, _p, d) { d.gsub!("CHRONO_BUILD_SHA=$CHRONO_BUILD_SHA", "MISSING_SHA=yes") },
  ->(_c, _p, d) { d.gsub!("CHRONO_BUILD_ARTIFACT=promotable", "CHRONO_BUILD_ARTIFACT=local") },
]
mutations.each do |mutate|
  inputs = Marshal.load(Marshal.dump([ci, publish, dockerfile]))
  mutate.call(*inputs)
  begin
    validate(*inputs)
  rescue RuntimeError
    next
  end
  abort "web identity regression accepted"
end
puts "Web build inputs, image proof ordering and mutation checks passed"

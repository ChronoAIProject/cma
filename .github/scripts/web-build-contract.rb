#!/usr/bin/env ruby
require "yaml"

root = File.expand_path("../..", __dir__)

def validate(ci, publish, dockerfile)
  frontend = ci.fetch("jobs").fetch("frontend").fetch("steps")
  compiled = frontend.find { |step| step["name"] == "Production build" }
  raise "browser gate needs immutable bundle inputs" unless compiled && compiled["run"] == 'CHRONO_BUILD="git-${GITHUB_SHA:0:12}" npm run build' && compiled["env"] == { "CHRONO_BUILD_SHA" => "${{ github.sha }}", "CHRONO_BUILD_ARTIFACT" => "promotable" }
  images = ci.fetch("jobs").fetch("images").fetch("steps")
  candidate = images.find { |step| step["id"] == "build-identity" }
  raise "candidate identity missing" unless candidate && candidate["run"] == 'echo "version=git-${GITHUB_SHA:0:12}" >> "$GITHUB_OUTPUT"'
  builds = images.select { |step| step["uses"].to_s.start_with?("docker/build-push-action@") }
  raise "two build passes required" unless builds.length == 2
  builds.each do |build|
    args = build.fetch("with").fetch("build-args").lines.map(&:strip)
    raise "immutable build inputs missing" unless args.include?("CHRONO_BUILD=${{ steps.build-identity.outputs.version }}") && args.include?("CHRONO_BUILD_SHA=${{ github.sha }}")
  end
  gate = images.find { |step| step["name"] == "Verify web bundle identity against image metadata" }
  raise "web image proof missing" unless gate && gate["run"].start_with?("python3 .github/scripts/web-build-identity.py ")
  raise "web image proof bypassed" unless gate["if"] == "matrix.component == 'web'" && !gate.key?("continue-on-error")
  publication = images.index { |step| step["name"] == "Export tested native image" }
  raise "proof must precede transfer/publication" unless publication && images.index(gate) < publication
  # A release promotes the staging-verified web image and never builds one.
  publish.fetch("jobs").each_value do |job|
    raise "release must not rebuild images" if job.fetch("steps", []).any? { |step| step["uses"].to_s.start_with?("docker/build-push-action@") }
  end
  build_stage = dockerfile.split(/^FROM /)[1]
  %w[CHRONO_BUILD CHRONO_BUILD_SHA].each do |name|
    raise "missing build-stage #{name}" unless build_stage.include?("ARG #{name}=") && build_stage.include?("#{name}=$#{name}")
  end
  raise "image must require promotable metadata" unless build_stage.include?("CHRONO_BUILD_ARTIFACT=promotable")
end

ci = { "jobs" => {
  "frontend" => YAML.load_file(File.join(root, ".github/workflows/ci-web.yml")).fetch("jobs").fetch("web"),
  "images" => YAML.load_file(File.join(root, ".github/workflows/ci-images.yml")).fetch("jobs").fetch("images")
} }
publish = YAML.load_file(File.join(root, ".github/workflows/publish.yml"))
dockerfile = File.read(File.join(root, "web/Dockerfile"))
validate(ci, publish, dockerfile)
mutations = [
  ->(c, _p, _d) { c["jobs"]["images"]["steps"].each { |s| s.dig("with", "build-args")&.gsub!("CHRONO_BUILD=", "OMITTED_BUILD=") } },
  ->(c, _p, _d) { c["jobs"]["images"]["steps"].reject! { |s| s["name"] == "Verify web bundle identity against image metadata" } },
  ->(_c, p, _d) { p["jobs"]["images"]["steps"] << { "uses" => "docker/build-push-action@10e90e3645eae34f1e60eeb005ba3a3d33f178e8" } },
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

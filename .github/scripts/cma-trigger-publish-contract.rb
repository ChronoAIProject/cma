#!/usr/bin/env ruby

require "yaml"

ROOT = File.expand_path("../..", __dir__)
WORKFLOW_PATH = File.join(ROOT, ".github/workflows/publish.yml")
RELEASE_DOC_PATH = File.join(ROOT, "docs/Release.md")
READINESS_DOC_PATH = File.join(ROOT, "docs/CMA_Trigger_Release_Readiness.md")

COMPONENTS = %w[api web runtime].freeze
METADATA_KEYS = %w[
  archive
  archiveReference
  archiveSha256
  component
  fullRevision
  platforms
  revision
  revisionReference
  schemaVersion
  sourceDigest
  sourceReference
  sourceRepository
  version
  versionReference
].freeze
ARTIFACT_KEYS = %w[
  acpHash
  acpVersion
  architecture
  codexLauncherHash
  codexNativeHash
  codexVersion
  platform
  runtimeHash
  uid
].freeze

class ContractError < StandardError; end

def check(condition, message)
  raise ContractError, message unless condition
end

def required_hash(value, message)
  check(value.is_a?(Hash), message)
  value
end

def required_steps(job, job_name)
  steps = required_hash(job, "publish.yml must retain the #{job_name} job")["steps"]
  check(steps.is_a?(Array), "#{job_name} must define a step list")
  steps
end

def named_step(steps, name)
  steps.find { |step| step.is_a?(Hash) && step["name"] == name }
end

def required_step(steps, name)
  step = named_step(steps, name)
  check(step, "publish.yml must retain the #{name.inspect} step")
  step
end

def always_condition?(value)
  value.to_s.include?("always()")
end

def contains_in_order?(source, needles)
  offset = 0
  needles.all? do |needle|
    found = source.index(needle, offset)
    next false unless found

    offset = found + needle.length
    true
  end
end

def verify_contract(workflow, release_doc, readiness_doc)
  jobs = required_hash(workflow, "publish.yml must contain a workflow mapping")["jobs"]
  jobs = required_hash(jobs, "publish.yml must contain jobs")

  images = required_hash(jobs["images"], "publish.yml must retain the images job")
  check(
    images.dig("strategy", "matrix", "component") == COMPONENTS,
    "images matrix must cover exactly api, web, runtime",
  )
  image_steps = required_steps(images, "images")
  publish = required_step(image_steps, "Publish")
  publish_with = required_hash(publish["with"], "Publish must retain its build-push inputs")
  check(
    publish_with["outputs"] == "type=registry,oci-mediatypes=true",
    "Publish must retain digest-preserving OCI registry output",
  )
  check(publish_with["provenance"] == false, "Publish must keep provenance disabled")

  exports = required_hash(
    jobs["image-exports"],
    "publish.yml must retain the image-exports job",
  )
  check(exports["name"] == "Image export", "image-exports must remain named Image export")
  check(
    Array(exports["needs"]) == %w[plan images],
    "image-exports must depend exactly on plan and images",
  )
  check(
    exports.dig("strategy", "matrix", "component") == COMPONENTS,
    "image-exports matrix must cover exactly api, web, runtime",
  )
  export_steps = required_steps(exports, "image-exports")
  export = required_step(export_steps, "Export the published image")
  export_run = export["run"].to_s
  check(
    contains_in_order?(
      export_run,
      [
        ".github/export-release-image.sh",
        '"${{ matrix.component }}"',
        '"${{ needs.plan.outputs.version }}"',
        '"$GITHUB_SHA"',
        '"${{ steps.image.outputs.repository }}"',
        '"${{ steps.image.outputs.output }}"',
      ],
    ),
    "image-exports must invoke the exporter with all five landed arguments",
  )

  transfer = required_step(export_steps, "Upload the OCI transfer artifact")
  transfer_with = required_hash(
    transfer["with"],
    "OCI transfer upload must retain artifact inputs",
  )
  expected_artifact = 'cma-${{ matrix.component }}-${{ needs.plan.outputs.version }}-${{ steps.image.outputs.revision }}-oci'
  check(
    transfer_with["name"] == expected_artifact,
    "OCI transfer artifact name must retain component, version, and revision",
  )
  expected_paths = [
    '${{ steps.image.outputs.output }}/cma-${{ matrix.component }}-${{ needs.plan.outputs.version }}-${{ steps.image.outputs.revision }}.oci.tar',
    '${{ steps.image.outputs.output }}/cma-${{ matrix.component }}-${{ needs.plan.outputs.version }}-${{ steps.image.outputs.revision }}.json',
  ]
  actual_paths = transfer_with["path"].to_s.lines.map(&:strip).reject(&:empty?)
  check(actual_paths == expected_paths, "OCI transfer must contain exactly its archive and metadata")
  check(transfer_with["retention-days"] == 1, "OCI transfer retention must remain one day")
  check(transfer_with["compression-level"] == 0, "OCI transfer upload must disable recompression")
  check(transfer_with["overwrite"] == true, "OCI transfer retry must replace its same-named artifact")
  check(
    transfer_with["if-no-files-found"] == "error",
    "OCI transfer must fail when either exported file is missing",
  )

  gate = required_hash(
    jobs["runtime-image-gate"],
    "publish.yml must retain the runtime-image-gate job",
  )
  check(
    Array(gate["needs"]) == %w[plan images image-exports],
    "runtime-image-gate must depend exactly on plan, images, and image-exports",
  )
  check(always_condition?(gate["if"]), "runtime-image-gate must run after dependency failure")
  gate_steps = required_steps(gate, "runtime-image-gate")

  initialize = gate_steps.first
  check(
    initialize.is_a?(Hash) && initialize["name"] == "Initialize closed release-image evidence",
    "runtime-image-gate must initialize closed fallback evidence in its first step",
  )
  check(always_condition?(initialize["if"]), "closed fallback initialization must always run")
  initialize_run = initialize["run"].to_s
  [
    'schema: "cma-trigger-d09-v2"',
    'result: "inconclusive"',
    'failure: {code: "release_image_gate_incomplete"}',
    '"$D09_FALLBACK_PATH"',
    'cp "$D09_FALLBACK_PATH" "$D09_EVIDENCE_PATH"',
  ].each do |contract|
    check(
      initialize_run.include?(contract),
      "closed fallback initialization is missing #{contract}",
    )
  end

  dependency = required_step(gate_steps, "Require successful image publication and export")
  dependency_env = required_hash(
    dependency["env"],
    "runtime dependency check must bind dependency results",
  )
  expected_dependency_env = {
    "PLAN_RESULT" => '${{ needs.plan.result }}',
    "IMAGES_RESULT" => '${{ needs.images.result }}',
    "EXPORTS_RESULT" => "${{ needs['image-exports'].result }}",
  }
  check(
    expected_dependency_env.all? { |name, value| dependency_env[name] == value },
    "runtime dependency check must consume plan, images, and image-exports results",
  )
  dependency_run = dependency["run"].to_s
  %w[plan:$PLAN_RESULT images:$IMAGES_RESULT image-exports:$EXPORTS_RESULT].each do |result|
    check(
      dependency_run.include?(result),
      "runtime dependency check must fail closed for #{result.split(':').first}",
    )
  end

  resolve = required_step(gate_steps, "Resolve the exact runtime OCI artifact")
  resolve_run = resolve["run"].to_s
  [
    'revision=${GITHUB_SHA:0:7}',
    'base="cma-runtime-${VERSION}-${revision}"',
    'artifact_name=${base}-oci',
    'metadata_path=$RUNNER_TEMP/runtime-export/${base}.json',
    'archive_path=$RUNNER_TEMP/runtime-export/${base}.oci.tar',
  ].each do |contract|
    check(resolve_run.include?(contract), "runtime artifact resolution is missing #{contract}")
  end

  download = required_step(gate_steps, "Download the exact runtime OCI artifact")
  download_with = required_hash(
    download["with"],
    "runtime artifact download must retain exact inputs",
  )
  check(
    download["uses"] == "actions/download-artifact@v4",
    "runtime artifact download must use actions/download-artifact@v4",
  )
  check(
    download_with["name"] == '${{ steps.runtime_artifact.outputs.artifact_name }}',
    "runtime artifact download must use the resolved exact artifact name",
  )
  check(
    download_with["path"] == '${{ runner.temp }}/runtime-export',
    "runtime artifact download must use its isolated export directory",
  )

  validation = required_step(gate_steps, "Validate the runtime OCI metadata and archive")
  validation_run = validation["run"].to_s
  schema_match = validation_run.match(/keys == \[(.*?)\]\)/m)
  schema_keys = schema_match ? schema_match[1].scan(/"([^"]+)"/).flatten : []
  check(schema_keys == METADATA_KEYS, "runtime metadata must enforce the exact 14-field schema")
  [
    ['.schemaVersion == 1', "runtime metadata must bind schema version 1"],
    ['.component == "runtime"', "runtime metadata must bind the runtime component"],
    ['.version == $version', "runtime metadata must bind the release version"],
    ['.revision == $revision', "runtime metadata must bind the short revision"],
    ['.fullRevision == $full_revision', "runtime metadata must bind the full revision"],
    ['.sourceRepository == $repository', "runtime metadata must bind the lowercase runtime repository"],
    ['.versionReference == ($repository + ":" + $version)', "runtime metadata must bind the version reference"],
    ['.revisionReference == ($repository + ":" + $revision)', "runtime metadata must bind the revision reference"],
    ['.sourceDigest | test("^sha256:[0-9a-f]{64}$")', "runtime metadata must validate the source digest"],
    ['.sourceReference == ($repository + "@" + .sourceDigest)', "runtime metadata must bind the immutable source reference"],
    ['.archive == $archive', "runtime metadata must bind the exact archive filename"],
    ['.archiveReference == $version', "runtime metadata must bind the archive version"],
    ['.archiveSha256 | test("^sha256:[0-9a-f]{64}$")', "runtime metadata must validate the archive digest"],
    ['.platforms == ["linux/amd64"]', "runtime metadata must permit only linux/amd64"],
  ].each do |contract, message|
    check(validation_run.include?(contract), message)
  end
  check(
    validation_run.include?("tr '[:upper:]' '[:lower:]'") &&
      validation_run.include?('expected_repository="ghcr.io/${owner}/cma-runtime"'),
    "runtime metadata must derive a lowercase GHCR runtime repository",
  )
  check(
    validation_run.include?('find "$export_dir" -mindepth 1 -maxdepth 1') &&
      validation_run.include?('[ "$entry_count" -ne 2 ]'),
    "runtime artifact must contain exactly two entries",
  )
  check(
    validation_run.include?('sha256sum "$ARCHIVE_PATH"') &&
      validation_run.include?('[ "$actual_archive_sha" != "$expected_archive_sha" ]'),
    "runtime archive checksum must be verified against metadata",
  )

  pull = required_step(gate_steps, "Pull the immutable runtime image")
  check(
    pull["run"].to_s.include?('docker pull "$SOURCE_REFERENCE"'),
    "D-09 must pull the validated immutable sourceReference",
  )

  campaign = required_step(gate_steps, "Prove the release runtime network boundary")
  campaign_run = campaign["run"].to_s
  [
    ['--image "$SOURCE_REFERENCE"', "release D-09 must use the immutable sourceReference"],
    ['--build-commit "$GITHUB_SHA"', "release D-09 must bind the full release commit"],
    ["--expected-architecture amd64", "release D-09 must require amd64"],
    ['--output "$D09_EVIDENCE_PATH"', "release D-09 must write the retained evidence path"],
  ].each do |argument, message|
    check(campaign_run.include?(argument), message)
  end

  close = required_step(gate_steps, "Close release-image evidence")
  check(always_condition?(close["if"]), "D-09 evidence closure must always run")
  close_run = close["run"].to_s
  artifact_schema_match = close_run.match(/\.artifacts \| keys\) == \[(.*?)\]\)/m)
  artifact_keys = artifact_schema_match ? artifact_schema_match[1].scan(/"([^"]+)"/).flatten : []
  check(artifact_keys == ARTIFACT_KEYS, "passing D-09 evidence must bind the exact artifact-facts schema")
  [
    ['.schema == "cma-trigger-d09-v2"', "closed D-09 evidence must bind its schema"],
    ['.evidenceClass == "D"', "closed D-09 evidence must bind evidence class D"],
    ['.buildCommit == $build_commit', "closed D-09 evidence must bind the release commit"],
    ['.image.reference == $source_reference', "passing D-09 evidence must bind the immutable image"],
    ['.image.architecture == "amd64"', "passing D-09 evidence must bind amd64"],
    ['.image.revision == $build_commit', "passing D-09 evidence must bind image revision"],
    ['(.artifacts | type) == "object"', "passing D-09 evidence must retain artifact identity"],
    ['(.artifacts.codexVersion | type) == "string"', "passing D-09 evidence must bind the Codex version"],
    ['(.artifacts.codexVersion | length) > 0', "passing D-09 evidence must retain a non-empty Codex version"],
    ['(.artifacts.acpVersion | type) == "string"', "passing D-09 evidence must bind the ACP version"],
    ['(.artifacts.acpVersion | length) > 0', "passing D-09 evidence must retain a non-empty ACP version"],
    ['.artifacts.acpHash | test("^[0-9a-f]{64}$")', "passing D-09 evidence must bind the ACP hash"],
    ['.artifacts.codexLauncherHash | test("^[0-9a-f]{64}$")', "passing D-09 evidence must bind the Codex launcher hash"],
    ['.artifacts.codexNativeHash | test("^[0-9a-f]{64}$")', "passing D-09 evidence must bind the Codex native hash"],
    ['.artifacts.runtimeHash | test("^[0-9a-f]{64}$")', "passing D-09 evidence must bind the runtime hash"],
    ['.artifacts.uid == 10001', "passing D-09 evidence must bind the runtime uid"],
    ['.artifacts.platform == "linux"', "passing D-09 evidence must bind Linux"],
    ['.artifacts.architecture == "x64"', "passing D-09 evidence must bind the runtime architecture"],
    ['.cleanup.status == "verified"', "passing D-09 evidence must prove cleanup"],
    ['.cleanup.failureCodes == []', "passing D-09 evidence must have no cleanup failures"],
    ['(has("failure") | not)', "passing D-09 evidence must not retain a failure"],
    ['.failure.code = "evidence_record_invalid"', "malformed D-09 evidence must close inconclusive"],
    ['"$D09_FALLBACK_PATH" > "$D09_EVIDENCE_PATH"', "malformed D-09 evidence must replace output with the closed fallback"],
  ].each do |contract, message|
    check(close_run.include?(contract), message)
  end

  upload = required_step(gate_steps, "Retain release runtime boundary evidence")
  check(always_condition?(upload["if"]), "D-09 evidence upload must always run")
  upload_with = required_hash(upload["with"], "D-09 evidence upload must retain artifact inputs")
  check(
    upload_with["name"] == 'cma-trigger-d09-release-${{ github.run_id }}-${{ github.run_attempt }}',
    "D-09 evidence artifact must bind the workflow run and attempt",
  )
  check(
    upload_with["path"] == '${{ runner.temp }}/cma-trigger-d09-release.json',
    "D-09 upload must retain the closed evidence path",
  )
  check(upload_with["retention-days"] == 90, "D-09 evidence must be retained for 90 days")
  check(
    upload_with["if-no-files-found"] == "error",
    "a missing D-09 evidence record must fail closed",
  )

  require_pass = required_step(gate_steps, "Require passing release runtime evidence")
  check(always_condition?(require_pass["if"]), "the D-09 pass assertion must always run")
  check(
    require_pass["run"].to_s.include?('.result == "pass"'),
    "GitHub Release creation must require a passing D-09 result",
  )
  close_index = gate_steps.index(close)
  upload_index = gate_steps.index(upload)
  pass_index = gate_steps.index(require_pass)
  check(
    close_index < upload_index && upload_index < pass_index,
    "D-09 steps must close evidence, upload it, then require pass",
  )

  release = required_hash(jobs["release"], "publish.yml must retain the release job")
  check(
    Array(release["needs"]) == %w[plan images image-exports runtime-image-gate],
    "release must depend directly on image-exports and runtime-image-gate",
  )
  release_steps = required_steps(release, "release")
  preflight = required_step(release_steps, "Verify the current run's image artifacts")
  preflight_run = preflight["run"].to_s
  [
    ["for component in api web runtime; do", "release artifact preflight must cover api, web, and runtime"],
    ['expected="cma-${component}-${VERSION}-${revision}-oci"', "release preflight must resolve exact OCI artifact names"],
    ['[ "$matches" -ne 1 ]', "release preflight must require exactly one artifact per component"],
    ['.expired == false', "release preflight must reject expired artifacts"],
    ['.size_in_bytes | type == "number" and . > 0', "release preflight must reject empty artifacts"],
  ].each do |contract, message|
    check(preflight_run.include?(contract), message)
  end

  release_prose = release_doc.gsub(/\s+/, " ")
  check(
    release_prose.include?("cannot prevent or retract those image tags"),
    "Release documentation must state that D-09 cannot prevent or retract GHCR image tags",
  )
  check(
    release_prose.include?("requires a pass before GitHub Release creation"),
    "Release documentation must state that D-09 gates GitHub Release creation",
  )

  readiness_prose = readiness_doc.gsub(/\s+/, " ")
  check(
    readiness_prose.include?("GitHub Release creation must depend on that run"),
    "release-readiness must name the exact GitHub Release gate",
  )
  check(
    readiness_prose.include?("D-09 does not gate that initial GHCR tag publication"),
    "release-readiness must not claim D-09 gates initial GHCR publication",
  )
end

def load_inputs
  workflow = YAML.safe_load(File.read(WORKFLOW_PATH), aliases: true)
  [workflow, File.read(RELEASE_DOC_PATH), File.read(READINESS_DOC_PATH)]
rescue Psych::Exception => error
  raise ContractError, "publish.yml must remain valid YAML: #{error.message}"
end

def cloned(value)
  Marshal.load(Marshal.dump(value))
end

def mutation_step(workflow, job, name)
  named_step(workflow.fetch("jobs").fetch(job).fetch("steps"), name).tap do |step|
    raise "self-test fixture is missing #{job}/#{name}" unless step
  end
end

def replace_once!(source, before, after)
  changed = source.sub!(before, after)
  raise "self-test fixture is missing #{before.inspect}" unless changed
end

def run_self_tests(workflow, release_doc, readiness_doc)
  tests = [
    [
      "collapsed OCI export matrix",
      "image-exports matrix must cover exactly api, web, runtime",
      lambda do |candidate, _release, _readiness|
        candidate.fetch("jobs").fetch("image-exports").fetch("strategy").fetch("matrix")["component"] = ["runtime"]
      end,
    ],
    [
      "extended OCI transfer retention",
      "OCI transfer retention must remain one day",
      lambda do |candidate, _release, _readiness|
        mutation_step(candidate, "image-exports", "Upload the OCI transfer artifact").fetch("with")["retention-days"] = 90
      end,
    ],
    [
      "fallback moved away from the first step",
      "runtime-image-gate must initialize closed fallback evidence in its first step",
      lambda do |candidate, _release, _readiness|
        steps = candidate.fetch("jobs").fetch("runtime-image-gate").fetch("steps")
        steps.insert(1, steps.shift)
      end,
    ],
    [
      "full revision validation removed",
      "runtime metadata must bind the full revision",
      lambda do |candidate, _release, _readiness|
        run = mutation_step(candidate, "runtime-image-gate", "Validate the runtime OCI metadata and archive").fetch("run")
        replace_once!(run, ".fullRevision == $full_revision", ".fullRevision != null")
      end,
    ],
    [
      "D-09 changed to a floating image",
      "release D-09 must use the immutable sourceReference",
      lambda do |candidate, _release, _readiness|
        run = mutation_step(candidate, "runtime-image-gate", "Prove the release runtime network boundary").fetch("run")
        replace_once!(run, '--image "$SOURCE_REFERENCE"', '--image "ghcr.io/example/cma-runtime:latest"')
      end,
    ],
    [
      "runtime gate removed from release dependencies",
      "release must depend directly on image-exports and runtime-image-gate",
      lambda do |candidate, _release, _readiness|
        candidate.fetch("jobs").fetch("release")["needs"] = %w[plan images image-exports]
      end,
    ],
    [
      "pass assertion moved before evidence upload",
      "D-09 steps must close evidence, upload it, then require pass",
      lambda do |candidate, _release, _readiness|
        steps = candidate.fetch("jobs").fetch("runtime-image-gate").fetch("steps")
        pass_index = steps.index { |step| step["name"] == "Require passing release runtime evidence" }
        upload_index = steps.index { |step| step["name"] == "Retain release runtime boundary evidence" }
        pass_step = steps.delete_at(pass_index)
        steps.insert(upload_index, pass_step)
      end,
    ],
    [
      "honest GHCR limitation removed from release documentation",
      "Release documentation must state that D-09 cannot prevent or retract GHCR image tags",
      lambda do |_candidate, release, _readiness|
        replace_once!(release, "cannot prevent or retract those image tags", "prevents those image tags")
      end,
    ],
  ]

  tests.each do |name, expected_error, mutation|
    candidate = cloned(workflow)
    candidate_release = release_doc.dup
    candidate_readiness = readiness_doc.dup
    mutation.call(candidate, candidate_release, candidate_readiness)
    begin
      verify_contract(candidate, candidate_release, candidate_readiness)
    rescue ContractError => error
      unless error.message.include?(expected_error)
        raise ContractError,
              "self-test #{name.inspect} failed for the wrong reason: #{error.message}"
      end
      next
    end
    raise ContractError, "self-test #{name.inspect} was not rejected"
  end

  puts "CMA Trigger publish contract self-tests passed (#{tests.length} mutations)"
end

unless ARGV.empty? || ARGV == ["--self-test"]
  warn "usage: #{File.basename($PROGRAM_NAME)} [--self-test]"
  exit 2
end

workflow, release_doc, readiness_doc = load_inputs
verify_contract(workflow, release_doc, readiness_doc)
if ARGV == ["--self-test"]
  run_self_tests(workflow, release_doc, readiness_doc)
else
  puts "CMA Trigger publish contract passed"
end

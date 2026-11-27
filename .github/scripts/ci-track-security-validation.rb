# Repository trust and release-policy validation for the workflow tests.
require 'json'
require 'open3'

def require_repo_secret_shards(documents)
  jobs = documents.fetch('ci-repo').fetch('jobs')
  secrets, shards, gate = jobs.values_at('secrets', 'scanner-tests', 'result')
  require_check([secrets, shards, gate].all?, 'Repo requires secret policy, scanner shards and result jobs')
  require_check([secrets, shards, gate].none? { |job| job.key?('continue-on-error') }, 'Repo secret checks must propagate failure')
  require_check(!secrets.key?('if') && !secrets.key?('needs') &&
    shards['needs'] == ['secrets'] && !shards.key?('if'), 'scanner shards must wait for successful secret policy')
  require_check(shards['strategy'] == {'fail-fast' => false, 'matrix' => {'shard' => [0, 1]}},
    'both scanner shards must run independently')
  require_check(gate['if'] == 'always()' && gate['needs'] == %w[structure secrets scanner-tests],
    'Repo result must always check every child')

  checkout = {
    'uses' => 'actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09',
    'with' => {'ref' => '${{ github.sha }}'}
  }
  history_checkout = checkout.merge('with' => checkout['with'].merge('fetch-depth' => 0))
  require_check(secrets['steps'].first == history_checkout &&
    shards['steps'].first == checkout && gate['steps'].first == checkout,
    'secret policy, shards and gate must check out the same candidate SHA with full history for the history scan')
  history, policy = secrets['steps'].drop(1)
  require_check(secrets['steps'].size == 3 && history['name'] == 'gitleaks' &&
    !history.key?('if') && !history.key?('continue-on-error') &&
    history['run'].lines.last(2).map(&:strip) == [
      'bash .github/scripts/scan-secret-history-tests.sh "$RUNNER_TEMP/gitleaks"',
      'bash .github/scripts/scan-secret-history.sh "$RUNNER_TEMP/gitleaks"'
    ] && policy == {
      'name' => 'Verify frozen image scanner and reject repository bypasses',
      'run' => <<~BASH
        python3 -B .github/scripts/image-scanner-policy-tests.py
        python3 -B .github/scripts/image_scanner_policy.py
      BASH
    }, 'history scan and frozen policy must succeed before either shard runs')

  outputs = {
    'shard_0' => '${{ steps.tests.outputs.shard_0 }}',
    'shard_1' => '${{ steps.tests.outputs.shard_1 }}'
  }
  install = {
    'name' => 'Install pinned image secret scanner', 'timeout-minutes' => 5,
    'run' => 'bash .github/scripts/install-image-gitleaks.sh'
  }
  run_shard = {
    'id' => 'tests', 'name' => 'Run frozen image scanner test shard',
    'working-directory' => '.github/scripts/image-secret-scanner', 'shell' => 'bash',
    'env' => {'SHARD' => '${{ matrix.shard }}'},
    'run' => <<~'BASH'
      umask 077
      python3 -B ../ci_tracks/repo_secrets.py --shard "$SHARD"
      printf 'shard_%s=%s\n' "$SHARD" "$GITHUB_SHA" >> "$GITHUB_OUTPUT"
    BASH
  }
  require_check(shards['outputs'] == outputs && shards['steps'] == [checkout, install, run_shard],
    'each scanner shard must publish its own candidate SHA only after successful testing')
  validate_results = {
    'name' => 'Validate repository job results',
    'env' => {'NEEDS' => '${{ toJSON(needs) }}'},
    'run' => 'python3 -B .github/scripts/ci_tracks/repo_secrets.py --check-gate'
  }
  require_check(gate['steps'] == [checkout, validate_results], 'Repo result must validate all child outcomes and shard SHAs')
  caller = documents.fetch('ci').fetch('jobs')
  require_check(caller.fetch('repo')['uses'] == './.github/workflows/ci-repo.yml' &&
    !caller['repo'].key?('continue-on-error') &&
    caller.fetch('gate-repo')['steps'].last['run'] == 'python3 -B .github/scripts/ci_tracks/gate.py repo',
    'CI / Repo must require the reusable Repo workflow result')
end

def check_repo_secret_gate(root)
  sha = 'f' * 40
  success = {'structure' => {'result' => 'success'}, 'secrets' => {'result' => 'success'},
    'scanner-tests' => {'result' => 'success', 'outputs' => {'shard_0' => sha, 'shard_1' => sha}}}
  command = ['python3', '-B', File.join(root, '.github/scripts/ci_tracks/repo_secrets.py'), '--check-gate']
  accepts = lambda do |needs|
    _, _, status = Open3.capture3({'NEEDS' => JSON.generate(needs), 'GITHUB_SHA' => sha}, *command)
    status.success?
  end
  require_check(accepts.call(success), 'Repo gate refused the complete successful run')
  success.each_key do |job|
    %w[failure cancelled skipped missing].each do |outcome|
      needs = Marshal.load(Marshal.dump(success))
      outcome == 'missing' ? needs.delete(job) : needs[job]['result'] = outcome
      require_check(!accepts.call(needs), "Repo gate accepted #{job} with outcome #{outcome}")
    end
  end
  %w[shard_0 shard_1].each do |shard|
    [nil, '', 'a' * 40].each do |receipt|
      needs = Marshal.load(Marshal.dump(success))
      outputs = needs['scanner-tests']['outputs']
      receipt.nil? ? outputs.delete(shard) : outputs[shard] = receipt
      require_check(!accepts.call(needs), "Repo gate accepted an absent or mismatched SHA for #{shard}")
    end
  end
end

def require_runtime_inventory(documents)
  [['ci-images', 'images', 'runtime', 'cma-runtime:ci', "matrix.component == 'runtime'"],
   ['ci-omarchy', 'omarchy', 'runtime-omarchy', 'cma-runtime-omarchy:ci', nil]].each do |file, job, component, image, condition|
    steps = documents.fetch(file).fetch('jobs').fetch(job).fetch('steps')
    gates = steps.select { |step| step.fetch('run', '').include?('runtime_image_hygiene.py --image') }
    require_check(gates.length == 1, 'one mandatory runtime layer inventory required')
    gate = gates.first
    require_check(gate['if'] == condition && !gate['continue-on-error'], 'runtime layer inventory is optional')
    report = component == 'runtime' ? 'runtime-layer-hygiene.json' : 'omarchy-evidence/layer-hygiene.json'
    command = "python3 -B .github/scripts/runtime_image_hygiene.py --image #{image} --component #{component} --output \"$RUNNER_TEMP/#{report}\""
    require_check(gate['run'] == command && gate['timeout-minutes'] == 20, 'runtime layer inventory command differs')
    export = steps.index { |step| step.fetch('run', '').include?('export-ci-native-image.py') }
    require_check(export && steps.index(gate) < export, 'runtime image exported before inventory')
  end
end

IMAGE_SCAN_IF = "success() && steps.native-image-export.outcome == 'success' && (github.event_name != 'push' || github.ref != 'refs/heads/develop')"
IMAGE_RETAIN_IF = "success() && steps.native-image-export.outcome == 'success' && (steps.native-image-scan.outcome == 'success' || (github.event_name == 'push' && github.ref == 'refs/heads/develop'))"
OMARCHY_SCAN_IF = "success() && (github.event_name == 'merge_group' || " \
  "(github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository) || " \
  "(github.event_name == 'push' && startsWith(github.ref, 'refs/heads/tl/release/')) || " \
  "(inputs.export_verification_image && github.ref == 'refs/heads/develop' && " \
  "(github.event_name == 'schedule' || github.event_name == 'workflow_dispatch')))"

# Staging CD refuses an image whose secret scan leaves a blocking finding. CI
# runs the same gate on its exported image before retaining it, so a PR or
# merge group fails first instead of the release after the merge.
def require_image_secret_scan(documents)
  steps = documents.fetch('ci-images').fetch('jobs').fetch('images').fetch('steps')
  install = steps.index { |step| step['name'] == 'Install pinned image secret scanner' }
  scan = steps.index { |step| step['name'] == 'Scan tested image as staging publication does' }
  export = steps.index { |step| step.fetch('run', '').include?('export-ci-native-image.py') }
  keep = steps.index { |step| step['name'] == 'Retain tested native image' }
  require_check(install && scan && export && keep && export < install && install < scan && scan < keep,
                'ci-images must scan the exported image before retaining it')
  [steps[install], steps[scan]].each do |step|
    require_check(step['if'] == IMAGE_SCAN_IF && !step.key?('continue-on-error'), 'ci-images image secret scan is optional')
  end
  require_check(steps[install]['run'] == "bash .github/scripts/staging_cd_transport.sh\nbash .github/scripts/install-image-gitleaks.sh\n" &&
                steps[install]['timeout-minutes'] == 5, 'ci-images image scanner install differs')
  command = 'python3 -B .github/scripts/ci_image_secret_scan.py "$IMAGE_COMPONENT" "$RUNNER_TEMP/ci-native-image" "$GITHUB_SHA"'
  require_check(steps[scan]['run'] == command && steps[scan]['timeout-minutes'] == 30 &&
    steps[scan]['id'] == 'native-image-scan', 'ci-images image secret scan command differs')
  require_check(steps[keep]['if'].to_s.split.join(' ') == IMAGE_RETAIN_IF, 'ci-images verification archives require a successful scan')
  require_omarchy_image_secret_scan(documents.fetch('ci-omarchy').fetch('jobs').fetch('omarchy').fetch('steps'))
end

def require_omarchy_image_secret_scan(steps)
  input = steps.find { |step| step['id'] == 'omarchy-scan-input' }
  install = steps.find { |step| step['id'] == 'omarchy-scanner-install' }
  scan = steps.find { |step| step['id'] == 'omarchy-scan-start' }
  export = steps.find { |step| step['name'] == 'Export tested Omarchy image' }
  retain = steps.find { |step| step['name'] == 'Retain tested Omarchy image' }
  order = [steps.find { |step| step['name'] == 'Verify every Omarchy image layer' }, input, install, scan,
           steps.find { |step| step['name'] == 'Verify Omarchy runtime contract' },
           steps.find { |step| step['name'] == 'Omarchy vulnerability gate' }, export, retain]
  require_check(order.all? && order.map { |step| steps.index(step) }.each_cons(2).all? { |a, b| a < b },
                'Omarchy must join its scanner after the foreground image checks and before retention')
  require_check(input['if'].to_s.split.join(' ') == OMARCHY_SCAN_IF && input['timeout-minutes'] == 11 &&
                input['run'] == <<~'SH', 'Omarchy must prepare a private scan input on scan-eligible runs')
    if timeout 10m python3 .github/scripts/export-ci-native-image.py runtime-omarchy "$GITHUB_SHA" "$RUNNER_TEMP/omarchy-ci-scan-input"; then
      echo "ready=true" >> "$GITHUB_OUTPUT"
    fi
  SH
  require_check(install['if'] == "success() && steps.omarchy-scan-input.outputs.ready == 'true'" &&
                install['timeout-minutes'] == 6 && install['env'] == {'CD_COMPONENT' => 'runtime-omarchy'} &&
                install['run'] == <<~'SH', 'Omarchy image scanner install differs')
    if timeout 5m bash -ec 'bash .github/scripts/staging_cd_transport.sh && bash .github/scripts/install-image-gitleaks.sh'; then
      echo "ready=true" >> "$GITHUB_OUTPUT"
    fi
  SH
  require_check(scan['if'] == "success() && steps.omarchy-scanner-install.outputs.ready == 'true'" &&
                scan['timeout-minutes'] == 1 && scan['run'] == <<~'SH', 'Omarchy must attest scanner startup')
    if python3 -B .github/scripts/omarchy-ci-scan.py start "$GITHUB_SHA"; then
      echo "ready=true" >> "$GITHUB_OUTPUT"
    fi
  SH
  require_check([input, install, scan, export, retain].none? { |step| step.key?('continue-on-error') },
                'Omarchy scan start and retention must reject optional workflow failure')
  require_check(export['env'] == {
    'OMARCHY_SCAN_STARTED' => '${{ steps.omarchy-scan-start.outputs.ready }}',
  } && export['timeout-minutes'] == 31 && export['run'] == <<~'SH', 'Omarchy could export before image scan completion')
    if [[ "$GITHUB_EVENT_NAME" = push && "$GITHUB_REF" != refs/heads/tl/release/* ]]; then
      timeout 10m python3 .github/scripts/export-ci-native-image.py runtime-omarchy "$GITHUB_SHA" "$RUNNER_TEMP/ci-native-image"
    else
      test "$OMARCHY_SCAN_STARTED" = true && python3 -B .github/scripts/omarchy-ci-scan.py promote "$GITHUB_SHA"
    fi
  SH
  require_check(retain['if'] == "success() && steps.native-image-export.outcome == 'success'",
    'Omarchy retention must require promotion success')
end

def image_scan_workflow_mutations
  [
    ->(d) { d['ci-images']['jobs']['images']['steps'].reject! { |s| s['name'] == 'Scan tested image as staging publication does' } },
    ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Scan tested image as staging publication does' }['continue-on-error'] = true },
    ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Scan tested image as staging publication does' }['if'] = "github.event_name == 'merge_group'" },
    ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Scan tested image as staging publication does' }['run'] += ' || true' },
    ->(d) { steps = d['ci-images']['jobs']['images']['steps']; scan = steps.index { |s| s['name'] == 'Scan tested image as staging publication does' }; steps.insert(steps.index { |s| s['name'] == 'Retain tested native image' } + 1, steps.delete_at(scan)) },
    ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].reject! { |s| s['name'] == 'Install pinned image secret scanner' } },
    ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Scan tested Omarchy image as staging publication does' }['timeout-minutes'] = 120 },
    ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['id'] == 'omarchy-scan-input' }['if'] = 'success()' },
    ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Export tested Omarchy image' }['run'].sub!('test "$OMARCHY_SCAN_STARTED" = true', 'true') },
    ->(d) { d['ci-omarchy']['jobs']['omarchy']['steps'].find { |s| s['name'] == 'Export tested Omarchy image' }['run'].sub!('if [[ "$GITHUB_EVENT_NAME" = push', 'if [[ "$GITHUB_EVENT_NAME" = schedule || "$GITHUB_EVENT_NAME" = push') },
    ->(d) { d['ci-images']['jobs']['images']['steps'].find { |s| s['name'] == 'Retain tested native image' }['if'] = "success() && steps.native-image-export.outcome == 'success'" },
  ]
end

def check_verification_archive_scan_paths(documents)
  images = documents.fetch('ci-images').fetch('jobs').fetch('images').fetch('steps')
  scan = images.find { |step| step['id'] == 'native-image-scan' }
  omarchy = documents.fetch('ci-omarchy').fetch('jobs').fetch('omarchy').fetch('steps')
  promote = omarchy.find { |step| step['id'] == 'native-image-export' }
  scanner = <<~'SH'
    python3() { printf '%s\n' "$@"; return "$TEST_SCAN_EXIT"; }
    timeout() { return 97; }
  SH
  revision = 'a' * 40
  %w[schedule workflow_dispatch].each do |event|
    env = {'GITHUB_EVENT_NAME' => event, 'GITHUB_REF' => 'refs/heads/develop',
           'GITHUB_SHA' => revision, 'RUNNER_TEMP' => '/synthetic', 'OMARCHY_SCAN_STARTED' => 'true'}
    [['api', scan, %W[-B .github/scripts/ci_image_secret_scan.py api /synthetic/ci-native-image #{revision}]],
     ['runtime', scan, %W[-B .github/scripts/ci_image_secret_scan.py runtime /synthetic/ci-native-image #{revision}]],
     ['runtime-omarchy', promote, %W[-B .github/scripts/omarchy-ci-scan.py promote #{revision}]]].each do |component, step, args|
      [0, 42].each do |exit_code|
        out, _, status = Open3.capture3(env.merge('IMAGE_COMPONENT' => component, 'TEST_SCAN_EXIT' => exit_code.to_s),
          'bash', '-ec', scanner + step.fetch('run'))
        require_check(out.lines.map(&:chomp) == args && status.exitstatus == exit_code,
          "#{event} #{component} must use the exact archive and propagate scan status #{exit_code}")
      end
    end
    out, _, status = Open3.capture3(env.merge('OMARCHY_SCAN_STARTED' => '', 'TEST_SCAN_EXIT' => '0'),
      'bash', '-ec', scanner + promote.fetch('run'))
    require_check(out.empty? && !status.success?, "#{event} Omarchy must refuse an absent scan")
  end
end

RUNTIME_VULNERABILITY_BASE = "${{ github.event_name == 'pull_request' && github.event.pull_request.base.sha || github.event_name == 'merge_group' && github.event.merge_group.base_sha || github.event.before }}"

# Pinned runtime inputs keep building the same packages while the advisory
# database changes. Their trivy scans report everything, and the gate fails
# exactly the findings a change owns or that stopped being fresh. Every other
# image keeps the blanket exit-code gate.
def require_runtime_vulnerability_gate(documents)
  images = documents.fetch('ci-images').fetch('jobs').fetch('images').fetch('steps')
  blanket = images.find { |s| s['name'] == 'Vulnerability scan' }
  require_check(blanket && blanket['if'] == "matrix.component != 'runtime'" && blanket.dig('with', 'exit-code') == '1',
                'non-runtime images must keep the blocking vulnerability scan')
  [['ci-images', 'images', 'Runtime vulnerability scan', 'Runtime vulnerability gate', 'runtime',
    '"$RUNNER_TEMP/runtime-vulnerabilities.json"', "matrix.component == 'runtime'"],
   ['ci-omarchy', 'omarchy', 'Scan tested Omarchy image', 'Omarchy vulnerability gate', 'runtime-omarchy',
    '"$RUNNER_TEMP/omarchy-evidence/vulnerabilities.json"', nil]].each do |file, job, scan_name, gate_name, component, report, condition|
    steps = documents.fetch(file).fetch('jobs').fetch(job).fetch('steps')
    scan = steps.index { |s| s['name'] == scan_name }
    gate = steps.index { |s| s['name'] == gate_name }
    require_check(scan && gate && gate == scan + 1, "#{file} must gate its runtime vulnerability report right after the scan")
    require_check(steps[scan].dig('with', 'exit-code') == '0' && steps[scan].dig('with', 'format') == 'json' &&
                  steps[scan].dig('with', 'severity') == 'HIGH,CRITICAL', "#{file} runtime scan must report JSON for the gate")
    command = "python3 -B .github/scripts/image_vulnerability_gate.py --component #{component} --report #{report} --base \"$BASE_SHA\""
    require_check(steps[gate]['run'] == command && !steps[gate].key?('continue-on-error') && steps[gate]['if'] == condition &&
                  steps[gate].dig('env', 'BASE_SHA') == RUNTIME_VULNERABILITY_BASE, "#{file} runtime vulnerability gate differs")
  end
end

def check_version_label(workflow, condition = '${{ !cancelled() }}')
  version = workflow.fetch('jobs').fetch('version-label')
  require_check(version.fetch('name') == 'CI / Version label' && version.fetch('if') == condition, 'version gate missing')
  require_check(version.fetch('needs') == ['changes'], 'version must use selector')
  require_check(!version.key?('continue-on-error'), 'version job must propagate failure')
  label_check = version.fetch('steps').last
  require_check(label_check['run'] == 'python3 -B .github/scripts/ci_tracks/version_label.py' &&
    !label_check.key?('if') && !label_check.key?('continue-on-error'), 'version validator missing or bypassed')
  require_check(label_check.fetch('env') == {
    'DEPLOYABLES' => '${{ needs.changes.outputs.deployables }}',
    'SELECTOR_RESULT' => '${{ needs.changes.result }}', 'GH_TOKEN' => '${{ github.token }}'
  }, 'version selector or live label context differs')
end

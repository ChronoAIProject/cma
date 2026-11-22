module ScannerCIContract
  COMMANDS = {
    'Public-value scanner mechanics' => 'node --test .github/scripts/scanner-contract.test.mjs',
    'Checksum-verified public-value scanner contracts' => 'python3 -B .github/scripts/scanner-contract.py'
  }.freeze

  def self.check(value)
    raise 'scanner qualification must remain required in AM with the shared archive' unless value
  end

  def self.validate(documents)
    rust = documents.fetch('ci-rust').fetch('jobs')
    job = rust.fetch('timing')
    check(!job.key?('if') && !job.key?('continue-on-error') && job['runs-on'] == 'ubuntu-latest')
    check(job.dig('env', 'CMA_NEXTEST_REUSE') == '1' && job.dig('env', 'CARGO_INCREMENTAL') == '0')
    steps = job.fetch('steps')
    indexes = COMMANDS.map do |name, command|
      matches = steps.each_index.select { |i| steps[i]['name'] == name || steps[i].fetch('run', '').include?(command) }
      check(matches.length == 1)
      step = steps[matches.first]
      check(step == {'name' => name, 'run' => command, 'if' => "inputs.track == 'am'",
        'working-directory' => '.', 'timeout-minutes' => 5})
      matches.first
    end
    check(indexes == indexes.sort && indexes.uniq.length == 2)
    node = steps.each_index.select { |i| steps[i].fetch('uses', '').start_with?('actions/setup-node@') }
    archive = steps.each_index.select { |i| steps[i]['uses'] == './.github/actions/rust-archive' }
    check(node.length == 1 && archive.length == 1 && node.first < archive.first && archive.first < indexes.first)
    check(steps[node.first]['with'] == {'node-version' => '24.15.0'})
    [steps[node.first], steps[archive.first]].each do |step|
      check(!step.key?('if') && !step.key?('continue-on-error') && !step.key?('env'))
    end
    result = rust.fetch('result')
    check(result['if'] == 'always()' && !result.key?('continue-on-error') && result['needs'].include?('timing'))
    gate = result.fetch('steps').last
    check(gate['run'] == 'python3 .github/scripts/ci_tracks/children.py rust' &&
      gate['env'] == {'NEEDS' => '${{ toJSON(needs) }}', 'TRACK' => '${{ inputs.track }}'} &&
      !gate.key?('if') && !gate.key?('continue-on-error'))
  end

  def self.mutations
    mutations = []
    COMMANDS.each_key do |name|
      ['removed', 'duplicate', 'optional', 'disabled', 'wrong-track', 'timeout', 'cwd', 'env', 'shell', 'skip'].each do |change|
        mutations << lambda do |documents|
          steps = documents['ci-rust']['jobs']['timing']['steps']
          step = steps.find { |s| s['name'] == name }
          case change
          when 'removed' then steps.delete(step)
          when 'duplicate' then steps << step.dup
          when 'optional' then step['continue-on-error'] = true
          when 'disabled' then step['if'] = 'false'
          when 'wrong-track' then step['if'] = "inputs.track == 'eg'"
          when 'timeout' then step.delete('timeout-minutes')
          when 'cwd' then step.delete('working-directory')
          when 'env' then step['env'] = {'CMA_TEST_GITLEAKS' => '/unverified/tool'}
          when 'shell' then step['shell'] = 'bash {0}'
          when 'skip' then step['run'] += ' || true'
          end
        end
      end
    end
    ['archive', 'node', 'node-version', 'reuse', 'incremental', 'optional', 'disabled', 'order', 'result'].each do |change|
      mutations << lambda do |documents|
        rust = documents['ci-rust']['jobs']
        job = rust['timing']
        steps = job['steps']
        case change
        when 'archive' then steps.reject! { |s| s['uses'] == './.github/actions/rust-archive' }
        when 'node' then steps.reject! { |s| s.fetch('uses', '').start_with?('actions/setup-node@') }
        when 'node-version' then steps.find { |s| s.fetch('uses', '').start_with?('actions/setup-node@') }['with']['node-version'] = 'latest'
        when 'reuse' then job['env'].delete('CMA_NEXTEST_REUSE')
        when 'incremental' then job['env']['CARGO_INCREMENTAL'] = '1'
        when 'optional' then job['continue-on-error'] = true
        when 'disabled' then job['if'] = 'false'
        when 'order' then steps << steps.delete(steps.find { |s| s['uses'] == './.github/actions/rust-archive' })
        when 'result' then rust['result']['steps'].last['run'] = 'true'
        end
      end
    end
    mutations
  end
end

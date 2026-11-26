# frozen_string_literal: true

module KeyProviderCIContract
  COMMANDS = {
    'Counting-provider fixture regressions' => 'node --test .github/scripts/work-recovery/*.test.mjs .github/scripts/key-provider-contracts.test.mjs',
    'KeyIssuer counting-provider contracts' => 'python3 -B .github/scripts/key-provider-contracts.py'
  }.freeze

  def self.check(documents)
    rust = documents.fetch('ci-rust').fetch('jobs')
    job = rust.fetch('timing')
    raise 'KeyIssuer timing job must be required and bounded' unless
      !job.key?('if') && !job.key?('continue-on-error') && job['timeout-minutes'] == 35 &&
      job.dig('env', 'CMA_NEXTEST_REUSE') == '1' && job.dig('env', 'CARGO_INCREMENTAL') == '0'
    steps = job.fetch('steps')
    nodes = steps.select { |s| s.fetch('uses', '').start_with?('actions/setup-node@') }
    archives = steps.select { |s| s['uses'] == './.github/actions/rust-archive' }
    raise 'KeyIssuer requires pinned Node and archive restore' unless
      nodes.size == 1 && nodes.first.dig('with', 'node-version') == '24.15.0' && archives.size == 1 &&
      [nodes.first, archives.first].all? { |s| (s.keys & %w[if continue-on-error]).empty? }
    previous = [steps.index(nodes.first), steps.index(archives.first)].max
    all_steps = documents.values.flat_map { |doc| doc.fetch('jobs').values.flat_map { |j| j.fetch('steps', []) } }
    COMMANDS.each do |name, command|
      matches = steps.select { |s| s['name'] == name }
      raise 'KeyIssuer step must run exactly once' unless matches.size == 1
      step = matches.first
      entrypoint = name == 'Counting-provider fixture regressions' ? '.github/scripts/work-recovery/' : '.github/scripts/key-provider-contracts.py'
      raise 'KeyIssuer execution duplicated outside its required step' unless
        all_steps.select { |s| s.fetch('run', '').include?(entrypoint) } == [step]
      raise 'KeyIssuer step cannot skip or hide failure' unless step == {
        'name' => name, 'run' => command, 'if' => "inputs.track == 'am'",
        'working-directory' => '.', 'timeout-minutes' => 5
      }
      raise 'KeyIssuer prerequisites must precede execution' unless steps.index(step) > previous
      previous = steps.index(step)
    end
    raise 'KeyIssuer failure must reach the required Rust result' unless
      rust.fetch('result')['if'] == 'always()' && rust.fetch('result').fetch('needs').include?('timing') &&
      !rust.fetch('result').key?('continue-on-error')
  end

  def self.mutations
    mutations = []
    COMMANDS.each_key do |name|
      mutations << ->(d) { d['ci-rust']['jobs']['timing']['steps'].reject! { |s| s['name'] == name } }
      mutations << ->(d) { steps = d['ci-rust']['jobs']['timing']['steps']; steps << steps.find { |s| s['name'] == name }.dup }
      mutations << ->(d) { steps = d['ci-rust']['jobs']['timing']['steps']; steps << steps.find { |s| s['name'] == name }.merge('name' => 'Duplicate execution') }
      [{'if' => 'false'}, {'continue-on-error' => true}, {'timeout-minutes' => nil},
       {'working-directory' => 'api'}, {'shell' => 'bash {0} || true'}].each do |bypass|
        mutations << ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == name }.merge!(bypass) }
      end
      mutations << ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['name'] == name }['run'] += ' || true' }
      mutations << ->(d) { steps = d['ci-rust']['jobs']['timing']['steps']; step = steps.find { |s| s['name'] == name }; steps.delete(step); steps.unshift(step) }
    end
    mutations + [
      ->(d) { d['ci-rust']['jobs']['timing']['continue-on-error'] = true },
      ->(d) { d['ci-rust']['jobs']['timing']['if'] = 'false' },
      ->(d) { d['ci-rust']['jobs']['timing'].delete('timeout-minutes') },
      ->(d) { d['ci-rust']['jobs']['timing']['env'].delete('CMA_NEXTEST_REUSE') },
      ->(d) { d['ci-rust']['jobs']['timing']['env'].delete('CARGO_INCREMENTAL') },
      ->(d) { d['ci-rust']['jobs']['timing']['steps'].reject! { |s| s['uses'] == './.github/actions/rust-archive' } },
      ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s['uses'] == './.github/actions/rust-archive' }['if'] = 'false' },
      ->(d) { d['ci-rust']['jobs']['timing']['steps'].find { |s| s.fetch('uses', '').start_with?('actions/setup-node@') }['with']['node-version'] = 'latest' },
      ->(d) { d['ci-rust']['jobs']['result']['needs'].delete('timing') },
      ->(d) { d['ci-rust']['jobs']['result']['continue-on-error'] = true }
    ]
  end
end

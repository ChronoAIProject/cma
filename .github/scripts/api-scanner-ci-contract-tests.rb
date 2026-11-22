# frozen_string_literal: true
require 'yaml'

module APIScannerContract
  NAME = 'Qualify the scanner from the exact API image'
  EXPECTED = {
    'name' => NAME, 'if' => "matrix.component == 'api'", 'timeout-minutes' => 3,
    'env' => {'CMA_SCANNER_IMAGE_ID' => '${{ steps.native-image.outputs.imageid }}'},
    'run' => 'python3 -B .github/scripts/api_scanner_qualification.py',
  }.freeze

  def self.check(document)
    steps = document.fetch('jobs').fetch('images').fetch('steps')
    found = steps.select { |step| step['name'] == NAME }
    raise 'exact mandatory API scanner qualification required' unless found == [EXPECTED]
    native = steps.select { |step| step['id'] == 'native-image' }
    raise 'exact native API image build required' unless native.size == 1 &&
      native.first['name'] == 'Load native image for scanning' &&
      native.first.fetch('uses', '').start_with?('docker/build-push-action@') &&
      native.first.dig('with', 'load') == true && native.first.dig('with', 'platforms') == 'linux/amd64' &&
      !native.first.key?('if') && !native.first.key?('continue-on-error') &&
      native.first.dig('with', 'build-args').lines.map(&:strip).include?('CHRONO_BUILD_SHA=${{ github.sha }}')
    export = steps.index { |step| step['name'] == 'Export tested native image' }
    raise 'qualify the loaded image before exporting it' unless export &&
      steps.index(native.first) < steps.index(found.first) && steps.index(found.first) < export
    mechanics = steps.select { |step| step['name'] == 'Shared image build contracts' }
    raise 'API scanner mechanics must run' unless mechanics.size == 1 &&
      !mechanics.first.key?('if') && !mechanics.first.key?('continue-on-error') &&
      mechanics.first.fetch('run').lines.map(&:strip).include?('python3 -B .github/scripts/api_scanner_qualification_tests.py')
  end
end

document = YAML.load_file(File.expand_path('../workflows/ci-images.yml', __dir__))
APIScannerContract.check(document)
mutations = [
  ->(s) { s.reject! { |v| v['name'] == APIScannerContract::NAME } },
  ->(s) { s << s.find { |v| v['name'] == APIScannerContract::NAME }.dup },
  ->(s) { s.find { |v| v['name'] == APIScannerContract::NAME }['if'] = 'false' },
  ->(s) { s.find { |v| v['name'] == APIScannerContract::NAME }['continue-on-error'] = true },
  ->(s) { s.find { |v| v['name'] == APIScannerContract::NAME }['env']['CMA_SCANNER_IMAGE_ID'] = 'synthetic:ci' },
  ->(s) { s.find { |v| v['name'] == APIScannerContract::NAME }['run'] = 'true' },
  ->(s) { s.find { |v| v['id'] == 'native-image' }['id'] = 'wrong-image' },
  ->(s) { s.find { |v| v['id'] == 'native-image' }['with']['platforms'] = 'linux/arm64' },
  ->(s) { s.find { |v| v['id'] == 'native-image' }['with']['build-args'].sub!('CHRONO_BUILD_SHA', 'UNRELATED_SOURCE') },
  ->(s) { s.unshift(s.delete(s.find { |v| v['name'] == APIScannerContract::NAME })) },
  ->(s) { s.find { |v| v['name'] == 'Shared image build contracts' }['run'] = 'true' },
]
mutations.each_with_index do |mutate, index|
  copy = Marshal.load(Marshal.dump(document))
  mutate.call(copy.fetch('jobs').fetch('images').fetch('steps'))
  begin
    APIScannerContract.check(copy)
  rescue StandardError
    next
  end
  raise "API scanner workflow mutation #{index} was accepted"
end
puts "API scanner workflow contract passed: #{mutations.size} bypass mutations rejected"

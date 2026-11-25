require 'minitest/autorun'
require 'digest'
require 'fileutils'
require 'json'
require 'open3'
require 'tmpdir'
require 'timeout'
require 'yaml'

class GatewayLatencyCheckTest < Minitest::Test
  ROOT = File.expand_path('../..', __dir__)
  REPORTS = %w[cmaeg-latency.txt cmaeg-latency-console.txt].freeze
  SELECTOR = 'package(=cmaeg) and test(~latency_budget)'

  def setup
    @root = Dir.mktmpdir('eg-latency-check-')
    @api = File.join(@root, 'api')
    @scripts = File.join(@root, '.github/scripts')
    FileUtils.mkdir_p([File.join(@api, 'target'), @scripts])
    runner = File.join(ROOT, '.github/scripts/eg-latency-check.py')
    FileUtils.cp(runner, @scripts) if File.file?(runner)
    File.write(File.join(@scripts, 'backend-nextest.sh'), <<~'SH')
      #!/usr/bin/env bash
      exec python3 - "$@" <<'PY'
      import json, os, signal, sys
      from pathlib import Path
      calls = Path('calls.json')
      rows = json.loads(calls.read_text()) if calls.exists() else []
      rows.append(sys.argv[1:])
      calls.write_text(json.dumps(rows))
      if os.environ.get('TEST_SIGNAL') == '1':
          Path('ready').write_text(str(os.getpid()))
          while True:
              signal.pause()
      reports = ['cmaeg-latency.txt', 'cmaeg-latency-console.txt']
      if os.environ.get('TEST_MISSING') == '1':
          reports.pop()
      for report in reports:
          (Path('target') / report).write_text('fresh ' + report)
      statuses = [int(value) for value in os.environ['TEST_STATUSES'].split(',')]
      raise SystemExit(statuses[min(len(rows) - 1, len(statuses) - 1)])
      PY
    SH
    git('init', '-q')
    git('add', '.')
    git('-c', 'user.name=Test Fixture', '-c', 'user.email=fixture@example.test',
        '-c', 'core.hooksPath=/dev/null', 'commit', '-qm', 'Synthetic latency runner fixture')
    @head = git('rev-parse', 'HEAD').strip
    @env = {'BASH_ENV' => nil, 'TEST_STATUSES' => '0', 'TEST_MISSING' => '0', 'TEST_SIGNAL' => '0',
            'GITHUB_RUN_ID' => '41', 'GITHUB_RUN_ATTEMPT' => '2'}
    workflow = YAML.load_file(File.join(ROOT, '.github/workflows/ci-rust.yml'))
    @command = workflow.fetch('jobs').fetch('timing').fetch('steps').find do |step|
      step.fetch('name', '').start_with?('Enforce') && step['if'] == "inputs.track == 'eg'"
    end.fetch('run')
  end

  def teardown
    FileUtils.remove_entry(@root)
  end

  def git(*args)
    output, status = Open3.capture2e('git', '-C', @root, *args)
    raise output unless status.success?
    output
  end

  def run_check(values = {})
    stdout, stderr, status = Open3.capture3(@env.merge(values), 'bash', '-c', @command, chdir: @api)
    [status.exitstatus, stdout + stderr]
  end

  def calls
    JSON.parse(File.read(File.join(@api, 'calls.json')))
  end

  def result
    JSON.parse(File.read(File.join(@api, 'target/cmaeg-latency-results/attempt-1/result.json')))
  end

  def assert_one_invocation
    assert_equal [['run', '--run-ignored', 'only', '-E', SELECTOR, '--success-output', 'final']], calls
  end

  def test_first_failure_cannot_be_replaced_by_a_later_success
    code, output = run_check('TEST_STATUSES' => '7,0')
    assert_equal 7, code, output
    assert_one_invocation
    assert_equal 7, result.fetch('test_exit_code')
    assert_equal 7, result.fetch('gate_exit_code')
  end

  def test_success_retains_both_reports_and_exact_attempt_identity
    code, output = run_check
    assert_equal 0, code, output
    assert_one_invocation
    data = result
    assert_equal @head, data.fetch('checkout')
    assert_equal '41', data.fetch('workflow_run_id')
    assert_equal '2', data.fetch('workflow_attempt')
    assert_equal 1, data.fetch('invocation')
    assert_equal SELECTOR, data.fetch('selection')
    assert_equal 0, data.fetch('test_exit_code')
    assert_equal 0, data.fetch('gate_exit_code')
    REPORTS.each do |name|
      entry = data.fetch('reports').fetch(name)
      assert_equal 'present', entry.fetch('status')
      path = File.join(@api, 'target/cmaeg-latency-results/attempt-1', name)
      assert_equal "fresh #{name}", File.read(path)
      assert_equal Digest::SHA256.file(path).hexdigest, entry.fetch('sha256')
    end
  end

  def test_repeated_failures_do_not_add_a_second_invocation
    code, output = run_check('TEST_STATUSES' => '9,9')
    assert_equal 9, code, output
    assert_one_invocation
    assert_equal 9, result.fetch('test_exit_code')
  end

  def test_existing_attempt_evidence_is_not_overwritten
    assert_equal 7, run_check('TEST_STATUSES' => '7').first
    original = result
    refute_equal 0, run_check.first
    assert_one_invocation
    assert_equal original, result
  end

  def test_missing_report_is_explicit_and_stale_files_cannot_supply_it
    REPORTS.each { |name| File.write(File.join(@api, 'target', name), 'stale previous invocation') }
    code, output = run_check('TEST_MISSING' => '1')
    assert_equal 1, code, output
    assert_one_invocation
    data = result
    assert_equal 0, data.fetch('test_exit_code')
    assert_equal 1, data.fetch('gate_exit_code')
    assert_equal 'missing', data.fetch('reports').fetch(REPORTS.last).fetch('status')
    refute File.exist?(File.join(@api, 'target/cmaeg-latency-results/attempt-1', REPORTS.last))
  end

  def test_term_terminates_the_invocation_and_retains_its_failure
    check_signal('TERM', 15)
  end

  def test_int_terminates_the_invocation_and_retains_its_failure
    check_signal('INT', 2)
  end

  def check_signal(name, number)
    pid = Process.spawn(@env.merge('TEST_SIGNAL' => '1'), 'bash', '-c', "exec #{@command.strip}",
                        chdir: @api, out: File::NULL, err: File::NULL)
    ready = File.join(@api, 'ready')
    Timeout.timeout(10) { sleep 0.01 until File.exist?(ready) }
    child = Integer(File.read(ready))
    Process.kill(name, pid)
    _, status = Timeout.timeout(10) { Process.wait2(pid) }
    pid = nil
    assert_equal 128 + number, status.exitstatus
    assert_one_invocation
    data = result
    assert_equal 128 + number, data.fetch('gate_exit_code')
    assert_equal number, data.fetch('signal')
    assert_raises(Errno::ESRCH) { Process.kill(0, child) }
  ensure
    if pid
      Process.kill('KILL', pid) rescue Errno::ESRCH
      Process.wait(pid) rescue Errno::ECHILD
    end
  end
end

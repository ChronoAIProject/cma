#!/usr/bin/env python3
"""Isolated CD timing: standard library only, no workflow/deployment mutations."""
import argparse
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path
import re
import signal
import statistics
import subprocess
import sys
import time

STAGES = {'source', 'reuse', 'cache-restore', 'cache-export', 'build', 'inspect',
          'scan', 'runtime-smoke', 'registry-check', 'login', 'push', 'verify'}
CATEGORIES = {'rust-compile', 'dependency-compile', 'go-compile', 'media-compile',
              'cache-import', 'cache-export', 'archive-export', 'other-build'}
PHASES = STAGES | {'plan', 'allocate', 'finalize', 'admit', 'deploy', 'publication', 'overhead'}
ANSI = re.compile(r'\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))')


def digest(data):
    return hashlib.sha256(data).hexdigest()


def number(value):
    return type(value) in (int, float) and 0 <= value <= sys.float_info.max


def parse_events(lines):
    """Allowlist numeric event fields; never retain commands, paths or payloads."""
    rows, builds, caches = [], [], []
    for line in lines:
        match = re.search(r'(\{.*\})\s*$', ANSI.sub('', line))
        if not match:
            continue
        try:
            event = json.loads(match[1])
        except ValueError:
            continue
        if not isinstance(event, dict):
            continue
        if (event.get('event') == 'publication-stage' and isinstance(event.get('stage'), str) and event['stage'] in STAGES
                and number(event.get('seconds')) and event.get('result') in ('success', 'failure')):
            rows.append({k: event[k] for k in ('stage', 'seconds', 'result')})
        elif event.get('event') == 'publication-build':
            categories = event.get('category_seconds', {})
            if not isinstance(categories, dict):
                continue
            builds.append({'complete': event.get('complete') is True,
                           'category_seconds': {k: v for k, v in categories.items()
                                                if k in CATEGORIES and number(v)}})
        elif (event.get('event') == 'publication-cache'
              and event.get('operation') in ('restore', 'export')
              and event.get('result') in ('hit', 'miss', 'accepted', 'withheld')):
            caches.append({k: event[k] for k in ('operation', 'result')})
    totals = {stage: round(sum(r['seconds'] for r in rows if r['stage'] == stage), 6)
              for stage in sorted({r['stage'] for r in rows})}
    return {'events': rows, 'stage_seconds': totals, 'build_categories': builds,
            'cache_events': caches, 'all_emitted_stages_successful': bool(rows) and
            all(r['result'] == 'success' for r in rows),
            'note': 'Absent phases are unknown, not zero. Repeated rows are task sums. '
                    'Archive/category times are diagnostic subsets and may overlap; never add them to build wall time.'}


def seconds(start, end):
    if not isinstance(start, str) or not isinstance(end, str):
        return None
    try:
        started = dt.datetime.fromisoformat(start.replace('Z', '+00:00'))
        completed = dt.datetime.fromisoformat(end.replace('Z', '+00:00'))
    except ValueError:
        return None
    if started.tzinfo is None or completed.tzinfo is None:
        return None
    value = (completed - started).total_seconds()
    return value if value >= 0 else None


def identity(args):
    return {k: getattr(args, k) for k in
            ('revision', 'workload', 'contract', 'runner', 'cache', 'component', 'phase', 'task_count')}


def local(args):
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        raise ValueError('A command argv is required after --')
    rows = []
    for _ in range(args.repeats):
        start = time.monotonic()
        process = subprocess.Popen(command, cwd=args.cwd, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL, start_new_session=True)
        timed_out = False
        try:
            code = process.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                pass
            # Kill descendants even if their parent exited after TERM.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            code = process.wait()
        rows.append({'seconds': round(time.monotonic() - start, 6),
                     'exit_code': code, 'timed_out': timed_out})
        if code or timed_out:
            break
    successful = len(rows) == args.repeats and all(r['exit_code'] == 0 and not r['timed_out'] for r in rows)
    values = [r['seconds'] for r in rows]
    return {'mode': 'local', 'identity': identity(args), 'samples': rows, 'successful': successful,
            'metric_seconds': statistics.median(values) if successful else None,
            'range_seconds': [min(values), max(values)], 'requested_repeats': args.repeats,
            'command_sha256': digest(json.dumps(command, separators=(',', ':')).encode()),
            'output_retained': False, 'metric': 'median command wall seconds'}


def gh_json(endpoint):
    response = subprocess.run(['gh', 'api', endpoint], capture_output=True, timeout=55)
    if response.returncode:
        raise ValueError('GitHub GET failed; raw diagnostic suppressed')
    return json.loads(response.stdout)


def ci(args):
    if args.step is not None and args.timer_stage is not None:
        raise ValueError('Publication timers are job-scoped; --step and --timer-stage cannot be combined')
    auth = subprocess.run(['gh', 'auth', 'status', '--active'], capture_output=True, timeout=15)
    if auth.returncode or b'account chronoai-shining ' not in auth.stdout + auth.stderr:
        raise ValueError('Active GitHub account is not chronoai-shining; no switch attempted')
    job = gh_json(f'repos/{args.repo}/actions/jobs/{args.job}')
    if job.get('head_sha') != args.revision:
        raise ValueError('Requested revision differs from job head_sha')
    if args.runner == 'github-free' and ('self-hosted' in job.get('labels', []) or
            not set(job.get('labels', [])) & {'ubuntu-24.04', 'ubuntu-22.04', 'ubuntu-latest'}):
        raise ValueError('Job does not carry an expected standard GitHub Ubuntu label')
    record = job
    if args.step:
        matches = [s for s in job.get('steps', []) if s['name'] == args.step]
        if len(matches) != 1:
            raise ValueError('Requested step must match exactly one step')
        record = matches[0]
    duration = seconds(record.get('started_at'), record.get('completed_at'))
    successful = (job.get('status') == 'completed' and record.get('status') == 'completed'
                  and job.get('conclusion') == 'success' and record.get('conclusion') == 'success'
                  and duration is not None)
    result = {'mode': 'ci', 'identity': identity(args), 'job_id': args.job, 'run_id': job['run_id'],
              'job_state': job.get('conclusion') or job['status'],
              'record_state': record.get('conclusion') or record['status'],
              'started_at': record.get('started_at'), 'completed_at': record.get('completed_at'),
              'observed_seconds': None if record.get('conclusion') == 'skipped' else duration,
              'metric_seconds': duration if successful else None, 'successful': successful,
              'metric': 'job wall seconds' if not args.step else 'step wall seconds',
              'record_name_sha256': digest(record['name'].encode()), 'output_retained': False}
    if args.logs:
        if job.get('status') != 'completed':
            raise ValueError('Phase log extraction requires a completed job; no full workflow wait is needed')
        response = subprocess.run(['gh', 'api', '--allow-escape-sequences',
                                   f'repos/{args.repo}/actions/jobs/{args.job}/logs'],
                                  capture_output=True, timeout=55)
        if response.returncode:
            raise ValueError('Job log GET failed; raw diagnostic suppressed')
        result['phases'] = parse_events(response.stdout.decode(errors='replace').splitlines())
        result['log_sha256'] = digest(response.stdout)
        if args.timer_stage:
            events = [r for r in result['phases']['events'] if r['stage'] == args.timer_stage]
            complete = len(events) == args.task_count and all(r['result'] == 'success' for r in events)
            result['successful'] = successful and complete
            result['metric_seconds'] = sum(r['seconds'] for r in events) if result['successful'] else None
            result['metric'] = 'publication-stage:' + args.timer_stage
    return result


def phases(args):
    # Input is an owner-held log; this command saves only the closed projection.
    raw = sys.stdin.buffer.read() if args.input == '-' else Path(args.input).read_bytes()
    return {'mode': 'phases', 'identity': identity(args), 'input_sha256': digest(raw),
            'phases': parse_events(raw.decode(errors='replace').splitlines()),
            'metric_seconds': None, 'successful': False, 'output_retained': False,
            'note': 'Event projection alone does not establish whole-job success or completeness.'}


def compare(args):
    before_bytes = Path(args.before).read_bytes()
    after_bytes = Path(args.after).read_bytes()
    before = json.loads(before_bytes)
    after = json.loads(after_bytes)
    differences = [k for k in ('workload', 'contract', 'runner', 'cache', 'component', 'phase', 'task_count')
                   if before['identity'][k] != after['identity'][k]]
    if differences or before['mode'] != after['mode'] or before.get('metric') != after.get('metric'):
        raise ValueError('Not comparable: workload/contract/runner/cache/task/phase/method differs')
    if before['identity']['cache'] == 'unknown':
        raise ValueError('Cache condition must be explicit for a before/after claim')
    if not before['successful'] or not after['successful']:
        raise ValueError('Failed, partial or skipped runs cannot establish a speedup')
    if before['mode'] == 'local':
        if before['command_sha256'] != after['command_sha256']:
            raise ValueError('Local command argv differs; use the same relative command in each checkout')
        if min(len(before['samples']), len(after['samples'])) < 3:
            raise ValueError('Use at least three local samples per revision')
    else:
        if before.get('record_name_sha256') != after.get('record_name_sha256'):
            raise ValueError('CI job/step names differ; compare the same stable step')
    a, b = before['metric_seconds'], after['metric_seconds']
    if not number(a) or not number(b):
        raise ValueError('Both measurements must be finite nonnegative seconds')
    return {'mode': 'compare', 'before_revision': before['identity']['revision'],
            'after_revision': after['identity']['revision'], 'identity': after['identity'],
            'before_seconds': a, 'after_seconds': b, 'saving_seconds': round(a - b, 6),
            'saving_percent': round((a - b) / a * 100, 3) if a else None,
            'source_sha256': [digest(before_bytes), digest(after_bytes)],
            'note': 'Isolated phase comparison only. CI single samples are descriptive, not a distribution. '
                    'Does not certify safety equivalence, cold-cache behavior or merge-to-live.'}


def hex_value(length):
    def check(value):
        if not re.fullmatch('[0-9a-f]{' + str(length) + '}', value):
            raise argparse.ArgumentTypeError('Expected lowercase hexadecimal digest')
        return value
    return check


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='mode', required=True)
    for mode in ('local', 'ci', 'phases', 'compare'):
        p = commands.add_parser(mode); p.add_argument('--out', required=True)
        if mode == 'compare':
            p.add_argument('--before', required=True); p.add_argument('--after', required=True)
            continue
        p.add_argument('--revision', required=True, type=hex_value(40))
        p.add_argument('--workload', required=True, type=hex_value(64))
        p.add_argument('--contract', required=True, type=hex_value(64))
        p.add_argument('--runner', required=True, choices=('local', 'github-free'))
        p.add_argument('--cache', required=True, choices=('cold', 'warm', 'unknown'))
        p.add_argument('--component', required=True, choices=('am', 'eg', 'web', 'shared'))
        p.add_argument('--phase', required=True, choices=sorted(PHASES))
        p.add_argument('--task-count', type=int, default=1)
        if mode == 'local':
            p.add_argument('--cwd', required=True); p.add_argument('--repeats', type=int, default=3)
            p.add_argument('--timeout', type=float, default=300)
            p.add_argument('command', nargs=argparse.REMAINDER)
        elif mode == 'ci':
            p.add_argument('--repo', default='ChronoAIProject/cma', choices=('ChronoAIProject/cma',))
            p.add_argument('--job', required=True, type=int); p.add_argument('--step')
            p.add_argument('--logs', action='store_true')
            p.add_argument('--timer-stage', choices=sorted(STAGES))
        else:
            p.add_argument('--input', default='-')
    args = parser.parse_args(argv)
    try:
        if Path(args.out).exists():
            raise FileExistsError('Evidence output already exists')
        if args.mode == 'ci' and args.timer_stage and not args.logs:
            raise ValueError('A timer-stage metric requires --logs')
        if args.mode != 'compare' and args.task_count < 1:
            raise ValueError('Task count must be positive')
        if args.mode == 'local' and (not 1 <= args.repeats <= 20 or not math.isfinite(args.timeout) or not 0 < args.timeout <= 3600):
            raise ValueError('Require 1–20 repeats and a finite timeout of at most 3600s')
        output = {'local': local, 'ci': ci, 'phases': phases, 'compare': compare}[args.mode](args)
        output['captured_at'] = dt.datetime.now(dt.timezone.utc).isoformat()
        output['schema_version'] = 1
        path = Path(args.out)
        # Never overwrite a baseline or an existing evidence file.
        with path.open('x', encoding='utf-8') as stream:
            json.dump(output, stream, indent=2); stream.write('\n')
        print(json.dumps({'mode': args.mode, 'evidence_sha256': digest(path.read_bytes()),
                          'successful': output.get('successful'), 'metric_seconds': output.get('metric_seconds')}))
        return 0 if args.mode in {'phases', 'compare'} or output['successful'] else 2
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        # Exception text can contain paths or provider data; retain only class.
        print(json.dumps({'error': type(error).__name__, 'measurement_accepted': False}), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())

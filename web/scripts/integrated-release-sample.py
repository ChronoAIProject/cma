import json
import pathlib
import subprocess
import sys
import time

root = pathlib.Path(sys.argv[1])
identifier = sys.argv[2]
state = sys.argv[3]
assert root.name.startswith('attempt-') and identifier.startswith('cma-ui-ticket19-')
cgroup = pathlib.Path('/sys/fs/cgroup') / identifier
runner = '/Users/liuyitian/code/cma-ui-worktrees/runtime-proof/runsc-probe/runsc'
command = [runner, '--root=' + state, '--platform=systrap', '--network=none', '--file-access-mounts=shared', '--ignore-cgroups=false', '--log=' + str(root / 'runsc.log'), 'run', '--bundle=' + str(root), identifier]
samples = []
limits = {}
started = time.monotonic()
process = subprocess.Popen(command)
while process.poll() is None:
    if time.monotonic() - started > 480:
        subprocess.run([runner, '--root=' + state, 'kill', '--all', identifier, 'KILL'], check=False)
        process.kill()
        break
    try:
        cpu = dict(line.split() for line in (cgroup / 'cpu.stat').read_text().splitlines())
        phase = 'startup'
        if (root / 'relay/creation.json').exists():
            phase = 'preparation'
        if (root / 'relay/native-progress.json').exists():
            phase = 'task_and_followup'
        samples.append({'phase': phase, 'elapsed_seconds': time.monotonic() - started, 'wall_seconds': time.time(), 'usage_usec': int(cpu['usage_usec']), 'memory_current_bytes': int((cgroup / 'memory.current').read_text()), 'memory_peak_bytes': int((cgroup / 'memory.peak').read_text()), 'pids_current': int((cgroup / 'pids.current').read_text())})
        if not limits:
            limits = {name: (cgroup / name).read_text().strip() for name in ['cpu.max', 'memory.max', 'pids.max']}
    except FileNotFoundError:
        pass
    time.sleep(0.1)
code = process.wait()
(root / 'cgroup-samples.json').write_text(json.dumps(samples, indent=2))
report = {'exit_code': code, 'command': command, 'observed_limits': limits, 'samples': len(samples), 'scope': 'Only this native runtime cgroup; host API, Web and unrelated VM tasks excluded.', 'cpu_denominator': 'one host core; quota is not assumed enforced', 'elapsed_seconds': time.monotonic() - started}
if samples:
    report['peak_memory_bytes'] = max(sample['memory_peak_bytes'] for sample in samples)
    report['peak_pids'] = max(sample['pids_current'] for sample in samples)
    report['cpu_seconds'] = (samples[-1]['usage_usec'] - samples[0]['usage_usec']) / 1000000
(root / 'cgroup-budget.json').write_text(json.dumps(report, indent=2))
raise SystemExit(code)

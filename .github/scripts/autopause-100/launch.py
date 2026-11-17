#!/usr/bin/env python3
"""Own the finite local campaign and all of its subprocesses/port forwards."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time

from fixture import PauseFixture
from common import utc, write


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--ready', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--cycles', type=int, default=100)
    args = parser.parse_args()
    if not 1 <= args.cycles <= 100:
        parser.error('cycles must be in 1..100; only 100 qualifies the hard gate')
    os.umask(0o077)
    args.output.mkdir(parents=True, exist_ok=False)
    ready = json.loads(args.ready.read_text())
    if not ready['context'].startswith('kind-cma-autopause18-') or not ready['server'].startswith('https://127.0.0.1:'):
        raise ValueError('owned_local_Kind_required')
    fixture = PauseFixture(ready, args.output)
    result = {'start': utc(), 'verdict': 'RUNNING', 'operator_pod_exec': False,
              'secret_resource_access': False}
    child = None
    def interrupt(signum, frame):
        raise TimeoutError('campaign_signal_' + str(signum))
    for sig in [signal.SIGINT, signal.SIGTERM, signal.SIGALRM]:
        signal.signal(sig, interrupt)
    signal.alarm(14400)
    try:
        fixture.initialize()
        control = fixture.serve()
        config = {**ready, 'ams': fixture.origins, 'bearer': fixture.secret['bearer'],
            'admin_bearer': fixture.secret['admin_bearer'], 'host': 'api.localhost.direct',
            'public_origin': 'https://api.localhost.direct', 'control_origin': control,
            'control_token': fixture.control_token, 'output_dir': str(args.output), 'cycles': args.cycles}
        path = args.output / 'config.private.json'
        write(path, config)
        with (args.output / 'campaign.log').open('w') as log:
            child = subprocess.Popen(['node', str(Path(__file__).with_name('run.mjs')), str(path)],
                stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            result['campaign_pid'] = child.pid
            write(args.output / 'owner.json', {**result, 'owner_pid': os.getpid()})
            result['exit_code'] = child.wait(timeout=13500)
        result['verdict'] = 'PASS' if result['exit_code'] == 0 else 'FAIL'
    except Exception as error:
        result.update(verdict='FAIL', error=type(error).__name__ + ':' + str(error))
    finally:
        if child and child.poll() is None:
            fixture.client.stop(child)
        fixture.client.deadline = time.monotonic() + 180
        fixture.capture()
        if fixture.server:
            fixture.server.shutdown()
            fixture.server.server_close()
            fixture.thread.join(timeout=10)
        result['children_joined'] = fixture.client.close()
        result['end'] = utc()
        write(args.output / 'fixture-result.json', result)
        (args.output / 'owner.json').unlink(missing_ok=True)
        signal.alarm(0)
    print(json.dumps(result))
    return 0 if result['verdict'] == 'PASS' else 1


if __name__ == '__main__':
    raise SystemExit(main())

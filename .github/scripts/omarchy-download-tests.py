#!/usr/bin/env python3
"""Execute the production patch fetch/checksum against deterministic HTTP faults."""
import hashlib
import http.server
from pathlib import Path
import re
import subprocess
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / 'api/crates/layer4-runtime/src/omarchy/build-graphics.sh'
URL = ('https://api.github.com/repos/hyprwm/aquamarine/commits/'
       '7bb8bdf4e8fedaf4dfae58512bc4c728671727df')
SHA = '542a3075643971d69862e7481446b9ec8f368338d1dcbc13ce26438a0e7e23cf'
BODY = b'fixture immutable patch\n'


def production_fetch():
    source = SCRIPT.read_text()
    match = re.search(r'(curl [^\n]*--connect-timeout[^\n]*(?:\\\n[^\n]*)*\n'
                      r'echo "' + SHA + r'[^\n]+\| sha256sum -c -)', source)
    if not match:
        raise AssertionError('bounded patch fetch and checksum must remain adjacent')
    command = match.group(1)
    for argument in ['--connect-timeout 15', '--max-time 120', '--retry 3',
                     '--retry-max-time 180', 'Accept: application/vnd.github.patch',
                     'X-GitHub-Api-Version: 2022-11-28', URL, 'curl -fsSL']:
        if argument not in command:
            raise AssertionError('missing pinned download contract: ' + argument)
    return command


class DownloadTests(unittest.TestCase):
    def run_download(self, statuses, body=BODY):
        calls = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                calls.append((self.path, self.headers['Accept'],
                              self.headers['X-GitHub-Api-Version']))
                status = statuses[min(len(calls) - 1, len(statuses) - 1)]
                self.send_response(status)
                self.send_header('Content-Length', str(len(body)))
                self.send_header('Retry-After', '0')
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        with http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                with tempfile.TemporaryDirectory(prefix='cma-omarchy-download-') as directory:
                    # Keep the real flags and verification command. Only the origin and
                    # expected synthetic payload hash differ from the production input.
                    command = production_fetch().replace(
                        URL, f'http://127.0.0.1:{server.server_port}/pinned-commit')
                    command = command.replace(SHA, hashlib.sha256(BODY).hexdigest())
                    result = subprocess.run(
                        ['bash', '-euo', 'pipefail', '-c',
                         'build_dir="$1"\n' + command, 'download-test', directory],
                        capture_output=True, text=True, timeout=15)
            finally:
                server.shutdown()
                thread.join(timeout=2)
        self.assertTrue(all(path == '/pinned-commit' and
                            accept == 'application/vnd.github.patch' and
                            version == '2022-11-28'
                            for path, accept, version in calls))
        return result, len(calls)

    def test_exact_verified_success(self):
        result, calls = self.run_download([200])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, 1)
        self.assertIn('aquamarine.patch: OK', result.stdout)

    def test_transient_throttling_and_service_failure_retry(self):
        for status in [429, 503]:
            with self.subTest(status=status):
                result, calls = self.run_download([status, 200])
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(calls, 2)

    def test_persistent_throttling_exhausts_three_retries(self):
        result, calls = self.run_download([429])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, 4)
        self.assertNotIn(': OK', result.stdout)

    def test_missing_artifact_fails_without_retry(self):
        result, calls = self.run_download([404])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, 1)

    def test_success_status_cannot_bypass_checksum(self):
        result, calls = self.run_download([200], b'tampered fixture patch\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, 1)
        self.assertIn('FAILED', result.stdout)


if __name__ == '__main__':
    unittest.main()

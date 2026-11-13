#!/usr/bin/env python3
"""Offline control tests: synthetic kubectl only; no network or real kubeconfig."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

BUNDLE = Path(__file__).resolve().parent.parent
MOCK_KUBECTL = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with Path(os.environ['CMA_REVIEW_CALLS']).open('a') as f:
    f.write(json.dumps(args) + '\n')
state = json.loads(Path(os.environ['CMA_REVIEW_STATE']).read_text())
if args[4:6] == ['config', 'view']:
    print(state.get('server', 'https://127.0.0.1:16443'), end='')
else:
    print('{}')
'''


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='.control-tests-', dir=BUNDLE)
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in ['scripts', 'private', 'bin', 'evidence']:
            (self.root / name).mkdir()
        for name in ['k', 'mode.sh']:
            shutil.copyfile(BUNDLE / 'scripts' / name, self.root / 'scripts' / name)
            (self.root / 'scripts' / name).chmod(0o700)
        (self.root / 'private/kubeconfig').write_text('synthetic fixture, not a kubeconfig\n')
        (self.root / 'bin/kubectl').write_text(MOCK_KUBECTL)
        (self.root / 'bin/kubectl').chmod(0o700)
        self.calls_path = self.root / 'calls.jsonl'
        self.state_path = self.root / 'state.json'
        self.save_state({})
        self.env = dict(os.environ)
        for key in ['BASH_ENV', 'ENV', 'KUBECONFIG']:
            self.env.pop(key, None)
        self.env.update(PATH=str(self.root / 'bin') + os.pathsep + os.environ['PATH'],
                        CMA_REVIEW_CALLS=str(self.calls_path),
                        CMA_REVIEW_STATE=str(self.state_path))

    def save_state(self, value):
        self.state_path.write_text(json.dumps(value))

    def calls(self):
        return [json.loads(line) for line in self.calls_path.read_text().splitlines()] if self.calls_path.exists() else []

    def run_control(self, name, *args):
        return subprocess.run([str(self.root / 'scripts' / name), *args], cwd=self.root,
                              env=self.env, text=True, capture_output=True, timeout=30)


class WrapperTests(Fixture):
    def test_destination_and_identity_overrides_rejected_before_kubectl(self):
        forms = [
            ['-nreview-other'], ['-shttps://review.invalid:443'], ['-A=true'],
            ['-n', 'review-other'], ['-n=review-other'], ['-s', 'https://review.invalid'],
            ['-s=https://review.invalid'], ['-A'], ['-A=false'], ['-wnreview-other'],
            ['-wA'], ['-vnreview-other'], ['-onreview-other'],
            ['--namespace', 'review-other'], ['--namespace=review-other'],
            ['--server', 'https://review.invalid'], ['--server=https://review.invalid'],
            ['--all-namespaces'], ['--all-namespaces=true'], ['--all-namespaces=false'],
            ['--context', 'review-other'], ['--context=review-other'],
            ['--kubeconfig', 'review-file'], ['--kubeconfig=review-file'],
            ['--cluster=review-other'], ['--user=review-other'], ['--token=synthetic'],
            ['--client-certificate=review-file'], ['--client-key=review-file'],
            ['--password=synthetic'], ['--certificate-authority=review-file'],
            ['--insecure-skip-tls-verify'], ['--tls-server-name=review.invalid'],
            ['--proxy-url=https://review.invalid'], ['--as=review-subject'],
            ['--as-group=review-group'], ['--as-uid=review-subject'],
            ['--raw=https://review.invalid'], ['--', '-nreview-other'],
        ]
        for form in forms:
            with self.subTest(form=form):
                result = self.run_control('k', 'get', 'pods', *form)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(self.calls(), [])

    def test_unsupported_commands_rejected_before_kubectl(self):
        for args in [('exec', 'review-pod', '--', 'sh'), ('attach', 'review-pod'),
                     ('cp', 'review-file', 'review-pod:/tmp'), ('config', 'use-context', 'review')]:
            with self.subTest(args=args):
                self.assertEqual(self.run_control('k', *args).returncode, 2)
                self.assertEqual(self.calls(), [])

    def test_nonlocal_server_stops_after_precheck(self):
        self.save_state({'server': 'https://review.invalid:443'})
        self.assertEqual(self.run_control('k', 'get', 'pods').returncode, 2)
        self.assertEqual(len(self.calls()), 1)
        self.assertEqual(self.calls()[0][4:6], ['config', 'view'])

    def test_supported_output_and_selector_forms_keep_pinned_destination(self):
        for args in [('get', 'pods', '-o', 'json'),
                     ('get', 'pods', '-o=json', '-l=app=review'),
                     ('get', 'pods', '-l', 'app=review', '-o', 'json')]:
            with self.subTest(args=args):
                result = self.run_control('k', *args)
                self.assertEqual(result.returncode, 0, result.stderr)
                expected = ['--kubeconfig', str(self.root / 'private/kubeconfig'),
                            '--context', 'kind-cma-scale', '--namespace', 'cma-scale', *args]
                self.assertEqual(self.calls()[-1], expected)


if __name__ == '__main__':
    unittest.main()

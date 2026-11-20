"""Output isolation covers native libraries, children and buffered streams."""
import contextlib
import inspect
import io
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from staging_cd_runtime import mask_tree, run_private


class PrivateRuntimeTests(unittest.TestCase):

    def test_masking_escapes_annotation_injection(self):
        lines = []
        mask_tree({'fixture-name': ['value%\r\n::warning::hidden', 'value', 'value', '']}, lines.append)
        self.assertEqual(len(lines), 3)
        self.assertTrue(all((line.startswith('::add-mask::') and '\n' not in line and ('\r' not in line) for line in lines)))
        self.assertIn('::add-mask::value%25%0D%0A::warning::hidden', lines)

    def test_public_output_and_summary_are_only_mask_commands_and_named_pass_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'summary.md'
            for succeeds in (True, False):
                output = io.StringIO()

                def operation(masker, succeeds=succeeds):
                    masker({'fixture-name': 'private-fixture'})
                    print('private response must never reach logs')
                    if not succeeds:
                        raise ValueError('private exception must never reach logs')
                    return True
                with contextlib.redirect_stdout(output):
                    result = run_private(operation, path, subject='fixture-step', explain=False)
                status = 'PASS' if succeeds else 'FAIL'
                self.assertEqual(result, 0 if succeeds else 1)
                self.assertEqual(path.read_text(), status + '\n')
                lines = output.getvalue().splitlines()
                self.assertEqual(lines[-1], status + ' fixture-step')
                self.assertTrue(all((line.startswith('::add-mask::') for line in lines[:-1])))
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(run_private(lambda _masker: None, path, subject='fixture-step'), 1)
                self.assertEqual(run_private(lambda _masker: True, Path(directory) / 'missing' / 'summary',
                                             subject='fixture-step'), 1)

    def test_failure_names_its_kind_and_public_line_without_private_text(self):
        from staging_cd_common import Refusal, require

        def refuse(_masker):
            require(False, 'fixture-detail refusal')

        def time_out(_masker):
            raise subprocess.TimeoutExpired(['fixture-detail-command'], 1)

        def fail_io(_masker):
            raise OSError('fixture-detail path')

        def fail(_masker):
            raise KeyError('fixture-detail key')
        here = Path(__file__).name
        lines = {name: inspect.getsourcelines(function)[1] + 1 for name, function in
                 (('refused', refuse), ('timeout', time_out), ('os-error', fail_io), ('error', fail))}
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / 'summary'
            for kind, operation in (('refused', refuse), ('timeout', time_out), ('os-error', fail_io),
                                    ('error', fail), ('declined', lambda _masker: False)):
                with self.subTest(kind=kind):
                    output = io.StringIO()
                    with patch.dict(os.environ, {'GITHUB_STEP_SUMMARY': str(summary)}), \
                            contextlib.redirect_stdout(output):
                        self.assertEqual(run_private(operation, None, subject='fixture-step'), 1)
                    line, annotation = output.getvalue().splitlines()
                    site = f' at={here}:{lines[kind]}' if kind in lines else ''
                    self.assertRegex(line, rf'\AFAIL fixture-step reason={kind}{re.escape(site)} cause=[a-z]+\Z')
                    self.assertTrue(annotation.startswith('::error '))
                    self.assertIn('Next step: ', annotation)
                    text = output.getvalue() + summary.read_text()
                    self.assertNotIn('fixture-detail', text)
            self.assertEqual(summary.read_text().count('| fixture-step |'), 5)
        self.assertTrue(issubclass(Refusal, ValueError))

    def test_only_explicit_signals_are_called_transient(self):
        import urllib.error
        from staging_cd_runtime import failure_kind

        def http(code):
            return urllib.error.HTTPError('https://fixture.invalid', code, 'fixture-detail', {}, None)
        cases = ((subprocess.TimeoutExpired(['fixture'], 1), 'timeout', 'transient'),
                 (TimeoutError(), 'timeout', 'transient'),
                 (ConnectionResetError(), 'network', 'transient'),
                 (urllib.error.URLError('fixture-detail'), 'network', 'transient'),
                 (http(503), 'http-server', 'transient'), (http(429), 'http-server', 'transient'),
                 (http(403), 'http-denied', 'operator'), (http(401), 'http-denied', 'operator'),
                 (http(404), 'http-client', 'unknown'), (http(422), 'http-client', 'unknown'),
                 (FileNotFoundError('fixture-detail'), 'os-error', 'unknown'),
                 (PermissionError(), 'os-error', 'unknown'), (KeyError('x'), 'error', 'unknown'))
        for error, kind, cause in cases:
            with self.subTest(error=type(error).__name__, kind=kind):
                self.assertEqual(failure_kind(error), kind)
                with patch.dict(os.environ, {}, clear=True), contextlib.redirect_stdout(io.StringIO()) as output:
                    run_private(lambda _masker, error=error: (_ for _ in ()).throw(error), None, subject='fixture-step')
                self.assertRegex(output.getvalue().splitlines()[0], rf'reason={kind}( at=\S+)? cause={cause}\Z')
                self.assertNotIn('fixture-detail', output.getvalue())

    def test_native_and_child_output_cannot_escape_private_boundary(self):
        program = '\nimport os\nimport subprocess\nimport sys\nfrom staging_cd_runtime import run_private\n\ndef operation(masker):\n    masker(\'fixture-mask\')\n    sys.__stdout__.write(\'private buffered stdout\')\n    sys.__stderr__.write(\'private buffered stderr\')\n    os.write(1, b\'private native stdout\\n\')\n    os.write(2, b\'private native stderr\\n\')\n    subprocess.run([sys.executable, \'-c\',\n        "import os; os.write(1, b\'private child stdout\'); os.write(2, b\'private child stderr\')"], check=True)\n    if sys.argv[2] == \'fail\':\n        raise ValueError(\'private failure\')\n    return True\n\nresult = run_private(operation, sys.argv[1], subject=\'fixture-step\', explain=False)\nprint(\'restored stdout\')\nprint(\'restored stderr\', file=sys.stderr)\nraise SystemExit(result)\n'
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / 'summary'
            for (mode, status, code) in (('pass', 'PASS', 0), ('fail', 'FAIL', 1)):
                with self.subTest(mode=mode):
                    result = subprocess.run([sys.executable, '-B', '-c', program, str(summary), mode], cwd=Path(__file__).parent, text=True, capture_output=True, check=False)
                    self.assertEqual(result.returncode, code)
                    self.assertEqual(result.stdout, f'::add-mask::fixture-mask\n{status} fixture-step\nrestored stdout\n')
                    self.assertEqual(result.stderr, 'restored stderr\n')
                    self.assertEqual(summary.read_text(), status + '\n')


if __name__ == "__main__":
    unittest.main()

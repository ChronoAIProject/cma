"""Output isolation covers native libraries, children and buffered streams."""
import contextlib
import io
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from staging_cd_runtime import mask_tree, run_private
from staging_cd_progress import PublicationProgress, PublicationStage


class PrivateRuntimeTests(unittest.TestCase):

    def test_publication_diagnostics_emit_only_enumerated_stages(self):
        for stage in PublicationStage:
            progress = PublicationProgress()
            output = io.StringIO()
            def operation(_masker):
                progress.enter(stage)
                print("synthetic private diagnostics")
                raise ValueError("synthetic private failure")
            with contextlib.redirect_stdout(output):
                result = run_private(operation, None, publication_progress=progress)
            self.assertEqual(result, 1)
            self.assertEqual(output.getvalue(), "publication-stage=" + stage.value + " result=FAIL\nFAIL\n")
        progress.current = "unsafe\n::warning::injected"
        with contextlib.redirect_stdout(output := io.StringIO()):
            run_private(lambda _: False, None, publication_progress=progress)
        self.assertEqual(output.getvalue(), "publication-stage=unknown result=FAIL\nFAIL\n")
        with self.assertRaises(ValueError):
            progress.enter("build")

    def test_masking_escapes_annotation_injection(self):
        lines = []
        mask_tree({'fixture-name': ['value%\r\n::warning::hidden', 'value', 'value', '']}, lines.append)
        self.assertEqual(len(lines), 3)
        self.assertTrue(all((line.startswith('::add-mask::') and '\n' not in line and ('\r' not in line) for line in lines)))
        self.assertIn('::add-mask::value%25%0D%0A::warning::hidden', lines)

    def test_public_output_and_summary_are_only_mask_commands_and_pass_fail(self):
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
                    result = run_private(operation, path)
                status = 'PASS' if succeeds else 'FAIL'
                self.assertEqual(result, 0 if succeeds else 1)
                self.assertEqual(path.read_text(), status + '\n')
                lines = output.getvalue().splitlines()
                self.assertEqual(lines[-1], status)
                self.assertTrue(all((line.startswith('::add-mask::') for line in lines[:-1])))
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(run_private(lambda _masker: None, path), 1)
                self.assertEqual(run_private(lambda _masker: True, Path(directory) / 'missing' / 'summary'), 1)

    def test_native_and_child_output_cannot_escape_private_boundary(self):
        program = '\nimport os\nimport subprocess\nimport sys\nfrom staging_cd_runtime import run_private\n\ndef operation(masker):\n    masker(\'fixture-mask\')\n    sys.__stdout__.write(\'private buffered stdout\')\n    sys.__stderr__.write(\'private buffered stderr\')\n    os.write(1, b\'private native stdout\\n\')\n    os.write(2, b\'private native stderr\\n\')\n    subprocess.run([sys.executable, \'-c\',\n        "import os; os.write(1, b\'private child stdout\'); os.write(2, b\'private child stderr\')"], check=True)\n    if sys.argv[2] == \'fail\':\n        raise ValueError(\'private failure\')\n    return True\n\nresult = run_private(operation, sys.argv[1])\nprint(\'restored stdout\')\nprint(\'restored stderr\', file=sys.stderr)\nraise SystemExit(result)\n'
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / 'summary'
            for (mode, status, code) in (('pass', 'PASS', 0), ('fail', 'FAIL', 1)):
                with self.subTest(mode=mode):
                    result = subprocess.run([sys.executable, '-B', '-c', program, str(summary), mode], cwd=Path(__file__).parent, text=True, capture_output=True, check=False)
                    self.assertEqual(result.returncode, code)
                    self.assertEqual(result.stdout, f'::add-mask::fixture-mask\n{status}\nrestored stdout\n')
                    self.assertEqual(result.stderr, 'restored stderr\n')
                    self.assertEqual(summary.read_text(), status + '\n')


if __name__ == "__main__":
    unittest.main()

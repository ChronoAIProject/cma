"""Reject stale navigation and missing UI acknowledgements before more input."""
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    'browser_input', Path(__file__).with_name('omarchy-browser-input.py'))
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)
STAGES = ['cma-desktop-ready', 'cma-desktop-input-ready', 'cma-desktop-fullscreen',
          'cma-input-focused', 'cma-input:cma-codex', 'cma-ui:cma-codex']


class BrowserInput(unittest.TestCase):
    def execute(self, titles, fail_command=False):
        pending = iter(titles)
        self.last = ''
        self.actions = []
        self.reads = []

        def title(command, **options):
            self.assertEqual(command, ['python3', '/opt/probes/omarchy-active-title.py'])
            self.assertEqual(options, {'text': True})
            self.last = next(pending, self.last)
            self.reads.append(self.last)
            return self.last + ' - Chromium\n'

        def action(command, **options):
            self.assertEqual(command[0], 'xdotool')
            self.assertEqual(options, {'check': True})
            self.actions.append((command[1:], self.last))
            if fail_command:
                raise subprocess.CalledProcessError(1, command)

        with patch.object(PROBE.subprocess, 'check_output', side_effect=title), \
                patch.object(PROBE.subprocess, 'run', side_effect=action), \
                patch.object(PROBE.time, 'monotonic', side_effect=range(1000)), \
                patch.object(PROBE.time, 'sleep'):
            PROBE.main('http://127.0.0.1:8877')

    def test_delayed_navigation_does_not_accept_previous_page_title(self):
        self.execute(['Untitled', STAGES[0], STAGES[0], STAGES[0], *STAGES[1:]])
        fullscreen = next(item for item in self.actions if item[0][-1] == 'F11')
        self.assertEqual(fullscreen[1], 'cma-desktop-input-ready')
        self.assertEqual(self.actions[0][1], 'cma-desktop-ready')
        self.assertEqual(self.actions[-1][1], 'cma-input:cma-codex')
        self.assertEqual(self.reads[-1], 'cma-ui:cma-codex')

    def test_navigation_to_google_fails_before_fullscreen_or_field_input(self):
        with self.assertRaisesRegex(AssertionError, 'cma-desktop-input-ready'):
            self.execute([STAGES[0], STAGES[0], 'www.google.com'])
        self.assertEqual(len(self.actions), 3)
        self.assertEqual(self.actions[-1][0][-1], 'Return')

    def test_old_ready_title_never_acknowledges_second_navigation(self):
        with self.assertRaisesRegex(AssertionError, 'cma-desktop-input-ready'):
            self.execute([STAGES[0]])
        self.assertEqual(len(self.actions), 3)

    def test_each_missing_acknowledgement_fails_without_advancing(self):
        action_counts = [0, 3, 4, 5, 6, 7]
        for stage, count in enumerate(action_counts):
            with self.subTest(stage=STAGES[stage]), \
                    self.assertRaisesRegex(AssertionError, STAGES[stage]):
                self.execute(STAGES[:stage] + ['not-ready'])
            self.assertEqual(len(self.actions), count)

    def test_title_prefix_does_not_accept_wrong_input(self):
        with self.assertRaisesRegex(AssertionError, 'cma-input:cma-codex'):
            self.execute(STAGES[:4] + ['cma-input:cma-codex-wrong'])
        self.assertEqual(len(self.actions), 6)

    def test_keyboard_failure_is_not_swallowed(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.execute(STAGES, fail_command=True)
        self.assertEqual(len(self.actions), 1)


if __name__ == '__main__':
    unittest.main()

"""Exercise native Wayland close selection and retained-profile exit checks."""
import json
import os
from pathlib import Path
import runpy
import sys
import unittest
from unittest.mock import Mock, patch

SCRIPT = Path(__file__).with_name('omarchy-close-browser.py')
WINDOW = {'class': 'chromium', 'address': '0x123'}
PROFILE = b'--user-data-dir=/home/cma/.cma/chrome'


class CloseBrowser(unittest.TestCase):
    def execute(self, args=(), windows=None, processes=(), focused=True):
        windows = iter(windows if windows is not None else [[WINDOW], []])
        self.commands = []

        def output(command, **_options):
            self.assertEqual(os.environ['XDG_RUNTIME_DIR'], '/tmp/cma-desktop-runtime')
            self.assertEqual(os.environ['HYPRLAND_INSTANCE_SIGNATURE'], 'fixture-instance')
            self.commands.append(command)
            if command == ['hyprctl', '-j', 'clients']:
                return json.dumps(next(windows, []))
            if command == ['hyprctl', '-j', 'activewindow']:
                return json.dumps(WINDOW if focused else {'address': '0x456'})
            if command == ['pgrep', '-x', 'vncviewer']:
                return '123\n'
            if command == ['xdotool', 'search', '--onlyvisible', '--pid', '123']:
                return '456\n'
            self.fail('unexpected command: ' + str(command))

        def run(command, **options):
            self.assertEqual(options, {'check': True})
            self.commands.append(command)

        with patch.object(sys, 'argv', [str(SCRIPT), *args]), \
                patch('os.environ', {'HOME': '/tmp/isolated-agent-home'}), \
                patch.object(Path, 'read_text', return_value='fixture-instance'), \
                patch.object(Path, 'glob', return_value=processes), \
                patch('subprocess.check_output', side_effect=output), \
                patch('subprocess.run', side_effect=run), \
                patch('time.monotonic', side_effect=range(30)), \
                patch('time.sleep') as sleep:
            runpy.run_path(str(SCRIPT), run_name='__main__')
        return sleep

    def test_startup_close_uses_wayland_window_without_toggling_fullscreen(self):
        self.execute(['--normal-window'])
        focus = ['hyprctl', 'dispatch', 'hl.dsp.focus({window = "address:0x123"})']
        close = ['xdotool', 'key', '--clearmodifiers', 'super+w']
        self.assertLess(self.commands.index(focus), self.commands.index(close))
        self.assertNotIn(['xdotool', 'key', '--clearmodifiers', 'F11'], self.commands)

    def test_focus_must_reach_the_browser_before_sending_close_key(self):
        with self.assertRaisesRegex(AssertionError, 'did not gain focus'):
            self.execute(['--normal-window'], focused=False)
        self.assertNotIn(['xdotool', 'key', '--clearmodifiers', 'super+w'], self.commands)

    def test_post_input_close_restores_fullscreen_before_closing(self):
        self.execute()
        restore = ['xdotool', 'key', '--clearmodifiers', 'F11']
        close = ['xdotool', 'key', '--clearmodifiers', 'super+w']
        self.assertLess(self.commands.index(restore), self.commands.index(close))

    def test_window_disappearance_waits_for_exact_desktop_profile_exit(self):
        process = Mock()
        # Neither the shell's HOME nor argv[0] selects the Wayland window.
        process.read_bytes.side_effect = [b'wrapper\0' + PROFILE + b'\0', b'']
        sleep = self.execute(['--normal-window'], processes=[process])
        self.assertEqual(process.read_bytes.call_count, 2)
        sleep.assert_called_once_with(0.1)

    def test_lingering_profile_owner_fails_after_window_closes(self):
        process = Mock()
        process.read_bytes.return_value = b'wrapper\0' + PROFILE + b'\0'
        with self.assertRaisesRegex(AssertionError, 'finish closing its profile'):
            self.execute(['--normal-window'], processes=[process])

    def test_flattened_process_title_still_waits_for_profile_exit(self):
        process = Mock()
        process.read_bytes.side_effect = [b'chromium ' + PROFILE + b' --no-first-run\0', b'']
        sleep = self.execute(['--normal-window'], processes=[process])
        self.assertEqual(process.read_bytes.call_count, 2)
        sleep.assert_called_once_with(0.1)

    def test_lingering_flattened_profile_owner_fails(self):
        process = Mock()
        process.read_bytes.return_value = b'chromium ' + PROFILE + b'\0'
        with self.assertRaisesRegex(AssertionError, 'finish closing its profile'):
            self.execute(['--normal-window'], processes=[process])

    def test_flattened_renderer_and_other_profile_do_not_block_exit(self):
        renderer, other = Mock(), Mock()
        renderer.read_bytes.return_value = b'chromium ' + PROFILE + b' --type=renderer\0'
        other.read_bytes.return_value = b'chromium ' + PROFILE + b'-other\0'
        self.execute(['--normal-window'], processes=[renderer, other])

    def test_renderer_or_unrelated_profile_cannot_block_profile_flush(self):
        renderer, other = Mock(), Mock()
        renderer.read_bytes.return_value = b'chromium\0' + PROFILE + b'\0--type=renderer\0'
        other.read_bytes.return_value = b'chromium\0--user-data-dir=/tmp/other\0'
        self.execute(['--normal-window'], processes=[renderer, other])

    def test_missing_or_ambiguous_browser_refuses_to_send_close_key(self):
        for windows in ([], [WINDOW, WINDOW]):
            with self.subTest(windows=windows), self.assertRaisesRegex(AssertionError, 'one fixture browser'):
                self.execute(['--normal-window'], windows=[windows])
            self.assertNotIn(['xdotool', 'key', '--clearmodifiers', 'super+w'], self.commands)

    def test_window_that_does_not_close_fails(self):
        with self.assertRaisesRegex(AssertionError, 'window did not close'):
            self.execute(['--normal-window'], windows=[[WINDOW]] * 30)


if __name__ == '__main__':
    unittest.main()

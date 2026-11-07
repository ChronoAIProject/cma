"""Exercise delayed, stale, missing and conflicting real-UI observations."""
import importlib.util
import itertools
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    'browser_input', Path(__file__).with_name('desktop-browser-input.py'))
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)


def observation(stage, fullscreen=False, viewport=None, active=True):
    return {'stage': stage, 'fullscreen': fullscreen,
            'viewport': viewport or ((1280, 800) if fullscreen else (1040, 640)), 'active': active}


READY = observation('cma-desktop-ready')
NAVIGATED = observation('cma-desktop-input-ready')
FULL = observation('cma-desktop-input-ready', True)
FOCUS = observation('cma-input-focused', True)
TYPED = observation('cma-input:cma-codex', True)
CLICKED = observation('cma-ui:cma-codex', True)
STAGES = [READY, NAVIGATED, FULL, FOCUS, TYPED, CLICKED]


class BrowserInput(unittest.TestCase):
    def execute(self, observations, windowed=False, fail_command=None):
        pending = iter(observations)
        self.last = observation('Loading')
        self.actions = []
        self.reads = []

        def output(*args):
            if args == ('xdotool', 'search', '--onlyvisible', '--class', 'chromium'):
                return '123'
            if args == ('xdotool', 'getactivewindow'):
                self.last = next(pending, self.last)
                self.reads.append(self.last)
                return '123' if self.last['active'] else '456'
            if args == ('xdotool', 'getwindowname', '123'):
                width, height = self.last['viewport']
                return f"{self.last['stage']}|{width}x{height} - Chromium"
            if args == ('xprop', '-id', '123', '_NET_WM_STATE'):
                return '_NET_WM_STATE(ATOM) = ' + (
                    '_NET_WM_STATE_FULLSCREEN, _NET_WM_STATE_FOCUSED' if self.last['fullscreen']
                    else '_NET_WM_STATE_FOCUSED')
            self.fail(f'unexpected observation command: {args}')

        def run(*args):
            self.actions.append((args, self.last))
            if args[-1] == fail_command:
                raise subprocess.CalledProcessError(1, args)

        with patch.object(PROBE, 'output', side_effect=output), \
                patch.object(PROBE, 'run', side_effect=run), \
                patch.object(PROBE.time, 'monotonic', side_effect=itertools.count()), \
                patch.object(PROBE.time, 'sleep'):
            PROBE.windowed() if windowed else PROBE.main('http://127.0.0.1:8877')

    def test_delayed_navigation_and_fullscreen_wait_before_clicks(self):
        self.execute([READY, READY, NAVIGATED, NAVIGATED, FULL, FOCUS, TYPED, CLICKED])
        clicks = [o for args, o in self.actions if args[0] == 'mousemove']
        self.assertEqual(clicks, [FULL, TYPED])
        self.assertEqual(self.actions[-1][1], TYPED)
        self.assertEqual(self.reads[-1], CLICKED)

    def test_stale_initial_title_cannot_acknowledge_navigation(self):
        with self.assertRaisesRegex(AssertionError, 'Timed out waiting for navigation.*observed'):
            self.execute([READY])
        self.assertEqual(len(self.actions), 4)  # Activate + address-bar navigation only.
        self.assertEqual(self.actions[-1][0][-1], 'Return')

    def test_each_missing_acknowledgement_stops_before_later_input(self):
        names = ['initial page', 'navigation', 'fullscreen viewport', 'input focus',
                 'exact input', 'button acknowledgement']
        counts = [1, 4, 5, 6, 7, 8]
        for index, name in enumerate(names):
            with self.subTest(stage=name), self.assertRaisesRegex(AssertionError, 'Timed out waiting for ' + name):
                self.execute(STAGES[:index] + [observation('wrong')])
            self.assertEqual(len(self.actions), counts[index])

    def test_restored_fullscreen_is_not_toggled_out(self):
        self.execute([READY, FULL, FULL, FOCUS, TYPED, CLICKED])
        self.assertFalse(any(args[-1] == 'F11' for args, _ in self.actions))

    def test_window_manager_fullscreen_without_page_viewport_refuses_click(self):
        with self.assertRaisesRegex(AssertionError, 'fullscreen viewport'):
            self.execute([READY, observation('cma-desktop-input-ready', True, (1280, 720))])
        self.assertFalse(any(args[0] == 'mousemove' for args, _ in self.actions))

    def test_full_viewport_without_fullscreen_state_refuses_click(self):
        with self.assertRaisesRegex(AssertionError, 'fullscreen viewport'):
            self.execute([READY, observation('cma-desktop-input-ready', False, (1280, 800))])
        self.assertFalse(any(args[0] == 'mousemove' for args, _ in self.actions))

    def test_inactive_window_cannot_acknowledge_input(self):
        with self.assertRaisesRegex(AssertionError, 'input focus'):
            self.execute(STAGES[:3] + [observation('cma-input-focused', True, active=False)])
        self.assertEqual(self.actions[-1][0], ('mousemove', '120', '120', 'click', '1'))

    def test_exact_input_required_not_a_prefix(self):
        with self.assertRaisesRegex(AssertionError, 'exact input cma-codex'):
            self.execute(STAGES[:4] + [observation('cma-input:cma-codex-wrong', True)])
        self.assertEqual(self.actions[-1][0][-1], 'cma-codex')

    def test_action_failure_is_not_swallowed(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.execute(STAGES, fail_command='F11')
        self.assertEqual(self.actions[-1][0][-1], 'F11')

    def test_existing_first_window_selection_is_preserved(self):
        with patch.object(PROBE, 'output', return_value='123\n456'), \
                patch.object(PROBE, 'run') as run:
            browser = PROBE.Browser()
            self.assertEqual(browser.window, '123')
            run.assert_called_once_with('windowactivate', '--sync', '123')

    def test_close_waits_for_page_and_window_manager_to_leave_fullscreen(self):
        user = observation('cma-ui:cma-user', True)
        normal = observation('cma-ui:cma-user')
        self.execute([user, user, observation('cma-ui:cma-user', False, (1280, 800)), normal], windowed=True)
        self.assertEqual(self.reads[-1], normal)
        self.assertEqual(sum(args[-1] == 'F11' for args, _ in self.actions), 1)

    def test_normal_window_is_not_toggled_into_fullscreen_before_close(self):
        self.execute([observation('cma-ui:cma-user')], windowed=True)
        self.assertEqual(len(self.actions), 1)

    def test_close_timeout_refuses_unacknowledged_window_state(self):
        with self.assertRaisesRegex(AssertionError, 'normal browser viewport before closing'):
            self.execute([observation('cma-ui:cma-user', True)], windowed=True)

    def test_observation_failure_retries_but_has_a_named_timeout(self):
        with patch.object(PROBE.Browser, '__init__', return_value=None), \
                patch.object(PROBE.Browser, 'observe', side_effect=subprocess.TimeoutExpired('xdotool', 1)), \
                patch.object(PROBE.time, 'monotonic', side_effect=itertools.count()), \
                patch.object(PROBE.time, 'sleep'), \
                self.assertRaisesRegex(AssertionError, 'initial page.*TimeoutExpired'):
            PROBE.main('http://127.0.0.1:8877')

    def test_profile_exit_waits_for_original_process_and_refuses_timeout(self):
        with patch.object(PROBE, 'process_identity', side_effect=['start', 'start', None]), \
                patch.object(PROBE.time, 'sleep') as sleep:
            PROBE.wait_process_exit(123, 'start')
            self.assertEqual(sleep.call_count, 2)
        with patch.object(PROBE, 'process_identity', return_value='start'), \
                patch.object(PROBE.time, 'monotonic', side_effect=itertools.count()), \
                patch.object(PROBE.time, 'sleep'), \
                self.assertRaisesRegex(AssertionError, 'closed browser process to exit'):
            PROBE.wait_process_exit(123, 'start')


if __name__ == '__main__':
    unittest.main()

"""Reject stale navigation and missing UI acknowledgements before more input."""
import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    'browser_input', Path(__file__).with_name('omarchy-browser-input.py'))
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)
ORIGIN = 'http://127.0.0.1:8877'
ACTIVE = ['python3', '/opt/probes/omarchy-active-title.py']
NAVIGATE = [('key', '--clearmodifiers', '--delay', '25', 'ctrl+l'),
            ('type', '--clearmodifiers', '--delay', '25', ORIGIN + '/desktop-input'),
            ('key', '--clearmodifiers', 'Return')]
F11 = ('key', '--clearmodifiers', 'F11')
FOCUS = ('mousemove', '120', '120', 'click', '1')
TYPE = ('type', '--clearmodifiers', '--delay', '25', 'cma-codex')
SELECT = ('key', '--clearmodifiers', 'ctrl+a')
VERIFY = ('mousemove', '120', '220', 'click', '1')
ONCE = [*NAVIGATE, F11, FOCUS, TYPE, VERIFY]
STAGE_LIMIT = PROBE.FIRST_ATTEMPT_SECONDS + (PROBE.ATTEMPTS - 1) * PROBE.RETRY_SECONDS
# The single-send probe this replaced waited five seconds for each acknowledgement.
SINGLE_SEND_SECONDS = 5


class Desktop:
    """Chromium as omarchy-browser-page.cjs and Hyprland acknowledge input.

    Input is ordered, as on the real Xvfb, VNC and compositor path: an event
    delivered late holds back every event sent after it.
    """

    def __init__(self, ready_after=0, drop=None, mangle=None, late=None,
                 fullscreen_ack=True, mode=None, fail_command=False):
        self.clock = 0.0
        self.ready_after = ready_after
        self.drop = drop or (lambda index, event: False)
        self.mangle = mangle or (lambda index, event: event)
        self.late = late or (lambda index, event: 0)
        self.fullscreen_ack = fullscreen_ack
        self.mode = mode or (lambda fullscreen: '2' if fullscreen else '0')
        self.fail_command = fail_command
        self.title = 'Untitled'
        self.omnibox = None
        self.field = None
        self.selected = False
        self.fullscreen = False
        self.stored = None
        self.pending = []
        self.actions = []
        self.timed = []
        self.modes = []

    def deliver(self):
        while self.pending and self.pending[0][0] <= self.clock:
            self.apply(self.pending.pop(0)[1])
        if self.title == 'Untitled' and self.clock >= self.ready_after:
            self.title = 'cma-desktop-ready'

    def check_output(self, command, **options):
        assert options == {'text': True}, options
        self.deliver()
        if command == ACTIVE:
            return self.title + ' - Chromium\n'
        assert command == ACTIVE + ['--fullscreen'], command
        self.modes.append(self.mode(self.fullscreen))
        return self.modes[-1] + '\n'

    def run(self, command, **options):
        assert command[0] == 'xdotool' and options == {'check': True}, (command, options)
        self.deliver()
        event = tuple(command[1:])
        index = len(self.actions)
        self.actions.append(event)
        self.timed.append((self.clock, event, self.title))
        if self.fail_command:
            raise subprocess.CalledProcessError(1, command)
        if self.drop(index, event):
            return
        due = max([self.clock + self.late(index, event)] + [item[0] for item in self.pending])
        self.pending.append((due, self.mangle(index, event)))
        self.deliver()

    def navigate(self, url):
        self.title = ('cma-desktop-input-ready' if url == ORIGIN + '/desktop-input'
                      else 'cma-desktop-ready' if url.startswith(ORIGIN + '/') else 'www.google.com')
        self.field = None

    def apply(self, event):
        if event[0] == 'type':
            if self.omnibox is not None:
                self.omnibox = event[-1] if self.selected else self.omnibox + event[-1]
            elif self.field is not None and event[-1]:
                self.field = event[-1] if self.selected else self.field + event[-1]
                self.title = 'cma-input:' + self.field
            self.selected = False
        elif event[-1] == 'ctrl+l':
            self.omnibox, self.field, self.selected = '', None, True
        elif event[-1] == 'ctrl+a':
            self.selected = self.field is not None
        elif event[-1] == 'Return' and self.omnibox is not None:
            url, self.omnibox = self.omnibox, None
            self.navigate(url)
        elif event == F11:
            self.fullscreen = not self.fullscreen
            if self.fullscreen and self.fullscreen_ack and self.title == 'cma-desktop-input-ready':
                self.title = 'cma-desktop-fullscreen'
        elif event == FOCUS and self.fullscreen:
            self.omnibox = None
            if self.field is None:
                self.field = ''
                self.title = 'cma-input-focused'
        elif event == VERIFY and self.fullscreen:
            self.stored = self.field
            self.title = 'cma-ui:' + (self.field or '')

    def sleep(self, seconds):
        self.clock = round(self.clock + seconds, 6)


class BrowserInput(unittest.TestCase):
    def execute(self, **options):
        self.desktop = Desktop(**options)
        self.output = io.StringIO()
        with patch.object(PROBE.subprocess, 'check_output', side_effect=self.desktop.check_output), \
                patch.object(PROBE.subprocess, 'run', side_effect=self.desktop.run), \
                patch.object(PROBE.time, 'monotonic', side_effect=lambda: self.desktop.clock), \
                patch.object(PROBE.time, 'sleep', side_effect=self.desktop.sleep), \
                contextlib.redirect_stdout(self.output):
            PROBE.main(ORIGIN)
        self.assertEqual(self.desktop.title, 'cma-ui:cma-codex')
        self.assertEqual(self.desktop.stored, 'cma-codex')
        # Every passing run proves the compositor query before and after F11.
        self.assertFalse(int(self.desktop.modes[0]) & PROBE.FULLSCREEN)
        self.assertTrue(int(self.desktop.modes[-1]) & PROBE.FULLSCREEN)
        return self.output.getvalue().strip()

    def fails(self, stage, **options):
        with self.assertRaises(AssertionError) as raised:
            self.execute(**options)
        message = str(raised.exception)
        self.assertIn(stage, message)
        return message

    def test_acknowledged_inputs_are_each_sent_once(self):
        output = self.execute()
        self.assertEqual(self.desktop.actions, ONCE)
        self.assertEqual(self.desktop.modes, ['0', '2'])
        self.assertEqual(output, 'browser-navigation-fullscreen-and-input: pass (resent: none)')

    def test_delayed_navigation_does_not_accept_previous_page_title(self):
        self.execute(ready_after=3)
        self.assertGreaterEqual(self.desktop.timed[0][0], 3)
        self.assertEqual(self.desktop.timed[0][2], 'cma-desktop-ready')
        self.assertEqual(next(item for item in self.desktop.timed if item[1] == F11)[2],
                         'cma-desktop-input-ready')

    def test_missing_initial_page_fails_before_any_input(self):
        with self.assertRaisesRegex(AssertionError, "'cma-desktop-ready', got 'Untitled - Chromium'"):
            self.execute(ready_after=60)
        self.assertEqual(self.desktop.actions, [])

    def test_late_but_working_input_is_never_resent(self):
        # Input the single-send probe accepted must never be repeated now.
        for event in [*NAVIGATE, F11, FOCUS, TYPE, VERIFY]:
            with self.subTest(event=event[-1]):
                output = self.execute(late=lambda index, sent, event=event:
                                      SINGLE_SEND_SECONDS - 0.5 if sent == event else 0)
                self.assertEqual(self.desktop.actions, ONCE)
                self.assertTrue(output.endswith('(resent: none)'))

    def test_any_dropped_navigation_event_is_resent_whole(self):
        for dropped in range(len(NAVIGATE)):
            with self.subTest(dropped=NAVIGATE[dropped][-1]):
                output = self.execute(drop=lambda index, event: index == dropped)
                self.assertEqual(self.desktop.actions, [*NAVIGATE, *ONCE])
                self.assertGreaterEqual(self.desktop.timed[len(NAVIGATE)][0], SINGLE_SEND_SECONDS)
                self.assertTrue(output.endswith('(resent: navigation=1)'))

    def test_partial_address_is_replaced_not_appended(self):
        partial = NAVIGATE[1][:-1] + (ORIGIN + '/desktop-inp',)
        self.execute(mangle=lambda index, event: partial if index == 1 else event)
        self.assertEqual(self.desktop.actions, [*NAVIGATE, *ONCE])

    def test_dropped_f11_is_resent_while_the_window_is_normal(self):
        output = self.execute(drop=lambda index, event: event == F11 and index == 3)
        self.assertEqual(self.desktop.actions.count(F11), 2)
        self.assertEqual(self.desktop.modes, ['0', '0', '2'])
        self.assertTrue(output.endswith('(resent: fullscreen=1)'))

    def test_fullscreen_window_is_never_toggled_back_out(self):
        message = self.fails("acknowledgement 'cma-desktop-fullscreen'", fullscreen_ack=False)
        self.assertIn('after 1 of 3 input attempts', message)
        self.assertIn('window already fullscreen (mode 2); F11 not re-sent', message)
        self.assertEqual(self.desktop.actions, [*NAVIGATE, F11])
        self.assertTrue(self.desktop.fullscreen)

    def test_maximized_window_is_not_mistaken_for_fullscreen(self):
        self.execute(mode=lambda fullscreen: '3' if fullscreen else '1',
                     drop=lambda index, event: event == F11 and index == 3)
        self.assertEqual(self.desktop.actions.count(F11), 2)

    def test_fullscreen_query_must_report_an_integer_before_f11(self):
        for reported in ('', 'False', 'true'):
            with self.subTest(reported=reported):
                message = self.fails('no integer fullscreen mode', mode=lambda fullscreen: reported)
                self.assertIn(repr(reported), message)
                self.assertNotIn(F11, self.desktop.actions)

    def test_already_fullscreen_browser_fails_before_f11(self):
        message = self.fails('already fullscreen (mode 2) before F11', mode=lambda fullscreen: '2')
        self.assertNotIn(F11, self.desktop.actions)
        self.assertIn('before F11', message)

    def test_compositor_must_report_fullscreen_after_acknowledgement(self):
        self.fails('fullscreen mode 0 after the fullscreen acknowledgement', mode=lambda fullscreen: '0')
        self.assertEqual(self.desktop.actions, [*NAVIGATE, F11])

    def test_dropped_clicks_are_resent(self):
        for click, stage in ((FOCUS, 'focus'), (VERIFY, 'verify')):
            with self.subTest(click=stage):
                output = self.execute(drop=lambda index, event:
                                      event == click and self.desktop.actions.count(click) == 1)
                self.assertEqual(self.desktop.actions.count(click), 2)
                self.assertTrue(output.endswith(f'(resent: {stage}=1)'))

    def test_partial_or_repeated_typing_is_replaced_with_the_exact_value(self):
        for typed in ('cma-cod', 'cma-coddex', ''):
            with self.subTest(typed=typed):
                self.execute(mangle=lambda index, event:
                             TYPE[:-1] + (typed,) if event == TYPE and self.desktop.actions.count(TYPE) == 1 else event)
                self.assertEqual(self.desktop.actions[-4:], [TYPE, SELECT, TYPE, VERIFY])

    def test_each_missing_acknowledgement_fails_after_bounded_attempts(self):
        stages = [('cma-desktop-input-ready', NAVIGATE[2], 'cma-desktop-ready'),
                  ('cma-desktop-fullscreen', F11, 'cma-desktop-input-ready'),
                  ('cma-input-focused', FOCUS, 'cma-desktop-fullscreen'),
                  ('cma-input:cma-codex', TYPE, 'cma-input-focused'),
                  ('cma-ui:cma-codex', VERIFY, 'cma-input:cma-codex')]
        for stage, event, last in stages:
            with self.subTest(stage=stage):
                message = self.fails(f'acknowledgement {stage!r}',
                                     drop=lambda index, dropped, event=event: dropped == event)
                self.assertIn(f'after 3 of 3 input attempts in {STAGE_LIMIT:.1f}s', message)
                self.assertIn(f'last title {last + " - Chromium"!r}', message)
                self.assertEqual(self.desktop.actions.count(event), 3)
                # No later stage is touched once an acknowledgement is missing.
                self.assertLessEqual(set(self.desktop.actions), {*ONCE[:ONCE.index(event) + 1], SELECT})
                self.assertLessEqual(self.desktop.clock, STAGE_LIMIT + 0.5)

    def test_navigation_to_google_fails_before_fullscreen_or_field_input(self):
        wrong = NAVIGATE[1][:-1] + ('www.google.com',)
        message = self.fails("acknowledgement 'cma-desktop-input-ready'",
                             mangle=lambda index, event: wrong if event == NAVIGATE[1] else event)
        self.assertIn("last title 'www.google.com - Chromium'", message)
        self.assertEqual(self.desktop.actions, NAVIGATE * 3)

    def test_title_prefix_does_not_accept_wrong_input(self):
        wrong = TYPE[:-1] + ('cma-codex-wrong',)
        message = self.fails("acknowledgement 'cma-input:cma-codex'",
                             mangle=lambda index, event: wrong if event == TYPE else event)
        self.assertIn("last title 'cma-input:cma-codex-wrong - Chromium'", message)
        self.assertNotIn(VERIFY, self.desktop.actions)

    def test_keyboard_failure_is_not_swallowed(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.execute(fail_command=True)
        self.assertEqual(len(self.desktop.actions), 1)


if __name__ == '__main__':
    unittest.main()

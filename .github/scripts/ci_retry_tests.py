"""Retries are bounded by attempts and deadline; writes never repeat blindly."""
import ast
import email.message
import errno
import http.client
import socket
import subprocess
import sys
import unittest
import urllib.error
from pathlib import Path
from unittest.mock import Mock

import ci_retry
from ci_retry import ABSENT, COMMITTED, CONFLICT, PERMANENT, TRANSIENT


class Clock:
    """A fake monotonic clock that advances only when the policy sleeps."""

    def __init__(self, now=0.0):
        self.now, self.waits = now, []

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.waits.append(seconds)
        self.now += seconds


def failing(*errors, result="ok"):
    calls = []

    def op():
        calls.append(len(calls))
        if len(calls) <= len(errors):
            raise errors[len(calls) - 1]
        return result
    return op, calls


def policy(clock, log, **extra):
    return dict(name="fixture-read", classify=ci_retry.classify, clock=clock, sleep=clock.sleep,
                rand=lambda: 1.0, log=log.append, **extra)


class CallTests(unittest.TestCase):
    def test_transient_failures_repeat_with_capped_exponential_backoff_and_fixed_log(self):
        clock, log = Clock(), []
        op, calls = failing(TimeoutError(), ConnectionResetError())
        self.assertEqual(ci_retry.call(op, attempts=3, base=1.5, cap=2.0, **policy(clock, log)), "ok")
        self.assertEqual(len(calls), 3)
        self.assertEqual(clock.waits, [1.5, 2.0])
        self.assertEqual(log, ["RETRY op=fixture-read attempt=2/3 wait=1.5s",
                               "RETRY op=fixture-read attempt=3/3 wait=2.0s"])

    def test_full_jitter_draws_inside_the_exponential_bound(self):
        clock, log = Clock(), []
        draws = iter([0.25, 0.5])
        op, _ = failing(TimeoutError(), TimeoutError())
        options = policy(clock, log, base=4.0)
        options["rand"] = lambda: next(draws)
        ci_retry.call(op, **options)
        self.assertEqual(clock.waits, [1.0, 4.0])
        with self.assertRaises(ValueError):
            ci_retry.backoff(1, 1.0, 1.0, lambda: 1.5)

    def test_attempt_bound_raises_the_last_error(self):
        clock, log = Clock(), []
        last = TimeoutError("third")
        op, calls = failing(TimeoutError("first"), TimeoutError("second"), last)
        with self.assertRaises(TimeoutError) as raised:
            ci_retry.call(op, attempts=3, **policy(clock, log))
        self.assertIs(raised.exception, last)
        self.assertEqual(len(calls), 3)
        self.assertEqual(len(clock.waits), 2)

    def test_deadline_is_never_slept_past(self):
        clock, log = Clock(), []
        op, calls = failing(TimeoutError(), TimeoutError(), TimeoutError())
        # The first wait (1 s) fits before t=2.5; the second (2 s) would reach it.
        with self.assertRaises(TimeoutError):
            ci_retry.call(op, attempts=5, deadline=2.5, **policy(clock, log))
        self.assertEqual(len(calls), 2)
        self.assertEqual(clock.waits, [1.0])
        self.assertLess(clock.now, 2.5)
        # A deadline already reached runs the attempt but never waits.
        clock, log = Clock(10.0), []
        op, calls = failing(TimeoutError())
        with self.assertRaises(TimeoutError):
            ci_retry.call(op, deadline=10.0, **policy(clock, log))
        self.assertEqual((len(calls), clock.waits, log), (1, [], []))

    def test_permanent_errors_are_not_repeated(self):
        permanent = (
            urllib.error.HTTPError("https://example.invalid", 401, "unauthorized", {}, None),
            urllib.error.HTTPError("https://example.invalid", 404, "missing", {}, None),
            urllib.error.HTTPError("https://example.invalid", 422, "invalid", {}, None),
            ValueError("digest mismatch"), FileNotFoundError(errno.ENOENT, "gh"),
            urllib.error.URLError("unknown url type"),
            ci_retry.WriteUncertain("fixture"),
        )
        for error in permanent:
            with self.subTest(error=type(error).__name__):
                clock, log = Clock(), []
                op, calls = failing(error)
                with self.assertRaises(type(error)):
                    ci_retry.call(op, **policy(clock, log))
                self.assertEqual((len(calls), clock.waits, log), (1, [], []))

    def test_names_and_bounds_are_validated_before_running(self):
        clock, log = Clock(), []
        op, calls = failing()
        for bad in ("Fixture", "fixture read", "a" * 64, "", None):
            with self.subTest(name=bad), self.assertRaises(ValueError):
                ci_retry.call(op, **dict(policy(clock, log), name=bad))
        with self.assertRaises(ValueError):
            ci_retry.call(op, attempts=0, **policy(clock, log))
        self.assertEqual(calls, [])


class BudgetTests(unittest.TestCase):
    def test_attempts_are_shortened_to_the_budget_and_refused_once_it_is_spent(self):
        clock = Clock(10.0)
        self.assertEqual(ci_retry.attempt_timeout(60, None, clock), 60)
        self.assertEqual(ci_retry.attempt_timeout(60, 100.0, clock), 60)
        self.assertEqual(ci_retry.attempt_timeout(60, 25.0, clock), 15.0)
        self.assertIsNone(ci_retry.attempt_timeout(60, 10.0, clock))

    def test_a_budget_only_ever_shortens_the_retry_deadline(self):
        self.assertEqual(ci_retry.bounded(None, None), {})
        self.assertEqual(ci_retry.bounded({"base": 0.5}, 30.0), {"base": 0.5, "deadline": 30.0})
        self.assertEqual(ci_retry.bounded({"deadline": 20.0}, 30.0), {"deadline": 20.0})
        policy = {"deadline": 40.0}
        self.assertEqual(ci_retry.bounded(policy, 30.0), {"deadline": 30.0})
        self.assertEqual(policy, {"deadline": 40.0})


class ClassifierTests(unittest.TestCase):
    def test_a_throttled_response_names_its_wait_and_an_opted_in_read_honours_it(self):
        self.assertEqual(ci_retry.retry_after("Retry-After: 45\n"), 45)
        self.assertEqual(ci_retry.retry_after({"retry-after": "Wed, 21 Oct 2015 07:28:30 GMT"},
                                              now=1445412480), 30)
        self.assertEqual(ci_retry.retry_after({"x-ratelimit-remaining": "0", "x-ratelimit-reset": "1000"}, now=940), 60)
        for headers in ({}, {"retry-after": "soon"}, {"x-ratelimit-remaining": "1", "x-ratelimit-reset": "9"}):
            self.assertIsNone(ci_retry.retry_after(headers, now=0))
        self.assertEqual(ci_retry.status_cause(429, {"retry-after": "12"}).after, 12)
        self.assertIsNone(ci_retry.status_cause(503).after)
        self.assertIsNone(ci_retry.status_cause(404, {"retry-after": "12"}))

        def throttled():
            raise RuntimeError("read failed") from ci_retry.status_cause(429, {"retry-after": "12"})
        for honour, wait in ((True, 12), (False, 1.0)):
            with self.subTest(honour=honour):
                clock = Clock()
                with self.assertRaises(RuntimeError):
                    ci_retry.call(throttled, name="fixture-read", classify=ci_retry.classify, attempts=2,
                                  base=2.0, cap=30.0, clock=clock, sleep=clock.sleep, rand=lambda: 0.5,
                                  log=lambda _line: None, honour_retry_after=honour)
                self.assertEqual(clock.waits, [wait])
        # Only ASCII seconds count; other scripts' digits are no wait and never raise.
        for value in ("\u00b2", "\u0663", "1" * 11, "-5", "1.5"):
            self.assertIsNone(ci_retry.retry_after({"retry-after": value}, now=0))
            self.assertIsNone(ci_retry.status_cause(429, {"retry-after": value}).after)
        self.assertIsNone(ci_retry.retry_after({"x-ratelimit-remaining": "0", "x-ratelimit-reset": "\u00b2"}, now=0))

        # One requested wait is capped, with or without a deadline.
        def slow():
            raise RuntimeError("read failed") from ci_retry.status_cause(429, {"retry-after": "600"})
        clock = Clock()
        with self.assertRaises(RuntimeError):
            ci_retry.call(slow, name="fixture-read", classify=ci_retry.classify, attempts=2, clock=clock,
                          sleep=clock.sleep, rand=lambda: 0.5, log=lambda _line: None, honour_retry_after=True)
        self.assertEqual(clock.waits, [ci_retry.RETRY_AFTER_CAP])
        # A requested wait past the deadline ends the read instead of sleeping.
        clock = Clock()
        with self.assertRaises(RuntimeError):
            ci_retry.call(throttled, name="fixture-read", classify=ci_retry.classify, attempts=3, deadline=10,
                          clock=clock, sleep=clock.sleep, rand=lambda: 0.5, log=lambda _line: None,
                          honour_retry_after=True)
        self.assertEqual(clock.waits, [])

    def test_a_mixed_in_transient_error_without_its_initializer_asked_for_no_wait(self):
        class Refused(ValueError):
            def __init__(self, *args):
                super().__init__(*args)  # Never reaches TransientError.__init__.

        class TransientRefusal(Refused, ci_retry.TransientError):
            pass
        error = TransientRefusal("read failed")
        self.assertIsNone(error.after)
        self.assertIsNone(ci_retry.requested_wait(error))
        try:
            raise TransientRefusal("read failed") from ci_retry.status_cause(429, {"retry-after": "12"})
        except TransientRefusal as chained:
            self.assertEqual(ci_retry.requested_wait(chained), 12)

    def test_http_statuses(self):
        for code in (408, 425, 429, 500, 502, 503, 504):
            self.assertEqual(ci_retry.http_status(code), TRANSIENT, code)
        for code in (400, 401, 403, 404, 409, 422):
            self.assertEqual(ci_retry.http_status(code), PERMANENT, code)

    def test_a_403_is_throttling_only_with_an_explicit_header(self):
        throttled = ("HTTP/2.0 403 Forbidden\nX-RateLimit-Remaining: 0\nX-RateLimit-Reset: 1",
                     "HTTP/2.0 403 Forbidden\nretry-after: 60", {"Retry-After": "60"},
                     {"x-ratelimit-remaining": "0"})
        for headers in throttled:
            with self.subTest(headers=headers):
                self.assertEqual(ci_retry.http_status(403, headers), TRANSIENT)
                self.assertIsInstance(ci_retry.status_cause(403, headers), ci_retry.TransientError)
        for code, headers in ((403, None), (403, "X-RateLimit-Remaining: 4999"), (403, {"X-Other": "0"}),
                              (403, "HTTP/2.0 403 Forbidden\nX-RateLimit-Remaining: 10"),
                              (401, {"Retry-After": "60"}), (404, "X-RateLimit-Remaining: 0")):
            with self.subTest(code=code, headers=headers):
                self.assertEqual(ci_retry.http_status(code, headers), PERMANENT)
                self.assertIsNone(ci_retry.status_cause(code, headers))
        message = email.message.Message()
        message["Retry-After"] = "30"
        throttled = urllib.error.HTTPError("https://example.invalid", 403, "forbidden", message, None)
        denied = urllib.error.HTTPError("https://example.invalid", 403, "forbidden", email.message.Message(), None)
        self.assertEqual((ci_retry.classify(throttled), ci_retry.classify(denied)), (TRANSIENT, PERMANENT))

    def test_exception_types_and_explicit_causes(self):
        def chained(cause):
            try:
                raise ValueError("fixed public message") from cause
            except ValueError as error:
                return error
        transient = (
            urllib.error.HTTPError("https://example.invalid", 503, "unavailable", {}, None),
            urllib.error.URLError(socket.gaierror(-3, "Temporary failure in name resolution")),
            urllib.error.URLError(ConnectionRefusedError()), subprocess.TimeoutExpired("gh", 60),
            http.client.IncompleteRead(b""), http.client.RemoteDisconnected(), BrokenPipeError(),
            OSError(errno.ENETUNREACH, "unreachable"), ci_retry.TransientError(),
            chained(TimeoutError()), chained(ci_retry.TransientError()),
        )
        for error in transient:
            with self.subTest(error=repr(error)):
                self.assertEqual(ci_retry.classify(error), TRANSIENT)
        # Implicit context is not a cause: a later logic failure stays permanent.
        try:
            try:
                raise TimeoutError()
            except TimeoutError:
                raise ValueError("unrelated")
        except ValueError as error:
            self.assertEqual(ci_retry.classify(error), PERMANENT)
        # A permanent marker ends the chain even over a transient cause.
        self.assertEqual(ci_retry.classify(chained(ci_retry.WriteUncertain())), PERMANENT)
        try:
            raise ci_retry.WriteUncertain("fixture") from TimeoutError()
        except ci_retry.WriteUncertain as error:
            self.assertEqual(ci_retry.classify(error), PERMANENT)

    def test_cli_transport_messages(self):
        transient = (
            "gh: Server Error (HTTP 502)", "gh: HTTP 503", b"HTTP/2.0 500 Internal Server Error",
            "error connecting to api.github.com\ncheck your internet connection",
            'Get "https://api.github.com/x": read tcp 10.0.0.1:1->10.0.0.2:443: read: connection reset by peer',
            "net/http: TLS handshake timeout", "dial tcp: lookup ghcr.io: no such host",
            "writing blob: received unexpected HTTP status: 502 Bad Gateway",
            "fatal: unable to access: The requested URL returned error: 504",
            "error: RPC failed; curl 92 HTTP/2 stream 5 was not closed cleanly",
            "fatal: the remote end hung up unexpectedly",
            "command failed with exit status 128: Connection reset by peer",
            "toomanyrequests: retry later", "local error: tls: bad record MAC",
            'writing blob: Patch "https://ghcr.io/v2/x/blobs/uploads/y": unexpected EOF',
            'Get "https://api.github.com/repos/x": EOF', "read tcp 10.0.0.1:1->10.0.0.2:443: unexpected EOF",
            "context deadline exceeded (Client.Timeout exceeded while awaiting headers)",
        )
        permanent = (
            "", None, "gh: Not Found (HTTP 404)", "gh: Bad credentials (HTTP 401)",
            "gh: Validation Failed (HTTP 422)", "unauthorized: authentication required",
            "denied: permission_denied: write_package", "manifest unknown",
            "Reference already exists (HTTP 422) after timeout",
            # A local archive cut short is damaged, not a network blip.
            "unexpected EOF", "archive/tar: unexpected EOF",
            "copying system image: reading blob sha256:abc: unexpected EOF",
            # Text carries no headers, so a 403 is never read as throttling.
            "You have exceeded a secondary rate limit (HTTP 403)", "API rate limit exceeded (HTTP 403)",
        )
        for text in transient:
            with self.subTest(text=text):
                self.assertEqual(ci_retry.cli_message(text), TRANSIENT)
                self.assertIsInstance(ci_retry.cli_cause(text), ci_retry.TransientError)
        for text in permanent:
            with self.subTest(text=text):
                self.assertEqual(ci_retry.cli_message(text), PERMANENT)
                self.assertIsNone(ci_retry.cli_cause(text))
        self.assertIsInstance(ci_retry.status_cause(429), ci_retry.TransientError)
        self.assertIsNone(ci_retry.status_cause(404))


class WriteTests(unittest.TestCase):
    def write(self, *outcomes):
        calls = []

        def op():
            calls.append("write")
            outcome = outcomes[0]
            if isinstance(outcome, BaseException):
                raise outcome
            return outcome
        return op, calls

    def test_successful_and_permanent_writes_run_once_without_readback(self):
        clock, log = Clock(), []
        readback = Mock()
        op, calls = self.write("created")
        options = policy(clock, log)
        self.assertEqual(ci_retry.call_write(op, readback, **options), "created")
        denied = urllib.error.HTTPError("https://example.invalid", 403, "denied", {}, None)
        op, calls = self.write(denied)
        with self.assertRaises(urllib.error.HTTPError):
            ci_retry.call_write(op, readback, **options)
        self.assertEqual(calls, ["write"])
        readback.assert_not_called()

    def test_uncertain_write_runs_exactly_once_and_resolves_by_retried_readback(self):
        for state, expected in ((COMMITTED, None), (CONFLICT, ci_retry.WriteConflict),
                                (ABSENT, subprocess.TimeoutExpired), ("unexpected", ci_retry.WriteUncertain)):
            with self.subTest(state=state):
                clock, log = Clock(), []
                failure = subprocess.TimeoutExpired("skopeo", 1800)
                op, calls = self.write(failure)
                reads = []

                def readback():
                    reads.append(len(reads))
                    if len(reads) == 1:
                        raise ConnectionResetError()
                    return state
                if expected is None:
                    self.assertIsNone(ci_retry.call_write(op, readback, **policy(clock, log)))
                else:
                    with self.assertRaises(expected) as raised:
                        ci_retry.call_write(op, readback, **policy(clock, log))
                    if expected is subprocess.TimeoutExpired:
                        self.assertIs(raised.exception, failure)
                self.assertEqual(calls, ["write"])
                self.assertEqual(len(reads), 2)
                self.assertEqual(log, ["RETRY op=fixture-read-readback attempt=2/3 wait=1.0s"])

    def test_unreadable_target_is_uncertain_and_never_repeated(self):
        clock, log = Clock(), []
        op, calls = self.write(subprocess.TimeoutExpired("git", 10))
        readback = Mock(side_effect=TimeoutError())
        with self.assertRaises(ci_retry.WriteUncertain):
            ci_retry.call(lambda: ci_retry.call_write(op, readback, **policy(clock, log)),
                          **policy(clock, log, attempts=3))
        self.assertEqual(calls, ["write"])
        self.assertEqual(readback.call_count, 3)

    def test_outer_policy_repeats_a_write_only_after_proven_absence(self):
        clock, log = Clock(), []
        outcomes = [subprocess.TimeoutExpired("skopeo", 1800), "pushed"]
        calls = []

        def op():
            calls.append("write")
            outcome = outcomes[len(calls) - 1]
            if isinstance(outcome, BaseException):
                raise outcome
            return outcome
        readback = Mock(return_value=ABSENT)
        result = ci_retry.call(lambda: ci_retry.call_write(op, readback, readback_attempts=1, **policy(clock, log)),
                               **policy(clock, log, attempts=2))
        self.assertEqual(result, "pushed")
        self.assertEqual(calls, ["write", "write"])
        readback.assert_called_once_with()


class LeafTests(unittest.TestCase):
    def test_retry_lines_cross_a_private_descriptor_redirect_alone(self):
        # run_private silences descriptors 1 and 2 after imports; only the fixed line escapes.
        script = ("import os, ci_retry\n"
                  "sink = os.open(os.devnull, os.O_WRONLY)\n"
                  "os.dup2(sink, 1)\nos.dup2(sink, 2)\n"
                  "print('private response', flush=True)\n"
                  "ci_retry.emit('RETRY op=fixture-read attempt=2/3 wait=0.5s')\n")
        result = subprocess.run([sys.executable, "-B", "-c", script], cwd=Path(ci_retry.__file__).parent,
                                capture_output=True, text=True, timeout=30, check=True)
        self.assertEqual(result.stdout, "RETRY op=fixture-read attempt=2/3 wait=0.5s\n")
        self.assertEqual(result.stderr, "")

    def test_module_imports_only_the_standard_library(self):
        tree = ast.parse(Path(ci_retry.__file__).read_text(encoding="utf-8"))
        names = {alias.name.split(".")[0] for node in ast.walk(tree) if isinstance(node, ast.Import)
                 for alias in node.names}
        names |= {node.module.split(".")[0] for node in ast.walk(tree)
                  if isinstance(node, ast.ImportFrom) and node.module}
        self.assertEqual(names, {"email", "errno", "http", "os", "random", "re", "socket", "ssl", "subprocess",
                                 "sys", "time", "urllib"})


if __name__ == "__main__":
    unittest.main()

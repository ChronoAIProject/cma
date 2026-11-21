"""Failing test names leave private BuildKit logs only through strict failure patterns."""
import base64
import contextlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

import staging_cd_publish as publication
import staging_cd_test_failures as failures
from staging_cd_build import build
from staging_cd_build_progress import summarize
import staging_cd_build_tests
from staging_cd_publication_diagnostics import ImageCommandRefusal
from staging_cd_reservations import next_reservation
from staging_cd_receipts_tests import fixture_identity, fixture_receipt

# Stdout of `node --test` on Node 24.19.0 without a TTY. Each test throws or
# prints text shaped like reporter output: escapes.log puts \\r, U+2028 and
# \\f before a fake failure line in error messages; nested.log has a nested
# suite, a TAP-shaped multi-line error, a timeout and a file that fails to
# load; forged.log's test prints a fake spec summary with console.log and a
# fake unittest block with console.error, which Node forwards to stdout.
CAPTURES = Path(__file__).resolve().parent / "fixtures" / "node24-test-reporter"
ROOT = Path(__file__).resolve().parents[2]
RUNTIME = "api/crates/layer4-runtime/Dockerfile"
GATEWAY = "api/crates/cmaeg/Dockerfile"
QUALIFICATION = "[runtime-qualification 1/1] RUN --mount=type=bind,source=private-source cp -a /payload/. /"
STEP = "runtime-qualification 1/1"
MEDIA_STEP = "[media-self-test 7/7] RUN --network=none <<'MEDIA_SELF_TEST'"

# Node 24's default reporter without a TTY (BuildKit gives none) is spec.
# Captured from `docker buildx build --progress=rawjson`, with private markers added.
SPEC = """\
✔ passes quickly (0.438875ms)
✖ reopens the original conversation after restart (0.548ms)
▶ suite of things
  ✖ nested failure with <angle> & "quotes" (0.112292ms)
  ✔ nested pass (0.106666ms)
✖ suite of things (0.461083ms)
✖ /tmp/cma-codex-source/crash.test.cjs (24.379458ms)
✖ inline private-only line before the summary (1ms)
ℹ tests 6
ℹ fail 4

✖ failing tests:

test at tmp/cma-codex-source/sample.test.cjs:4:1
✖ reopens the original conversation after restart (0.548ms)
  AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:

  'private-actual' !== 'private-expected'

      at TestContext.<anonymous> (/tmp/cma-codex-source/sample.test.cjs:4:72)

test at tmp/cma-codex-source/sample.test.cjs:6:8
✖ nested failure with <angle> & "quotes" (0.112292ms)
  Error: private-detail
  ✖ indented private line inside an error (1ms)

test at tmp/cma-codex-source/crash.test.cjs:1:1
✖ /tmp/cma-codex-source/crash.test.cjs (24.379458ms)
  'test failed'

test at tmp/cma-codex-source/timeout.test.cjs:2:1
✖ session open overlap stays bounded (21.798166ms)
  'test timed out after 20ms'
"""

TAP = """\
TAP version 13
# node:internal/test_runner/harness:124
# Error: private load failure
# Subtest: passes quickly
ok 1 - passes quickly
  ---
  duration_ms: 0.490833
  type: 'test'
  ...
# Subtest: reopens the original conversation after restart
not ok 2 - reopens the original conversation after restart
  ---
  duration_ms: 0.56325
  type: 'test'
  location: '/tmp/cma-codex-source/sample.test.cjs:4:1'
  failureType: 'testCodeFailure'
  error: |-
    private-detail
    not ok 9 - private line inside an error
  code: 'ERR_ASSERTION'
  ...
# Subtest: suite of things
    # Subtest: nested failure with <angle> & "quotes"
    not ok 1 - nested failure with <angle> & "quotes"
      ---
      duration_ms: 0.132208
      type: 'test'
      location: '/tmp/cma-codex-source/sample.test.cjs:6:8'
      failureType: 'testCodeFailure'
      error: 'private-detail'
      ...
    1..1
not ok 3 - suite of things
  ---
  duration_ms: 0.805334
  type: 'suite'
  location: '/tmp/cma-codex-source/sample.test.cjs:5:6'
  failureType: 'subtestsFailed'
  error: '1 subtest failed'
  ...
# Subtest: /tmp/cma-codex-source/crash.test.cjs
not ok 4 - /tmp/cma-codex-source/crash.test.cjs
  ---
  duration_ms: 25.966542
  type: 'test'
  location: '/tmp/cma-codex-source/crash.test.cjs:1:1'
  failureType: 'testCodeFailure'
  exitCode: 7
  error: 'test failed'
  ...
not ok 5 - retries a known flake # TODO private reason
  ---
  duration_ms: 1
  ...
not ok 6 - escaped \\# hash and first\\nsecond
  ---
  location: '/tmp/cma-codex-source/turn_steering.test.cjs:9:1'
  ...
not ok 7 - private line without a diagnostic block
1..7
"""

# Python 3.12 unittest, as both recovery scripts run (`unittest.main()`):
# errors first, then failures, then the totals the block count is checked against.
UNITTEST = """\
.FEF
======================================================================
ERROR: test_beta_errors (__main__.Fixture.test_beta_errors)
----------------------------------------------------------------------
Traceback (most recent call last):
  File "/tmp/cma-codex-source/recovery_test.py", line 6, in test_beta_errors
    raise RuntimeError("private detail")
RuntimeError: private detail
FAIL: test_private_inside_a_traceback (private.Module)

======================================================================
ERROR: setUpClass (__main__.DesktopRecovery)
----------------------------------------------------------------------
Traceback (most recent call last):
  File "/tmp/cma-codex-source/recovery_desktop_test.py", line 30, in setUpClass
OSError: private

======================================================================
FAIL: test_alpha_fails (__main__.Fixture.test_alpha_fails)
----------------------------------------------------------------------
Traceback (most recent call last):
  File "/tmp/cma-codex-source/recovery_test.py", line 4, in test_alpha_fails
    self.assertEqual(1, 2)
AssertionError: 1 != 2

======================================================================
FAIL: test_gamma_sub (__main__.Fixture.test_gamma_sub) (mode='private-b')
----------------------------------------------------------------------
Traceback (most recent call last):
  File "/tmp/cma-codex-source/recovery_test.py", line 10, in test_gamma_sub
AssertionError: 'private-b' != 'a'

======================================================================
FAIL: test_gamma_sub (__main__.Fixture.test_gamma_sub) (mode='private-c')
----------------------------------------------------------------------
Traceback (most recent call last):
  File "/tmp/cma-codex-source/recovery_test.py", line 10, in test_gamma_sub
AssertionError: 'private-c' != 'a'

======================================================================
FAIL: test_older_python (recovery_test.Fixture)
Docstring line of a private description.
----------------------------------------------------------------------
Traceback (most recent call last):
  File "/tmp/private/helper.py", line 3, in helper

----------------------------------------------------------------------
Ran 7 tests in 0.001s

FAILED (failures=4, errors=2)
"""

MEDIA = """\
media-self-test case=h264_aac.mp4 op=keyframes outcome=ok frames=6 pcm_bytes=0 ms=12
media-self-test case=voice.ogg op=audio outcome=timeout frames=0 pcm_bytes=0 ms=30000
media-self-test case=private;fixture op=audio outcome=exit_nonzero frames=0 pcm_bytes=0 ms=1
"""


SPEC_NAMES = [
    {"step": STEP, "file": "sample.test.cjs", "test": "reopens the original conversation after restart"},
    {"step": STEP, "file": "sample.test.cjs", "test": "nested failure with <angle> quotes"},
    {"step": STEP, "file": "crash.test.cjs"},
    {"step": STEP, "file": "timeout.test.cjs", "test": "session open overlap stays bounded"}]
UNITTEST_NAMES = [
    {"step": STEP, "file": "recovery_test.py", "test": "Fixture.test_beta_errors"},
    {"step": STEP, "file": "recovery_desktop_test.py", "test": "DesktopRecovery.setUpClass"},
    {"step": STEP, "file": "recovery_test.py", "test": "Fixture.test_alpha_fails"},
    {"step": STEP, "file": "recovery_test.py", "test": "Fixture.test_gamma_sub"},
    {"step": STEP, "test": "recovery_test.Fixture.test_older_python"}]


def digest(number):
    return "sha256:" + format(number, "x") * 64


def progress(*steps, chunk=37):
    """BuildKit rawjson: a started vertex, its base64 log chunks, then its completion."""
    lines = []
    for number, name, error, streams in steps:
        started = {"digest": digest(number), "name": name, "started": "2026-11-21T00:00:01Z"}
        lines.append({"vertexes": [started]})
        for stream, text in streams.items():
            data = text.encode() if isinstance(text, str) else text
            for start in range(0, len(data), chunk):
                lines.append({"logs": [{"vertex": digest(number), "stream": stream, "timestamp": "2026-11-21T00:00:02Z",
                                        "data": base64.b64encode(data[start:start + chunk]).decode()}]})
        completed = dict(started, completed=f"2026-11-21T00:00:{number:02d}Z")
        if error:
            completed["error"] = error
        lines.append({"vertexes": [completed]})
    # buildx prints its private error text after the JSON stream.
    return b"\n".join(json.dumps(line).encode() for line in lines) + b"\nERROR: failed to solve: private trailer\n"


def failed_qualification(stdout="", stderr=""):
    raw = progress((1, QUALIFICATION, "process did not complete successfully: private", {1: stdout, 2: stderr}))
    return summarize(raw, dockerfile=RUNTIME)


class ExtractionTests(unittest.TestCase):
    def test_node_spec_summary_names_each_failing_test_and_its_file(self):
        result = failed_qualification(stdout=SPEC)
        self.assertEqual(result["failed_steps"], [STEP])
        self.assertEqual(result["failed_tests"], SPEC_NAMES)
        self.assertNotIn("private", json.dumps(result))

    def test_node_tap_names_leaf_failures_and_skips_parents_todos_and_unstructured_lines(self):
        result = failed_qualification(stdout=TAP)
        self.assertEqual(result["failed_tests"], [
            {"step": STEP, "file": "sample.test.cjs", "test": "reopens the original conversation after restart"},
            {"step": STEP, "file": "sample.test.cjs", "test": "nested failure with <angle> quotes"},
            {"step": STEP, "file": "crash.test.cjs"},
            {"step": STEP, "file": "turn_steering.test.cjs", "test": "escaped # hash and first second"}])
        self.assertNotIn("private", json.dumps(result))

    def test_python_unittest_names_failures_errors_and_their_script(self):
        result = failed_qualification(stderr=UNITTEST)
        self.assertEqual(result["failed_tests"], UNITTEST_NAMES)
        self.assertNotIn("private", json.dumps(result))

    def test_each_parser_reads_only_its_own_stream(self):
        result = failed_qualification(stdout=SPEC, stderr=UNITTEST)
        self.assertEqual(result["failed_tests"], (SPEC_NAMES + UNITTEST_NAMES)[:failures.MAX_NAMES])
        # Node forwards a test's stdout and stderr to stdout: a unittest report
        # there is not unittest's, and spec or TAP text on stderr is not node's.
        self.assertNotIn("failed_tests", failed_qualification(stdout=UNITTEST, stderr=SPEC))
        self.assertNotIn("failed_tests", failed_qualification(stderr=TAP))

    def test_media_self_test_names_failed_cases_only_in_its_own_stage(self):
        result = summarize(progress((7, MEDIA_STEP, "private", {1: MEDIA + SPEC})), dockerfile=GATEWAY)
        self.assertEqual(result["failed_tests"], [{"step": "media-self-test 7/7", "file": "voice.ogg",
                                                   "test": "audio timeout"}])
        self.assertNotIn("failed_tests", failed_qualification(stdout=MEDIA))
        self.assertNotIn("private", json.dumps(result))

    def test_a_passing_build_or_another_stage_publishes_no_names(self):
        cases = {
            "passing step": progress((1, QUALIFICATION, None, {1: SPEC, 2: UNITTEST})),
            "unlisted stage": progress((2, "[runtime-tools 9/12] RUN node private.cjs", "private", {1: SPEC})),
            "internal vertex": progress((3, "[internal] load build context", "private", {1: TAP})),
            "unnamed vertex": progress((4, "private vertex", "private", {1: TAP})),
            "prefix of a listed stage": progress((5, "[runtime-qualification-extra 1/1] RUN x", "private", {1: TAP})),
            "no output": progress((6, QUALIFICATION, "private", {})),
        }
        for case, raw in cases.items():
            with self.subTest(case=case):
                result = summarize(raw, dockerfile=RUNTIME)
                self.assertNotIn("failed_tests", result)
                self.assertNotIn("private", json.dumps(result))
        # Another vertex's output never names a failed qualification step.
        passing = (1, "[runtime-tools 9/12] RUN node private.cjs", None, {1: SPEC})
        failed = (2, QUALIFICATION, "private", {})
        self.assertNotIn("failed_tests", summarize(progress(passing, failed), dockerfile=RUNTIME))

    def test_a_listed_stage_names_tests_only_in_the_dockerfile_bound_to_it(self):
        qualification = progress((1, QUALIFICATION, "private", {1: SPEC}))
        media = progress((7, MEDIA_STEP, "private", {1: MEDIA}))
        self.assertEqual(summarize(qualification, dockerfile=RUNTIME)["failed_tests"], SPEC_NAMES)
        self.assertEqual(len(summarize(media, dockerfile=GATEWAY)["failed_tests"]), 1)
        for raw, dockerfile in ((qualification, GATEWAY), (media, RUNTIME), (qualification, None), (media, None),
                                (qualification, "web/Dockerfile"), (qualification, "private/Dockerfile")):
            with self.subTest(dockerfile=dockerfile):
                result = summarize(raw, dockerfile=dockerfile)
                self.assertNotIn("failed_tests", result)
                self.assertEqual(len(result["failed_steps"]), 1)

    def test_names_are_de_duplicated_bounded_and_ordered_by_failure(self):
        repeated = "".join(f"not ok {n} - flaky {n % 12}\n  ---\n  ...\n" for n in range(1, 40))
        result = failed_qualification(stdout="TAP version 13\n" + repeated)
        self.assertEqual([row["test"] for row in result["failed_tests"]],
                         [f"flaky {n}" for n in range(1, failures.MAX_NAMES + 1)])
        two = summarize(progress((2, QUALIFICATION, "private", {1: "TAP version 13\nnot ok 1 - second\n  ---\n"}),
                                 (1, QUALIFICATION, "private", {1: "TAP version 13\nnot ok 1 - first\n  ---\n"})),
                        dockerfile=RUNTIME)
        self.assertEqual([row["test"] for row in two["failed_tests"]], ["first", "second"])

    def test_malformed_records_are_ignored(self):
        good = {"vertex": digest(1), "stream": 1, "data": base64.b64encode(b"TAP version 13\nnot ok 1 - kept\n  ---\n").decode()}
        records = [{"logs": "private"}, {"logs": [None, "private", 7]},
                   {"logs": [dict(good, vertex=["private"])]}, {"logs": [dict(good, vertex=None)]},
                   {"logs": [dict(good, stream=3)]}, {"logs": [dict(good, stream=True)]},
                   {"logs": [dict(good, stream="1")]}, {"logs": [dict(good, data="!!private!!")]},
                   {"logs": [dict(good, data=None)]}, {"logs": [good]}, ["logs"], "private"]
        rows = [{"vertexes": [{"digest": digest(1), "name": QUALIFICATION, "started": "2026-11-21T00:00:01Z",
                               "completed": "2026-11-21T00:00:02Z", "error": "private"}]}]
        raw = b"\n".join(json.dumps(row).encode() for row in records + rows) + b"\nlogs private\n"
        result = summarize(raw, dockerfile=RUNTIME)
        self.assertEqual(result["failed_tests"], [{"step": STEP, "test": "kept"}])
        self.assertEqual(summarize(raw.decode(), dockerfile=RUNTIME)["failed_tests"], result["failed_tests"])


    def test_real_node_24_captures_name_only_the_reporters_own_failures(self):
        cases = {
            "escapes.log": [("a.test.cjs", "plain literal failure"), ("a.test.cjs", "error with carriage return"),
                            ("a.test.cjs", "error with line separator"), ("a.test.cjs", "error with form feed")],
            "nested.log": [("c.test.cjs", "nested literal failure"), ("c.test.cjs", "multi-line error content"),
                           ("c.test.cjs", "timeout case"), ("d.test.cjs", None)],
            "forged.log": [("e.test.cjs", "logs diagnostics")],
        }
        for name, expected in cases.items():
            with self.subTest(capture=name):
                capture = (CAPTURES / name).read_bytes()
                result = failed_qualification(stdout=capture)
                self.assertEqual(result["failed_tests"], [
                    {"step": STEP, "file": file, **({"test": test} if test else {})} for file, test in expected])
                text = json.dumps(result)
                for leaked in ("LEAKED", "cr_leak", "TAP-SHAPED", "FORGED", "forged", "Forged"):
                    self.assertNotIn(leaked, text)
                # The same bytes on stderr name nothing: node never writes its reporter there.
                self.assertNotIn("failed_tests", failed_qualification(stderr=capture))

    def test_error_text_split_only_at_newlines_never_reaches_column_zero(self):
        separators = ["\r", "\f", "\v", "\x1c", "\x1d", "\x1e", "\x85", " ", " "]
        details = "".join(f"  Error: child said{separator}✖ LEAKED-{index} ({index}ms){separator}"
                          f"test at x/leak_{index}.test.cjs:1:1\n" for index, separator in enumerate(separators))
        summary = "\n✖ failing tests:\n\ntest at a.test.cjs:1:1\n✖ real failure (1ms)\n" + details
        result = failed_qualification(stdout=summary.encode())
        self.assertEqual(result["failed_tests"], [{"step": STEP, "file": "a.test.cjs", "test": "real failure"}])
        # CRLF output still parses: one trailing \r is part of the line ending.
        crlf = summary.replace("\n", "\r\n").encode()
        self.assertEqual(failed_qualification(stdout=crlf)["failed_tests"], result["failed_tests"])

    def test_text_shaped_like_another_reporter_or_an_earlier_report_is_not_read(self):
        # Indented TAP-shaped error text inside a spec summary is not TAP.
        spec = ("✖ failing tests:\n\ntest at a.test.cjs:1:1\n✖ real failure (1ms)\n"
                "  Error: bridge said:\n  not ok 1 - SPEC-FORGED\n    ---\n    ...\n")
        self.assertEqual(failed_qualification(stdout=spec)["failed_tests"],
                         [{"step": STEP, "file": "a.test.cjs", "test": "real failure"}])
        # A TAP point's YAML block re-indents quoted text to look like a nested point.
        tap = ("TAP version 13\nnot ok 1 - real tap failure\n  ---\n  error: |-\n    bridge said:\n"
               "    not ok 1 - YAML-FORGED\n      ---\n      ...\n  ...\nnot ok 2 - later failure\n  ---\n  ...\n")
        self.assertEqual([row["test"] for row in failed_qualification(stdout=tap)["failed_tests"]],
                         ["real tap failure", "later failure"])
        # A test's console.log printed before the real summary imitates one.
        forged = ("✖ failing tests:\n\ntest at x/forged_file.cjs:1:1\n✖ STDOUT-FORGED (1ms)\n"
                  "✖ logs diagnostics (2ms)\n\n" + SPEC)
        self.assertEqual(failed_qualification(stdout=forged)["failed_tests"], SPEC_NAMES)
        # A test's own stderr can print a unittest block before the real report;
        # only the last failures + errors blocks before the last totals count.
        block = "=" * 70 + "\nFAIL: test_forged (stderr.Forged.test_forged)\n" + "-" * 70 + "\n"
        self.assertEqual(failed_qualification(stderr=block + UNITTEST)["failed_tests"], UNITTEST_NAMES)
        passed = "\n" + "-" * 70 + "\nRan 3 tests in 0.010s\n\nOK\n"
        for stderr in (block, block + passed, UNITTEST + block + passed, UNITTEST.replace("FAILED (", "FAILED ((")):
            with self.subTest(stderr=stderr[-40:]):
                self.assertNotIn("failed_tests", failed_qualification(stderr=stderr))


class SanitizationTests(unittest.TestCase):
    def test_hostile_or_secret_looking_titles_are_dropped_or_neutralized(self):
        token = "gh" + "p_" + "Ab1Cd2" * 6
        cases = [
            ("leaks " + token, None),
            ("digest " + "0123456789abcdef" * 2, None),
            ("blob " + base64.b64encode(bytes(range(40))).decode(), None),
            ("lowercase key " + "k3y" * 9, None),
            ("sets password=hunter2", None),
            ("api_key: short", None),
            ("value=" + "v" * 12, None),
            ("opens https://user:pass@example.invalid/path", None),
            ("auth header Bearer abc", None),
            ("pem -----BEGIN key", None),
            ("::error title=x::injected", None),
            ("percent %0A::warning::injected", None),
            ("\x1b[31mred\x1b[0m title", "red title"),
            ("zero​width ‮right-to-left\ttab", "zerowidth right-to-left tab"),
            ("only ✖�\x00\x07 symbols", "only symbols"),
            ("✖✖", None),
            ("word " * 60, ("word " * 24).rstrip()),
            ("unbroken " + "x" * 40, None),
            ("keeps punctuation: (a, b) <c> = d + e # f/g", "keeps punctuation: (a, b) <c> = d + e # f/g"),
            ("Fixture.test_plan_digest_matches_adapter_utf8_workspace_paths",
             "Fixture.test_plan_digest_matches_adapter_utf8_workspace_paths"),
        ]
        for raw, expected in cases:
            with self.subTest(raw=raw[:40]):
                self.assertEqual(failures.clean_name(raw), expected)
        self.assertEqual(failures.clean_file("/tmp/cma-codex-source/open_completion_native.test.cjs"),
                         "open_completion_native.test.cjs")
        for path in ("/tmp/x/" + token + ".test.cjs", "/tmp/x/has space.cjs", "/tmp/x/", "/tmp/x/" + "f" * 90,
                     "/tmp/x/" + "0123456789abcdef0.cjs", "/tmp/x/-flag.cjs", "/tmp/x/a\x1b.cjs"):
            with self.subTest(path=path[:40]):
                self.assertIsNone(failures.clean_file(path))

    def test_hostile_log_content_never_reaches_the_summary(self):
        token = "gh" + "p_" + "Ab1Cd2" * 6
        hostile = "TAP version 13\n" + "".join(f"not ok {n} - {title}\n  ---\n  location: '{location}:1:1'\n  ...\n" for n, (title, location) in
                          enumerate([("leaks " + token, "/tmp/x/a.test.cjs"),
                                     ("::error::injected", "/tmp/x/b.test.cjs"),
                                     ("clean title", "/tmp/x/" + token + ".cjs"),
                                     ("\x1b]8;;https://user:pass@example.invalid\x07link\x1b]8;;\x07", "/tmp/x/c.cjs"),
                                     ("first\r::warning::injected", "/tmp/x/d.cjs")], 1))
        hostile += "::error title=Image test failed::private injected line\n"
        hostile += "\x1b[2Knot ok 9 - ansi prefixed title\n  ---\n  ...\n"
        # A unittest-shaped line counts only between its own separators.
        stray = ("FAIL: test_x (a.B)\n" + "-" * 70 + "\nnoise\nERROR: test_y (a.B)\n" + "-" * 70 + "\n"
                 + "=" * 70 + "\nFAIL: test_z (a.B)\nnoise\nnoise\n" + "-" * 70 + "\n")
        result = failed_qualification(stdout=hostile, stderr=stray)
        self.assertEqual(result["failed_tests"], [
            {"step": STEP, "file": "a.test.cjs"}, {"step": STEP, "file": "b.test.cjs"},
            {"step": STEP, "test": "clean title"}, {"step": STEP, "file": "c.cjs", "test": "link"},
            {"step": STEP, "file": "d.cjs"}, {"step": STEP, "test": "ansi prefixed title"}])
        text = json.dumps(result)
        for leaked in (token, "injected", "user", "pass@", "\\u001b", "::", "%"):
            self.assertNotIn(leaked, text)

    def test_an_oversized_log_keeps_only_its_bounded_head_and_tail(self):
        # At most 1 MiB of one step's output is ever held: 512 KiB per stream.
        self.assertLessEqual(failures.HEAD + failures.TAIL, 512 * 1024)
        head = "TAP version 13\nnot ok 1 - early failure\n  ---\n  ...\n"
        middle = "not ok 2 - middle failure\n  ---\n  ...\n"
        tail = "not ok 3 - late failure\n  ---\n  ...\n"
        filler = ("private filler line\n" * (failures.HEAD // 20 + 1000))
        text = head + filler + middle + filler + tail
        result = failed_qualification(stdout=text.encode())
        self.assertEqual([row["test"] for row in result["failed_tests"]], ["early failure", "late failure"])
        retained = failures.Retained()
        for start in range(0, len(text), 4096):
            retained.add(text[start:start + 4096].encode())
        self.assertLessEqual(len(retained.head) + len(retained.tail), failures.HEAD + failures.TAIL)
        # A single chunk larger than both bounds is still cut to them.
        huge = failures.Retained()
        huge.add(b"x" * (failures.HEAD + failures.TAIL) * 3)
        self.assertEqual((len(huge.head), len(huge.tail)), (failures.HEAD, failures.TAIL))
        self.assertEqual(huge.text(), "")


def instructions(text):
    """Dockerfile instructions by stage: continuations joined, heredoc bodies kept."""
    stages, current, lines, index = {}, None, text.splitlines(), 0
    while index < len(lines):
        line = lines[index].strip()
        index += 1
        if not line or line.startswith("#"):
            continue
        while line.endswith("\\") and index < len(lines):
            line = line[:-1] + " " + lines[index].strip()
            index += 1
        for marker in re.findall(r"<<-?['\"]?([A-Za-z_][A-Za-z0-9_]*)['\"]?", line):
            while index < len(lines) and lines[index].strip() != marker:
                line += "\n" + lines[index]
                index += 1
            index += 1
        keyword = line.split(None, 1)[0].upper()
        if keyword == "FROM":
            named = re.search(r"\sAS\s+([A-Za-z0-9_.-]+)\s*$", line, re.IGNORECASE)
            current = named[1] if named else None
            if current:
                stages.setdefault(current, []).append([])
        elif current:
            stages[current][-1].append((keyword, line))
    return stages


class BindingTests(unittest.TestCase):
    def test_each_listed_stage_is_defined_once_in_its_bound_dockerfile_on_public_inputs(self):
        tracked = subprocess.run(["git", "-C", str(ROOT), "ls-files", "-z", "--", "*Dockerfile", "*.Dockerfile",
                                  "*Dockerfile.*"], capture_output=True, check=True).stdout.decode().split("\0")
        parsed = {path: instructions((ROOT / path).read_text()) for path in tracked if path}
        self.assertIn("api/crates/layer4-runtime/Dockerfile", parsed)
        # Build arguments the stage may declare itself: only BuildKit's public platform value.
        arguments = {"runtime-qualification": set(), "media-self-test": {"TARGETARCH"}}
        self.assertEqual(set(failures.STAGES), set(arguments))
        for stage, (dockerfile, _parsers) in failures.STAGES.items():
            with self.subTest(stage=stage):
                self.assertEqual([path for path, stages in parsed.items() if stage in stages], [dockerfile])
                self.assertEqual(len(parsed[dockerfile][stage]), 1, "defined exactly once")
                steps = parsed[dockerfile][stage][0]
                runs = [line for keyword, line in steps if keyword == "RUN"]
                self.assertTrue(runs)
                for line in runs:
                    self.assertNotRegex(line, r"type=(?:secret|ssh)\b")
                    self.assertNotRegex(line, r"--(?:secret|ssh)\b")
                declared = {re.split(r"[=\s]", line.split(None, 1)[1], maxsplit=1)[0]
                            for keyword, line in steps if keyword == "ARG"}
                self.assertEqual(declared, arguments[stage])
        media = parsed["api/crates/cmaeg/Dockerfile"]["media-self-test"][0]
        tests = [line for keyword, line in media if keyword == "RUN" and "--media-self-test" in line]
        self.assertEqual(len(tests), 1)
        self.assertTrue(tests[0].startswith("RUN --network=none "))

    def test_static_check_reads_continuations_heredocs_and_stage_reuse(self):
        stages = instructions("FROM a AS one\nARG X=1\nRUN --mount=type=secret,id=k \\\n  true\n"
                              "RUN <<'EOF'\nFROM b AS hidden\nEOF\nFROM one AS one\n# FROM c AS comment\n")
        self.assertEqual(list(stages), ["one"])
        self.assertEqual(len(stages["one"]), 2)
        self.assertEqual([keyword for keyword, _ in stages["one"][0]], ["ARG", "RUN", "RUN"])
        self.assertIn("type=secret", stages["one"][0][1][1])

    def test_the_build_binds_names_to_the_recipe_dockerfile(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            source.mkdir()
            staging_cd_build_tests.BuildTests().source(source)
            reservation = next_reservation("web", "a" * 40, "b" * 40, 42, "patch", 1, 1, [], [])
            stderr = progress((1, QUALIFICATION, "private", {1: SPEC}))
            for stages, expected in (({}, None), ({"runtime-qualification": ("web/Dockerfile", ("spec",))}, SPEC_NAMES)):
                with self.subTest(stages=stages), patch.dict(failures.STAGES, stages), \
                        patch("staging_cd_build.subprocess.run", return_value=Mock(returncode=1, stdout=b"", stderr=stderr)):
                    summary = {}
                    with self.assertRaises(ImageCommandRefusal):
                        build(root, reservation, root, source=source, progress=summary)
                    self.assertEqual(summary["failed_steps"], [STEP])
                    self.assertEqual(summary.get("failed_tests"), expected)


class AnnotationTests(unittest.TestCase):
    def test_annotations_print_only_valid_names_and_at_most_ten(self):
        rows = [{"step": STEP, "file": f"f{n}.test.cjs", "test": f"title {n}"} for n in range(12)]
        hostile = [{"step": STEP, "test": "x\n::warning::injected"}, {"step": "runtime-tools 1/2", "test": "x"},
                   {"step": STEP}, {"step": STEP, "test": "x", "error": "private"}, {"step": STEP, "test": 7},
                   "private", {"step": STEP + "\n::error::injected", "test": "x"}]
        output = io.StringIO()
        failures.annotate("runtime", {"failed_tests": hostile + rows}, output)
        lines = output.getvalue().splitlines()
        self.assertEqual(lines, [f"::error title=Image test failed::runtime {STEP}: f{n}.test.cjs › title {n}"
                                 for n in range(3)])
        output = io.StringIO()
        failures.annotate("runtime", {"failed_tests": rows + [{"step": STEP, "file": "only.cjs"}]}, output)
        self.assertEqual(len(output.getvalue().splitlines()), 10)
        for summary in ({}, {"failed_tests": "private"}, None, {"failed_tests": [{"step": STEP, "test": "x" * 121}]}):
            output = io.StringIO()
            failures.annotate("runtime", summary, output)
            self.assertEqual(output.getvalue(), "")
        output = io.StringIO()
        failures.annotate("eg", {"failed_tests": [{"step": "media-self-test 7/7", "file": "voice.ogg"}]}, output)
        self.assertEqual(output.getvalue(), "::error title=Image test failed::eg media-self-test 7/7: voice.ogg\n")

    def test_publication_annotates_test_names_only_when_it_stopped_in_that_build(self):
        identity = fixture_identity()
        reservation = {k: v for k, v in identity.items() if k != "build"}
        reservation["schema"] = "cma-version-reservation-v1"
        summary = failed_qualification(stdout=SPEC)
        expected = [f"::error title=Image test failed::web {STEP}: sample.test.cjs › "
                    "reopens the original conversation after restart",
                    f"::error title=Image test failed::web {STEP}: sample.test.cjs › "
                    "nested failure with <angle> quotes",
                    f"::error title=Image test failed::web {STEP}: crash.test.cjs",
                    f"::error title=Image test failed::web {STEP}: timeout.test.cjs › "
                    "session open overlap stays bounded"]
        # mode -> (build raises, publish raises, annotations expected)
        cases = {"stopped-in-build": (True, False, expected), "failed-after-build": (False, True, []),
                 "published": (False, False, [])}
        for mode, (build_fails, publish_fails, annotations) in cases.items():
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                env = {"GITHUB_EVENT_NAME": "push", "GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "1",
                       "RUNNER_TEMP": directory, "CD_COMPONENT": "web",
                       "CD_RECEIPTS_PATH": str(Path(directory) / "receipts.json"),
                       "CD_TASKS": json.dumps([{"component": "web", "mode": "publish", "reservation": reservation}])}

                def fake_build(*_args, progress, **_kwargs):
                    progress.update(summary)
                    if build_fails:
                        raise subprocess.CalledProcessError(1, "private build")
                    return Path(directory) / "image.tar", identity

                def fake_publish(*_args, diagnostics, **_kwargs):
                    if publish_fails:
                        diagnostics.mark("scan")
                        raise OSError("private")
                    return fixture_receipt()

                output = io.StringIO()
                with contextlib.ExitStack() as stack:
                    stack.enter_context(patch.dict(os.environ, env, clear=True))
                    for name in ("event_identity", "Registry", "GitHub", "Reservations", "snapshot"):
                        stack.enter_context(patch.object(publication, name))
                    stack.enter_context(patch.object(publication, "identity", return_value=(identity, {}, {})))
                    stack.enter_context(patch.object(publication, "try_reuse", return_value=None))
                    stack.enter_context(patch.object(publication, "build", side_effect=fake_build))
                    stack.enter_context(patch.object(publication, "publish", side_effect=fake_publish))
                    stack.enter_context(contextlib.redirect_stdout(output))
                    status = publication.main()
                text = output.getvalue()
                self.assertEqual(status, 1 if build_fails or publish_fails else 0)
                builds = [json.loads(line) for line in text.splitlines() if '"publication-build"' in line]
                self.assertEqual(builds, [{"event": "publication-build", **summary}])
                self.assertEqual([line for line in text.splitlines() if "Image test failed" in line], annotations)
                self.assertNotIn("private", text)


if __name__ == "__main__":
    unittest.main()

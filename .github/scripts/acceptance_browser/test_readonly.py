import copy
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "acceptance"))
from safety import Refusal
from evidence import Evidence
import artifacts
import binding
import readonly_browser as runner
from validation import validate, assess


def browser():
    return {
        "requests": [],
        "script_errors": 0,
        "blocked": dict.fromkeys(
            ("WRITE", "DESTINATION", "SOCKET", "BUDGET", "QUIET", "STOPPED"), 0
        ),
        "cases": [],
        "initialized": True,
        "actual_hidden": False,
        "navigation_return": False,
        "readiness_ms": None,
        "history": None,
        "continuity": None,
        "browser_version": "1.2.3.4",
        "observation_complete": True,
    }


class PrivacyTests(unittest.TestCase):
    def test_unknown_fields_strings_and_raw_identifiers_refused(self):
        value = browser()
        self.assertEqual(validate(value), value)
        for key, content in (
            ("raw_url", "https://example.test"),
            ("browser_version", "private identity"),
        ):
            bad = copy.deepcopy(value)
            bad[key] = content
            with self.assertRaises(Refusal):
                validate(bad)

    def test_unobserved_cases_remain_in_denominator(self):
        result = assess(browser(), False)
        self.assertEqual(len(result), 7)
        self.assertEqual([row["result"] for row in result], ["NOT OBSERVED"] * 7)

    def test_failed_response_not_erased_by_later_success(self):
        value = browser()
        value["cases"] = [
            {"phase": "A3", "start_ms": 0, "window_start_ms": 0, "end_ms": 60000}
        ]
        value["requests"] = [
            {"phase": "A3", "duplicate": False, "status": status}
            for status in (429, 200)
        ]
        self.assertEqual(assess(value, True)[2]["result"], "FAIL")

    def test_output_preflight_precedes_any_run(self):
        with tempfile.TemporaryDirectory() as directory:
            os.chmod(directory, 0o755)
            with patch.object(
                sys,
                "argv",
                ["runner", "public", "--output", directory + "/result.json"],
            ):
                with patch.object(runner, "run") as run:
                    with self.assertRaises(Refusal):
                        runner.main()
                    run.assert_not_called()

    def test_symlink_output_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            link = Path(directory) / "link"
            link.symlink_to(directory, target_is_directory=True)
            with self.assertRaises(Refusal):
                with Evidence(link / "result.json"):
                    self.fail("symlink accepted")

    def test_missing_fixture_never_creates_work(self):
        class Probe:
            state = {"fixtures": {}}

        with patch("permanent_fixture.load", return_value=None):
            with self.assertRaisesRegex(Refusal, "existing_dedicated_agent_required"):
                binding.existing_fixture(Probe())

    def test_rejected_identity_prevents_browser_spawn(self):
        with patch.object(runner, "bind_browser", side_effect=Refusal("rejected")):
            with patch.object(runner.subprocess, "Popen") as launch:
                with self.assertRaises(Refusal):
                    runner.browser_run("agent", object())
                launch.assert_not_called()


class ArtifactTests(unittest.TestCase):
    def test_failed_http_status_survives_refusal(self):
        http = artifacts.AuditedHttp()
        with patch("safety.Http.request", return_value=(503, {}, b"unavailable", 0.1)):
            with self.assertRaisesRegex(Refusal, "build_info_unavailable"):
                artifacts.attest(http)
        self.assertEqual(http.reads[0]["status"], 503)
        self.assertNotIn("unavailable", json.dumps(http.reads))

    def test_asset_destinations_are_canonical_and_bounded(self):
        self.assertEqual(
            artifacts.asset_path("./next.js", "/assets/entry.js"), "/assets/next.js"
        )
        for value in (
            "https://example.test/asset.js",
            "//example.test/assets/a.js",
            "/assets/../auth/login",
            "/assets/a.js?key=value",
        ):
            with self.assertRaises(Refusal):
                artifacts.asset_path(value)

    def test_transitive_cycle_and_hash_only_output(self):
        build = json.dumps(
            {"revision": "a" * 40, "version": "git-" + "a" * 12, "local": False}
        ).encode()
        payloads = {
            "/build-info.json": build,
            "/": b'<script type="module" src="/assets/main.js"></script>',
            "/assets/main.js": b'import "./next.js"',
            "/assets/next.js": b'import "./main.js"',
        }

        class Http:
            def request(self, _origin, path, **_kwargs):
                return 200, {}, payloads[path], 0.1

        result = artifacts.attest(
            Http(),
            extractor=lambda source: (
                ["./next.js"] if "next.js" in source else ["./main.js"]
            ),
        )
        self.assertEqual(len(result["assets"]), 2)
        self.assertNotIn("main.js", json.dumps(result))
        self.assertNotIn("import", json.dumps(result))

    def test_mixed_artifacts_refused(self):
        build = {"revision": "a" * 40, "version": "git-" + "a" * 12, "local": False}

        class Http:
            count = 0

            def request(self, _origin, path, **_kwargs):
                if path == "/build-info.json":
                    self.count += 1
                    value = dict(build, revision=("a" if self.count == 1 else "b") * 40)
                    return 200, {}, json.dumps(value).encode(), 0
                return (
                    200,
                    {},
                    b'<script src="/assets/main.js"></script>' if path == "/" else b"",
                    0,
                )

        with self.assertRaisesRegex(Refusal, "artifact_changed"):
            artifacts.attest(Http(), extractor=lambda _: [])


if __name__ == "__main__":
    unittest.main()

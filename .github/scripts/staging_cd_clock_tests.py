"""Dispatch queue time, cold setup and retries all consume the original clock."""
import re
import unittest
from pathlib import Path
from unittest.mock import Mock, patch

import yaml

from staging_cd_clock import (ADMISSION_SECONDS, BOOTSTRAP_ADMISSION_SECONDS, BOOTSTRAP_TOTAL_SECONDS,
                              PRODUCTION_WATCHDOG_SECONDS, STAGING_AM_RELEASE_TOTAL_SECONDS,
                              TERMINATION_GRACE_SECONDS, WATCHDOG_SECONDS, controller_seconds, dispatch_started,
                              elapsed, remaining, timestamp)
from staging_cd_common import Refusal


class ClockTests(unittest.TestCase):
    def test_single_absolute_clock_cannot_reset_at_phase_boundaries(self):
        started = timestamp(1000)
        self.assertEqual(elapsed(started, 1020), 20)
        for age in (47, 62, 110):
            self.assertEqual(remaining(started, now=1000 + age), 120 - age)
        self.assertEqual(remaining(started, mode="bootstrap", now=1020), 25)
        with self.assertRaises(Refusal):
            remaining(started, mode="bootstrap", now=1045)
        for mode, limit in (("bootstrap", 600), ("release", 990)):
            with self.subTest(mode=mode):
                self.assertEqual(controller_seconds(mode), limit)
                self.assertEqual(remaining(started, limit, now=1000 + limit - 5), 5)
                with self.assertRaises(Refusal):
                    remaining(started, limit, now=1000 + limit)
        self.assertEqual(WATCHDOG_SECONDS, 1005)
        # Explicit staging is the default; only a production release changes its total.
        for mode, staging, production in (("bootstrap", 600, 600), ("release", 990, 3600)):
            with self.subTest(mode=mode):
                self.assertEqual(controller_seconds(mode, "staging"), staging)
                self.assertEqual(controller_seconds(mode, "production"), production)
        self.assertEqual(PRODUCTION_WATCHDOG_SECONDS, 3615)
        self.assertEqual(remaining(started, controller_seconds("release", "production"), now=4595), 5)
        for mode, target in (("release", "prod"), ("release", None), ("deploy", "production")):
            with self.subTest(mode=mode, target=target), self.assertRaises(Refusal):
                controller_seconds(mode, target)
        for value, now in ((timestamp(1001), 1000), (started, 1120), (started, 1125),
                           ("1000", 1000), ("2026-01-01T00:00:00", 1000),
                           ("2026-99-99T00:00:00Z", 1000), (None, 1000)):
            with self.subTest(value=value), self.assertRaises(ValueError):
                remaining(value, now=now)

    def test_dispatch_run_created_timestamp_is_api_bound_and_retries_do_not_refresh_it(self):
        env = {"GITHUB_RUN_ID": "123", "GITHUB_SHA": "a" * 40, "GITHUB_RUN_ATTEMPT": "2",
               "GITHUB_EVENT_NAME": "workflow_dispatch"}
        run = {"id": 123, "event": "workflow_dispatch", "head_branch": "develop", "head_sha": "a" * 40,
               "path": ".github/workflows/redeploy-staging.yml", "created_at": timestamp(1000),
               "run_started_at": timestamp(1040), "repository": {"full_name": "ChronoAIProject/cma"}}
        api = Mock(return_value=run)
        with patch("staging_cd_clock.time.time", return_value=1020):
            self.assertEqual(dispatch_started(env, api), timestamp(1000))
            api.assert_called_with("actions/runs/123")
            for key, value in (("id", 124), ("event", "push"), ("head_branch", "main"),
                               ("head_sha", "b" * 40), ("path", ".github/workflows/other.yml"),
                               ("repository", {"full_name": "Example/fork"})):
                with self.subTest(key=key), self.assertRaises(Refusal):
                    dispatch_started(env, Mock(return_value=dict(run, **{key: value})))
        with patch("staging_cd_clock.time.time", return_value=1239):
            self.assertEqual(dispatch_started(env, api), timestamp(1000))
        with patch("staging_cd_clock.time.time", return_value=1240), self.assertRaises(Refusal):
            dispatch_started(env, api)

    def test_extra_time_is_only_for_staging_release_dispatch(self):
        started = timestamp(1000)
        staging = {"GITHUB_EVENT_NAME": "workflow_dispatch"}
        self.assertEqual(remaining(started, now=1121, env=staging), 119)
        for env in (dict(staging, CD_TARGET="production"), {"GITHUB_EVENT_NAME": "push"}):
            with self.subTest(env=env), self.assertRaises(Refusal):
                remaining(started, now=1120, env=env)
        with self.assertRaises(Refusal):
            remaining(started, now=1045, mode="bootstrap", env=staging)

    def test_each_deploy_job_outlives_its_release_watchdog(self):
        # Setup before the clock starts and cleanup after it need at least one
        # admission window beyond the watchdog, so a total cannot outgrow its job.
        workflows = Path(__file__).resolve().parent.parent / "workflows"
        staging = yaml.safe_load((workflows / "staging-deploy.yml").read_text(encoding="utf-8"))["jobs"]["deploy"]
        timeout = re.fullmatch(r"\$\{\{ inputs\.mode == 'release' && (\d+) \|\| (\d+) \}\}",
                               staging["timeout-minutes"])
        self.assertIsNotNone(timeout)
        release_minutes, bootstrap_minutes = map(int, timeout.groups())
        watchdog = STAGING_AM_RELEASE_TOTAL_SECONDS + TERMINATION_GRACE_SECONDS
        self.assertGreater(release_minutes * 60, max(watchdog, WATCHDOG_SECONDS) + 240)
        self.assertGreater(bootstrap_minutes * 60,
                           BOOTSTRAP_TOTAL_SECONDS + TERMINATION_GRACE_SECONDS + BOOTSTRAP_ADMISSION_SECONDS)
        production = yaml.safe_load((workflows / "prod-deploy.yml").read_text(encoding="utf-8"))["jobs"]["deploy"]
        self.assertGreater(production["timeout-minutes"] * 60, PRODUCTION_WATCHDOG_SECONDS + ADMISSION_SECONDS)


if __name__ == "__main__":
    unittest.main()

"""Dispatch queue time, cold setup and retries all consume the original clock."""
import unittest
from unittest.mock import Mock, patch

from staging_cd_clock import (WATCHDOG_SECONDS, controller_seconds, dispatch_started, elapsed, remaining, timestamp)
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
        with patch("staging_cd_clock.time.time", return_value=1125), self.assertRaises(Refusal):
            dispatch_started(env, api)


if __name__ == "__main__":
    unittest.main()

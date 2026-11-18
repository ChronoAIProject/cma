"""Real insert races, burned failures and final-tag separation regressions."""
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor

from staging_cd_common import Refusal
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_reservations import next_reservation, pending_name, reserve


def allocate(finals, pending, source="a" * 40, run_id=1, level="patch"):
    return next_reservation("web", source, "b" * 40, 42, level, run_id, 1, finals, pending)


class ReservationTests(unittest.TestCase):
    def test_two_concurrent_allocations_get_distinct_versions(self):
        pending, barrier, lock = [], threading.Barrier(2), threading.Lock()
        observed = threading.local()
        def refresh():
            with lock:
                snapshot = list(pending)
            if not getattr(observed, "first", False):
                observed.first = True
                barrier.wait(timeout=5)
            return [], snapshot
        def insert(value):
            with lock:
                if any(item["version"] == value["version"] for item in pending):
                    return False
                pending.append(value)
                return True
        with ThreadPoolExecutor(max_workers=2) as pool:
            tasks = [pool.submit(reserve, allocate, insert, refresh) for _ in range(2)]
            values = [task.result(timeout=10)["version"] for task in tasks]
        self.assertEqual(sorted(values), ["0.10.1", "0.10.2"])

    def test_failed_push_burns_reserved_version_and_uses_max_of_both_histories(self):
        burned = allocate([], [])
        after_failure = allocate([], [burned])
        self.assertEqual(after_failure["version"], "0.10.2")
        self.assertEqual(after_failure["previous"], "0.10.1")
        final = fixture_tag(fixture_receipt(previous="0.10.9", version="0.10.10"))
        self.assertEqual(allocate([final], [burned])["version"], "0.10.11")
        self.assertEqual(allocate([], [burned], level="minor")["version"], "0.11.0")
        self.assertEqual(pending_name(burned), "cma-web/pending/v0.10.1")

    def test_exhausted_conflicts_fail_closed(self):
        with self.assertRaises(Refusal):
            reserve(allocate, lambda _: False, lambda: ([], []), attempts=2)


if __name__ == "__main__":
    unittest.main()

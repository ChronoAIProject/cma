"""Real insert races, burned failures and final-tag separation regressions."""
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from unittest.mock import Mock

from staging_cd_common import Refusal
from staging_cd_history import DevelopHistory
from staging_cd_receipts_tests import fixture_receipt, fixture_tag
from staging_cd_reservations import ReservationSuperseded, Reservations, next_reservation, pending_name, reserve


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

    def test_queued_reservation_pruned_below_a_same_or_later_final_still_yields(self):
        oldest, queued_source, final_source = "f" * 40, "a" * 40, "b" * 40
        api = Mock()
        api.history.return_value = DevelopHistory("c" * 40, ["c" * 40, final_source, queued_source, oldest])
        store = Reservations(api, "fixture", Path.cwd())
        store.list = lambda _component=None: []
        queued = allocate([], [], source=queued_source)
        def final(source, version):
            previous = "0.10." + str(int(version.rsplit(".", 1)[1]) - 1)
            return fixture_tag(fixture_receipt(source=source, previous=previous, version=version))
        for source in (queued_source, final_source):
            api.tags.return_value = [final(source, "0.10.2")]
            with self.subTest(final=source), self.assertRaises(ReservationSuperseded):
                store.require_current(queued, build_started=False)
        # A started build, an older or lower final, or no final never excuses an absent reservation.
        for finals, started in (([final(final_source, "0.10.2")], True), ([final(oldest, "0.10.2")], False),
                                ([final(final_source, "0.10.1")], False), ([], False)):
            api.tags.return_value = finals
            with self.subTest(finals=finals, started=started), self.assertRaisesRegex(Refusal, "missing") as error:
                store.require_current(queued, build_started=started)
            self.assertNotIsInstance(error.exception, ReservationSuperseded)


if __name__ == "__main__":
    unittest.main()

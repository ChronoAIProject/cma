#!/usr/bin/env python3
"""Exercise ordered retirement execution against a controlled boundary."""

from pathlib import Path
import tempfile
import unittest

from sandbox_retirement_execution import Admission, Eligibility, Observation, execute
from sandbox_retirement_journal import RetirementJournal


GUARD = "f" * 64


class Approval:
    def __call__(self):
        return True

    def before_send(self):
        return True


class Boundary:
    def __init__(self):
        self.calls = []
        self.ready = set()
        self.admission = Admission.UNKNOWN

    def preflight(self, member):
        self.calls.append(("preflight", member["member_sha256"]))
        return member["eligibility"], GUARD

    def dispatch(self, member, before_send):
        if not before_send():
            return Admission.UNSENT, None
        self.calls.append(("dispatch", member["member_sha256"]))
        return self.admission, None

    def observe(self, member, *, guard_sha256):
        identity = member["member_sha256"]
        self.calls.append(("observe", identity, guard_sha256))
        return Observation(GUARD) if identity in self.ready else None


class ExecutionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="sandbox-retirement-execution-")
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name) / "journal.jsonl"
        self.first = "a" * 64
        self.second = "b" * 64
        self.members = [
            {"member_sha256": self.first, "eligibility": Eligibility.ELIGIBLE},
            {"member_sha256": self.second, "eligibility": Eligibility.EXISTING},
        ]
        self.boundary = Boundary()

    def journal(self):
        return RetirementJournal(self.path, "c" * 64, "d" * 64)

    def test_unresolved_attempt_blocks_following_member_and_reopen_only_reads(self):
        with self.journal() as journal:
            self.assertFalse(execute(self.members, journal, self.boundary, Approval()))
        self.assertEqual(
            self.boundary.calls,
            [("preflight", self.first), ("dispatch", self.first), ("observe", self.first, GUARD)],
        )
        self.boundary.calls.clear()
        self.boundary.ready.update({self.first, self.second})
        with self.journal() as journal:
            self.assertTrue(execute(self.members, journal, self.boundary, None))
            self.assertEqual(journal.next_index, 2)
        self.assertEqual(
            self.boundary.calls,
            [("observe", self.first, GUARD), ("preflight", self.second), ("observe", self.second, None)],
        )

    def test_expired_approval_prevents_new_intent(self):
        with self.journal() as journal:
            self.assertFalse(execute(self.members, journal, self.boundary, None))
            self.assertIsNone(journal.pending)
            self.assertEqual(journal.next_index, 0)
        self.assertEqual(self.boundary.calls, [("preflight", self.first)])

    def test_acknowledged_existing_retirement_can_finish_by_reads_after_reopen(self):
        self.boundary.admission = Admission.EXISTING
        with self.journal() as journal:
            self.assertFalse(execute(self.members[:1], journal, self.boundary, Approval()))
            self.assertTrue(journal.pending.existing)
        self.boundary.calls.clear()
        self.boundary.ready.add(self.first)
        with self.journal() as journal:
            self.assertTrue(execute(self.members[:1], journal, self.boundary, None))
            self.assertEqual(journal.next_index, 1)
        self.assertEqual(self.boundary.calls, [("observe", self.first, None)])


if __name__ == "__main__":
    unittest.main()

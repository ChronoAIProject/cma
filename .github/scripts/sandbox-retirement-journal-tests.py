#!/usr/bin/env python3
"""Exercise private retirement intent through interruption and reopen."""

import os
from pathlib import Path
import tempfile
import unittest

from sandbox_retirement_journal import Refused, RetirementJournal


REVIEW = "a" * 64
INVENTORY = "b" * 64
FIRST = "c" * 64
SECOND = "d" * 64
EVIDENCE = "e" * 64
GUARD = "f" * 64


class JournalTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="sandbox-retirement-journal-")
        self.addCleanup(temporary.cleanup)
        self.path = Path(temporary.name) / "journal.jsonl"

    def journal(self, review=REVIEW, inventory=INVENTORY):
        return RetirementJournal(self.path, review, inventory)

    def test_reopen_requires_observation_and_preserves_order(self):
        with self.journal() as journal:
            self.assertTrue(journal.reserve(0, FIRST, GUARD))

        with self.journal() as journal:
            self.assertFalse(journal.reserve(0, FIRST, GUARD))
            with self.assertRaises(Refused):
                journal.reserve(1, SECOND, GUARD)
            with self.assertRaises(Refused):
                journal.reserve(0, FIRST, EVIDENCE)
            with self.assertRaises(Refused):
                journal.complete(0, FIRST, "skipped", EVIDENCE)
            journal.complete(0, FIRST, "retired", EVIDENCE)
            journal.complete(1, SECOND, "existing_retired", EVIDENCE)

        with self.journal() as journal:
            self.assertEqual(journal.next_index, 2)
            self.assertIsNone(journal.pending)
            self.assertEqual(self.path.stat().st_mode & 0o777, 0o600)

    def test_refuses_a_competing_executor_and_different_review(self):
        with self.journal() as journal:
            self.assertTrue(journal.reserve(0, FIRST, GUARD))
            with self.assertRaisesRegex(Refused, "already open"), self.journal():
                self.fail("second executor acquired the journal")
        with self.assertRaisesRegex(Refused, "different review"), self.journal(review=SECOND):
            self.fail("different review acquired the journal")

    def test_refuses_an_incomplete_write_and_leaves_existing_intent(self):
        with self.journal() as journal:
            self.assertTrue(journal.reserve(0, FIRST, GUARD))
        original = self.path.read_bytes()
        with self.path.open("ab") as stream:
            stream.write(b'{"index":0')
            stream.flush()
            os.fsync(stream.fileno())
        with self.assertRaisesRegex(Refused, "incomplete write"), self.journal():
            self.fail("a broken journal reopened")
        self.assertEqual(self.path.read_bytes(), original + b'{"index":0')


if __name__ == "__main__":
    unittest.main()

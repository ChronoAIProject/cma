"""Exercise the real read-only SQLite probe against retained native schemas."""
import importlib.util
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("transport_goal.py")
spec = importlib.util.spec_from_file_location("transport_goal", SCRIPT)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)
SESSION = "01a19b73-5321-7600-90a6-1ba3fd73e5ad"


class GoalProof(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        prefix = patch.object(probe, "HOME_PREFIX", str(self.root.parent) + "/")
        prefix.start()
        self.addCleanup(prefix.stop)
        self.execute("state_5.sqlite", "CREATE TABLE threads(id TEXT PRIMARY KEY)")
        self.execute("state_5.sqlite", "INSERT INTO threads VALUES(?)", (SESSION,))
        self.execute("goals_1.sqlite", "CREATE TABLE thread_goals(thread_id TEXT PRIMARY KEY, status TEXT)")

    def execute(self, name, sql, args=()):
        db = sqlite3.connect(self.root / name)
        try:
            db.execute(sql, args)
            db.commit()
        finally:
            db.close()

    def inspect(self, session=SESSION):
        return probe.inspect(str(self.root), session)

    def test_absence_requires_exact_existing_thread_and_preserves_bytes(self):
        before = {p.name: p.read_bytes() for p in self.root.iterdir()}
        self.assertTrue(self.inspect())
        self.assertFalse(self.inspect("01a19b73-5321-7600-90a6-1ba3fd73e5ae"))
        self.assertEqual(before, {p.name: p.read_bytes() for p in self.root.iterdir()})

    def test_any_goal_status_defers_even_completed_or_unknown(self):
        for status in ("active", "paused", "blocked", "usage_limited", "budget_limited", "complete", "future"):
            with self.subTest(status=status):
                self.execute("goals_1.sqlite", "INSERT OR REPLACE INTO thread_goals VALUES(?,?)", (SESSION, status))
                self.assertFalse(self.inspect())

    def test_other_thread_goal_does_not_block_exact_thread(self):
        self.execute("goals_1.sqlite", "INSERT INTO thread_goals VALUES('other','active')")
        self.assertTrue(self.inspect())

    def test_missing_database_or_schema_is_not_absence_proof(self):
        (self.root / "goals_1.sqlite").unlink()
        with self.assertRaises(FileNotFoundError):
            self.inspect()
        self.assertFalse((self.root / "goals_1.sqlite").exists())
        self.execute("goals_1.sqlite", "CREATE TABLE thread_goals(thread_id TEXT, status TEXT)")
        with self.assertRaises(ValueError):
            self.inspect()

    def test_symlink_database_home_and_invalid_session_are_refused(self):
        original = self.root / "goals_1.sqlite"
        original.rename(self.root / "goals-target.sqlite")
        original.symlink_to(self.root / "goals-target.sqlite")
        with self.assertRaises(ValueError):
            self.inspect()
        linked_home = self.root / "linked-home"
        linked_home.symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(ValueError):
            probe.inspect(str(linked_home), SESSION)
        with self.assertRaises(ValueError):
            self.inspect("../other-thread")

    def test_live_wal_goal_is_seen_without_checkpoint_or_database_mutation(self):
        db = sqlite3.connect(self.root / "goals_1.sqlite")
        self.addCleanup(db.close)
        db.execute("PRAGMA journal_mode=WAL")
        db.execute("INSERT INTO thread_goals VALUES(?, 'active')", (SESSION,))
        db.commit()
        names = ["state_5.sqlite", "goals_1.sqlite", "goals_1.sqlite-wal"]
        before = {name: (self.root / name).read_bytes() for name in names}
        self.assertFalse(self.inspect())
        self.assertEqual(before, {name: (self.root / name).read_bytes() for name in names})

    def test_production_entrypoint_rejects_non_home_and_malformed_requests(self):
        for request in ('{"home":"' + str(self.root) + '","session":"' + SESSION + '"}', '{}', 'invalid'):
            result = subprocess.run([sys.executable, "-I", str(SCRIPT)], input=request, text=True, capture_output=True, check=True)
            self.assertIn('"goal_absent": false', result.stdout)
            self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()

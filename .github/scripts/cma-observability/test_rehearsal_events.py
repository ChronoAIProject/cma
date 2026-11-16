import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from hook_events import classify
from package_watcher import package
from rehearsal_events import verify_namespace


class RehearsalTests(unittest.TestCase):
    def test_backoff_reason_variants_and_warning_message(self):
        for reason in ("BackOff", "Backoff", "ImagePullBackOff", "CrashLoopBackOff"):
            self.assertEqual(classify({"reason": reason}), "backoff")
        self.assertEqual(classify({"type": "Warning", "reason": "Other",
                                   "message": "Back-off restarting failed container"}), "backoff")
        self.assertIsNone(classify({"type": "Normal", "reason": "Started",
                                   "message": "backoff counter reset"}))
        self.assertEqual(classify({"reason": "DrainHookFailed", "message": "backoff"}), "drain")

    def test_namespace_identity_and_deletion_fail_closed(self):
        def fake(command, **kwargs):
            self.assertEqual(command, ["kubectl", "--context", "kind-test", "get", "namespace",
                                      "local", "--request-timeout=5s", "-o", "json"])
            kwargs["stdout"].write(json.dumps(self.row).encode())
            return type("Result", (), {"returncode": 0})()
        self.row = {"kind": "Namespace", "metadata": {"name": "local", "uid": "original"}}
        with patch("rehearsal_events.subprocess.run", fake):
            verify_namespace("kubectl", "kind-test", "local", "original")
            for mutation in ({"uid": "replacement"}, {"deletionTimestamp": "now"}):
                self.row["metadata"] = {"name": "local", "uid": "original", **mutation}
                with self.assertRaisesRegex(ValueError, "events_namespace_identity_changed"):
                    verify_namespace("kubectl", "kind-test", "local", "original")

    def test_zipapp_runs_outside_checkout_and_flushes_each_detection(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            app = root / "watch.pyz"
            first = package(app)
            self.assertEqual(first, package(root / "again.pyz"))
            fake = root / "kubectl"
            fake.write_text('''#!/usr/bin/env python3
import datetime,json,pathlib,sys
root=pathlib.Path(__file__).parent
with (root/'commands.jsonl').open('a') as f:f.write(json.dumps(sys.argv[1:])+'\\n')
if 'namespace' in sys.argv:
 print(json.dumps({'kind':'Namespace','metadata':{'name':'local','uid':'original'}}))
else:
 p=root/'cycle';n=int(p.read_text())+1 if p.exists() else 1;p.write_text(str(n))
 now=datetime.datetime.now(datetime.timezone.utc).isoformat()
 def event(reason):
  return {'metadata':{'uid':reason,'namespace':'local'},'involvedObject':{'kind':'Pod','namespace':'local','name':'am-matrix-old','uid':'old-pod'},'reason':reason,'type':'Warning','count':1,'lastTimestamp':now,'message':'private hook command'}
 rows=[event('FailedPreStopHook')] if n==1 else [event('BackOff'),event('DrainHookFailed')]
 print(json.dumps({'kind':'EventList','items':rows}))
''')
            fake.chmod(0o700)
            command = [sys.executable, str(app), "--context", "kind-test", "--namespace-uid", "original",
                       "--target", "am:local:am-matrix-", "--output", str(root / "out"),
                       "--kubectl", str(fake), "--duration", "1", "--interval", "1"]
            with subprocess.Popen(command, cwd=root, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True) as proc:
                try:
                    self.assertTrue(select.select([proc.stdout], [], [], 8)[0])
                    first_line = proc.stdout.readline()
                    self.assertEqual(json.loads(first_line)["category"], "pre_stop")
                    self.assertIsNone(proc.poll(), "first detection must be flushed before exit")
                    rest, error = proc.communicate(timeout=8)
                    self.assertEqual(proc.returncode, 1, error)
                finally:
                    if proc.poll() is None:
                        proc.kill()
                        proc.wait()
            self.assertEqual({json.loads(l)["category"] for l in rest.splitlines()}, {"backoff", "drain"})
            self.assertNotIn("private hook command", first_line + rest)
            state = json.loads((root / "out/status.json").read_text())
            self.assertEqual(state["namespace_reads_ok"], {"local": 1})
            self.assertFalse(state["coverage_gap_observed"])
            commands = [json.loads(l) for l in (root / "commands.jsonl").read_text().splitlines()]
            self.assertEqual(len(commands), 4)
            self.assertTrue(all("get" in c and ("namespace" in c or "events" in c) for c in commands))
            # Same CLI, wrong namespace UID: never reaches the Events read.
            command[command.index("original")] = "replacement"
            command[command.index(str(root / "out"))] = str(root / "refusal")
            result = subprocess.run(command + ["--once"], cwd=root, capture_output=True, text=True, timeout=8)
            self.assertEqual(result.returncode, 2)
            self.assertIn("events_namespace_identity_changed", result.stdout)
            self.assertEqual(len((root / "commands.jsonl").read_text().splitlines()), 5)
            self.assertEqual(os.stat(root / "out/alerts.jsonl").st_mode & 0o777, 0o600)


if __name__ == "__main__":
    unittest.main()

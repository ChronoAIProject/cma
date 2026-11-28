from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
from threading import Event, Thread
import time
import unittest
from urllib.parse import parse_qs, urlsplit

from staging_cd_common import Refusal
from staging_cd_receipts import sha256
from staging_cd_web_continuity import REPOSITORY
from staging_cd_web_overlay import catalog_bytes
from staging_cd_web_prepare_availability import watch_prepare
from staging_cd_web_rollout_preflight import read


STOCK_IMAGE = REPOSITORY + "@sha256:" + "a" * 64
PREPARE_IMAGE = REPOSITORY + "@sha256:" + "b" * 64
STOCK = {"entry-old.js": b"old entry", "chunk-old.js": b"old lazy chunk"}
PREPARE = STOCK | {"chunk-new.js": b"new lazy chunk"}


class WebPrepareAvailabilityTests(unittest.TestCase):
    def setUp(self):
        self.serve = self.serve_stock
        requests = self.requests = []

        class Handler(BaseHTTPRequestHandler):
            def do_GET(handler):
                url = urlsplit(handler.path)
                status, body = self.serve(url.path.removeprefix("/assets/"))
                requests.append((url.path, parse_qs(url.query), status))
                handler.send_response(status)
                handler.send_header("Content-Length", str(len(body)))
                handler.end_headers()
                handler.wfile.write(body)

            def log_message(self, *_):
                pass

        class Server(ThreadingHTTPServer):
            request_queue_size = 32

        server = Server(("127.0.0.1", 0), Handler)
        service = Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
        service.start()
        self.addCleanup(server.server_close)
        self.addCleanup(service.join)
        self.addCleanup(server.shutdown)
        self.origin = f"http://127.0.0.1:{server.server_port}"
        self.stock = json.loads(catalog_bytes(STOCK))
        self.prepare = json.loads(catalog_bytes(PREPARE))
        self.rows = []

    @staticmethod
    def serve_stock(name):
        return (200, STOCK[name]) if name in STOCK else (404, b"missing asset")

    @staticmethod
    def serve_prepare(name):
        return 200, PREPARE[name]

    def watch(self, rollout, *, record=None, prepare_sha256=None, fetch=read):
        watch_prepare(
            self.origin, self.stock, self.prepare, incumbent_sha256=sha256(self.stock),
            prepare_sha256=prepare_sha256 or sha256(self.prepare),
            incumbent_image=STOCK_IMAGE, prepare_image=PREPARE_IMAGE,
            deadline=time.monotonic() + 10, rollout=rollout, record=record or self.rows.append, fetch=fetch,
        )

    def test_full_catalog_uses_same_edge_and_defers_candidate_only_paths_until_settled(self):
        stock = {f"incumbent-{index}.js": f"export default {index};".encode() for index in range(136)}
        prepare = stock | {f"candidate-{index}.js": f"export default {index};".encode() for index in range(151)}
        self.stock = json.loads(catalog_bytes(stock))
        self.prepare = json.loads(catalog_bytes(prepare))
        self.serve = lambda name: (200, stock[name]) if name in stock else (404, b"missing asset")
        sampled = Event()
        submit_ns = None

        def record(row):
            self.rows.append(row)
            if row.get("phase") == "overlap" and submit_ns is not None and row["request_started_ns"] >= submit_ns:
                sampled.set()

        def rollout():
            nonlocal submit_ns
            submit_ns = time.time_ns()
            self.assertTrue(sampled.wait(2))
            self.serve = lambda name: (200, prepare[name])

        self.watch(rollout, record=record)
        assets = [row for row in self.rows if row["type"] == "asset"]
        self.assertEqual({row["phase"] for row in assets if row["path"].startswith("/assets/candidate-")}, {"prepare"})
        for phase, expected in (("stock", stock), ("overlap", stock), ("prepare", prepare)):
            self.assertEqual({row["path"] for row in assets if row["phase"] == phase},
                             {"/assets/" + name for name in expected})
        self.assertEqual(len(self.requests), len({row["marker"] for row in assets}))
        self.assertTrue(all(query.get("cma_web_preflight") for _, query, _ in self.requests))
        self.assertTrue(all(row["body_sha256"] == row["expected_sha256"] for row in assets))
        self.assertTrue(self.rows[-1]["passed"])
        self.assertGreater(self.rows[-1]["in_window_starts"], 0)

    def test_requests_delayed_past_settle_cannot_qualify_overlap(self):
        settled = Event()
        stock_reads = 0

        def fetch(origin, path, **kwargs):
            nonlocal stock_reads
            stock_reads += 1
            if stock_reads > len(STOCK):
                self.assertTrue(settled.wait(2))
            return read(origin, path, **kwargs)

        def rollout():
            submit_ns = time.time_ns()
            self.serve = self.serve_prepare
            settle_ns = time.time_ns()
            settled.set()
            return submit_ns, settle_ns

        with self.assertRaisesRegex(Refusal, "had no requests started during the rollout"):
            self.watch(rollout, fetch=fetch)
        result = self.rows[-1]
        self.assertFalse(result["passed"])
        self.assertEqual(result["in_window_starts"], 0)
        self.assertEqual(result["phases"]["prepare"]["failures"], 0)
        self.assertGreater(result["phases"]["overlap"]["attempts"], 0)

    def test_in_window_request_can_finish_after_settle(self):
        submitted, in_flight, settled = Event(), Event(), Event()
        stock_reads = 0

        def fetch(origin, path, **kwargs):
            nonlocal stock_reads
            stock_reads += 1
            if stock_reads > len(STOCK):
                self.assertTrue(submitted.wait(2))
            return read(origin, path, **kwargs)

        def serve_during_rollout(name):
            in_flight.set()
            self.assertTrue(settled.wait(2))
            return self.serve_prepare(name)

        def rollout():
            self.serve = serve_during_rollout
            submit_ns = time.time_ns()
            submitted.set()
            self.assertTrue(in_flight.wait(2))
            settle_ns = time.time_ns()
            settled.set()
            return submit_ns, settle_ns

        self.watch(rollout, fetch=fetch)
        self.assertTrue(self.rows[-1]["passed"])
        self.assertGreater(self.rows[-1]["in_window_starts"], 0)

    def test_overlap_404_and_503_remain_failures_after_the_same_paths_recover(self):
        observed_failures, observed_recovery = Event(), Event()
        failures, recovered = set(), set()

        def record(row):
            self.rows.append(row)
            if row.get("phase") != "overlap":
                return
            path = row["path"]
            if row["status"] in (404, 503):
                failures.add(path)
                if len(failures) == len(STOCK):
                    observed_failures.set()
            if path in failures and row["status"] == 200:
                recovered.add(path)
                if recovered == failures:
                    observed_recovery.set()

        def rollout():
            self.serve = lambda name: (404 if name == "entry-old.js" else 503, b"edge fault")
            self.assertTrue(observed_failures.wait(2))
            self.serve = self.serve_stock
            self.assertTrue(observed_recovery.wait(2))
            self.serve = self.serve_prepare

        with self.assertRaisesRegex(Refusal, "Web prepare asset availability failed"):
            self.watch(rollout, record=record)
        result = self.rows[-1]
        self.assertFalse(result["passed"])
        self.assertGreater(result["phases"]["overlap"]["http_404"], 0)
        self.assertGreater(result["phases"]["overlap"]["http_503"], 0)
        self.assertEqual(result["phases"]["prepare"]["failures"], 0)

    def test_stock_failure_prevents_the_rollout(self):
        self.serve = lambda _: (404, b"missing")
        with self.assertRaisesRegex(Refusal, "Web stock asset availability failed"):
            self.watch(lambda: self.fail("Stock failure must prevent rollout"))
        self.assertEqual(self.rows[-1]["phases"]["stock"]["http_404"], len(STOCK))
        self.assertEqual(self.rows[-1]["phases"]["overlap"]["attempts"], 0)

    def test_prepare_requires_the_catalog_digest_and_all_incumbent_bytes(self):
        with self.assertRaisesRegex(Refusal, "does not match its release digest"):
            self.watch(lambda: self.fail("Catalog failed validation"), prepare_sha256="c" * 64)
        del self.prepare["chunk-old.js"]
        with self.assertRaisesRegex(Refusal, "does not preserve the incumbent catalog"):
            self.watch(lambda: self.fail("Catalog failed validation"))
        self.assertEqual(self.requests, [])

    def test_prepare_fails_if_candidate_only_content_is_wrong(self):
        def rollout():
            self.serve = lambda name: (200, b"x" * len(PREPARE[name])) if name not in STOCK else self.serve_stock(name)

        with self.assertRaisesRegex(Refusal, "Web prepare asset availability failed"):
            self.watch(rollout)
        self.assertEqual(self.rows[-1]["phases"]["prepare"]["failures"], 1)

    def test_cli_uses_fresh_handoff_files_and_journals_the_result(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            stock, prepare, ready, submitted, settled, journal = (
                root / name for name in ("stock.json", "prepare.json", "ready", "submitted", "settled", "evidence.jsonl")
            )
            stock.write_bytes(catalog_bytes(STOCK))
            prepare.write_bytes(catalog_bytes(PREPARE))
            arguments = [
                sys.executable, "-B", str(Path(__file__).with_name("staging_cd_web_prepare_availability.py")),
                "--origin", self.origin, "--incumbent-catalog", str(stock), "--prepare-catalog", str(prepare),
                "--incumbent-sha256", sha256(self.stock), "--prepare-sha256", sha256(self.prepare),
                "--incumbent-image", STOCK_IMAGE, "--prepare-image", PREPARE_IMAGE,
                "--ready-file", str(ready), "--submit-file", str(submitted), "--settled-file", str(settled),
                "--evidence", str(journal),
                "--seconds", "4",
            ]
            with subprocess.Popen(arguments, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
                try:
                    deadline = time.monotonic() + 3
                    while not ready.is_file() and process.poll() is None and time.monotonic() < deadline:
                        time.sleep(0.01)
                    self.assertTrue(ready.is_file())
                    submitted.touch()
                    submit_ns = submitted.stat().st_mtime_ns
                    while time.monotonic() < deadline:
                        if any(row.get("request_started_ns", 0) >= submit_ns
                               for row in map(json.loads, journal.read_text().splitlines()[:-1])):
                            break
                        time.sleep(0.01)
                    else:
                        self.fail("No overlap request started after the submit marker")
                    self.serve = self.serve_prepare
                    status = root / "status"
                    status.write_text("prepared\n")
                    status.rename(settled)
                    stdout, stderr = process.communicate(timeout=5)
                    self.assertEqual(process.returncode, 0, stderr)
                    self.assertIn("PASS", stdout)
                finally:
                    if process.poll() is None:
                        process.kill()
            original = journal.read_text()
            self.assertTrue(json.loads(original.splitlines()[-1])["passed"])
            replay = subprocess.run(arguments, text=True, capture_output=True, timeout=5)
            self.assertEqual(replay.returncode, 1)
            self.assertEqual(journal.read_text(), original)


if __name__ == "__main__":
    unittest.main()

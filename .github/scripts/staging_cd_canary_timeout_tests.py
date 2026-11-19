"""The actual Node campaign cannot create a resource after Python reconciliation."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from staging_cd_build import command
from staging_cd_canary_process_tests import alive
from staging_cd_runtime_smoke import CleanupIncomplete, qualify


class CanaryTimeoutTests(unittest.TestCase):
    def test_docker_volume_helper_is_dead_before_inventory_and_failure_keeps_id(self):
        self.check_timeout(startup_delay=0)

    def test_slow_start_still_exercises_the_live_volume_helper_timeout(self):
        self.check_timeout(startup_delay=0.8)

    def check_timeout(self, *, startup_delay):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            config, source = "sha256:" + "a" * 64, "b" * 40
            image = {"Id": config, "Os": "linux", "Architecture": "amd64", "Config": {
                "User": "10001:10001", "Labels": {"org.opencontainers.image.revision": source,
                    "org.opencontainers.image.source": "https://github.com/ChronoAIProject/cma",
                    "dev.chrono.cma.runtime-contract": "3", "dev.chrono.cma.runtime-bootstrap": "1",
                    "dev.chrono.cma.home-layout": "2"}}}
            pidfile, resource = root / "pid", root / "late-resource"
            docker = root / "docker"
            docker.write_text(f"#!{sys.executable}\n" +
                "import os, signal, sys, time\nfrom pathlib import Path\n"
                "if sys.argv[1:3] == ['image', 'inspect']:\n"
                f"    time.sleep({startup_delay!r})\n"
                f"    print({json.dumps([image])!r})\n"
                "elif sys.argv[1:3] == ['volume', 'create']:\n"
                "    signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                f"    Path({str(pidfile)!r}).write_text(str(os.getpid()))\n"
                "    time.sleep(2)\n"
                f"    Path({str(resource)!r}).touch()\n"
                "else: raise SystemExit(2)\n")
            docker.chmod(0o700)
            popen = subprocess.Popen
            def start_ready(*args, **kwargs):
                child = popen(*args, **kwargs)
                # The deadline must exercise a live creator, independent of
                # interpreter startup and runner scheduling. Return even on
                # startup failure so the real owner still reaps the group.
                deadline = time.monotonic() + 10
                while child.poll() is None and not pidfile.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                return child
            inventories = []
            def runner(args, *, timeout, env=None):
                if args[0] == "node":
                    # Hosted Python may need its loader path for the fake
                    # Docker executable; this is fixture-only configuration.
                    loader = {key: os.environ[key] for key in ("LD_LIBRARY_PATH",) if key in os.environ}
                    with patch("staging_cd_canary_process.subprocess.Popen", side_effect=start_ready):
                        return command([shutil.which("node"), *args[1:]], timeout=0.5,
                                       env=dict(env, PATH=str(root), **loader))
                if args[:3] == ["skopeo", "inspect", "--raw"]:
                    return json.dumps({"config": {"digest": config}}).encode()
                if args[:3] == ["docker", "image", "inspect"]:
                    return json.dumps([image]).encode()
                if args[0] == "docker" and (args[1] == "ps" or args[1:3] == ["volume", "ls"]):
                    self.assertFalse(alive(int(pidfile.read_text())))
                    self.assertFalse(resource.exists())
                    inventories.append(args[-1])
                return b""
            with self.assertRaises(CleanupIncomplete) as raised:
                qualify(root / "fixture.oci.tar", {"component": "runtime", "source": source},
                        "sha256:" + "d" * 64, "e" * 64, root, runner,
                        source=Path(__file__).resolve().parents[2])
            proof = raised.exception.receipt
            self.assertTrue(proof["helpers_quiesced"])
            self.assertTrue(pidfile.exists(), "Volume creator must start before the timeout")
            self.assertTrue(proof["inventory_reconciled"])
            self.assertEqual(proof["cleanup"], "incomplete")
            self.assertEqual(len(inventories), 4)
            self.assertTrue(all(value == "label=dev.chrono.cma.evidence=" + proof["campaign"]
                                for value in inventories))
            self.assertFalse(alive(int(pidfile.read_text())))
            self.assertFalse(resource.exists())

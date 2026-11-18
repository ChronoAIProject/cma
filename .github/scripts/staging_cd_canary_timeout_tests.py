"""The actual Node campaign cannot create a resource after Python reconciliation."""
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

from staging_cd_build import command
from staging_cd_canary_process_tests import alive
from staging_cd_runtime_smoke import CleanupIncomplete, qualify


class CanaryTimeoutTests(unittest.TestCase):
    def test_docker_volume_helper_is_dead_before_inventory_and_failure_keeps_id(self):
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
                f"    print({json.dumps([image])!r})\n"
                "elif sys.argv[1:3] == ['volume', 'create']:\n"
                "    signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                f"    Path({str(pidfile)!r}).write_text(str(os.getpid()))\n"
                "    time.sleep(2)\n"
                f"    Path({str(resource)!r}).touch()\n"
                "else: raise SystemExit(2)\n")
            docker.chmod(0o700)
            inventories = []
            def runner(args, *, timeout, env=None):
                if args[0] == "node":
                    return command([shutil.which("node"), *args[1:]], timeout=0.5,
                                   env=dict(env, PATH=str(root)))
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
            self.assertTrue(proof["inventory_reconciled"])
            self.assertEqual(proof["cleanup"], "incomplete")
            self.assertEqual(len(inventories), 4)
            self.assertTrue(all(value == "label=dev.chrono.cma.evidence=" + proof["campaign"]
                                for value in inventories))
            self.assertFalse(alive(int(pidfile.read_text())))
            self.assertFalse(resource.exists())

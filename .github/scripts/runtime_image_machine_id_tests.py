"""Container images must not distribute the builder's machine identity."""

import concurrent.futures
import contextlib
import importlib.util
import io
import re
from pathlib import Path
import tempfile
import unittest


IMAGE = Path(__file__).resolve().parents[2] / "api/crates/layer4-runtime/image"


def module(name):
    spec = importlib.util.spec_from_file_location(name, IMAGE / (name + ".py"))
    loaded = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loaded)
    return loaded


NORMALIZE = module("normalize-machine-id")
GATE = module("assert-no-private-keyrings")
START = module("initialize-machine-id")


class MachineIdentityTests(unittest.TestCase):
    def test_both_populated_locations_refuse_without_disclosing_values(self):
        for name in ("etc/machine-id", "var/lib/dbus/machine-id"):
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                identity = root / name
                identity.parent.mkdir(parents=True)
                identity.write_text("synthetic-machine-identity")
                self.assertEqual(GATE.machine_identities(root), [name])
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    self.assertEqual(GATE.main(root), 1)
                self.assertEqual(output.getvalue(), "Runtime image hygiene check failed: "
                                 "private_entries=0, machine_id_entries=1\n")

    def test_normalize_copied_identity_and_dangling_link_idempotently(self):
        for initial in ("populated", "dangling", "absent"):
            with self.subTest(initial=initial), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                etc = root / "etc/machine-id"
                bus = root / "var/lib/dbus/machine-id"
                etc.parent.mkdir(parents=True)
                bus.parent.mkdir(parents=True)
                if initial == "populated":
                    for path in (etc, bus):
                        path.write_text("synthetic-machine-identity")
                        path.chmod(0o444)
                elif initial == "dangling":
                    bus.symlink_to("/etc/machine-id")
                for _ in range(2):
                    NORMALIZE.normalize(root)
                    self.assertFalse(etc.is_symlink())
                    self.assertEqual(etc.read_bytes(), b"")
                    self.assertEqual(etc.stat().st_mode & 0o777, 0o444)
                    self.assertEqual(bus.readlink().as_posix(), "/etc/machine-id")
                    self.assertEqual(GATE.machine_identities(root), [])
                    with contextlib.redirect_stdout(io.StringIO()):
                        self.assertEqual(GATE.main(root), 0)

    def test_symlink_normalization_does_not_modify_external_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "image"
            identity = root / "etc/machine-id"
            identity.parent.mkdir(parents=True)
            outside = Path(directory) / "outside"
            outside.write_text("synthetic-outside-value")
            identity.symlink_to(outside)
            NORMALIZE.normalize(root)
            self.assertEqual(outside.read_text(), "synthetic-outside-value")
            self.assertEqual(identity.read_bytes(), b"")

    def test_links_special_files_and_incomplete_inspection_fail_closed(self):
        cases = (("etc/machine-id", "/outside"),
                 ("var/lib/dbus/machine-id", "/outside"),
                 ("var/lib/dbus/machine-id", "/etc/machine-id"))
        for name, target in cases:
            with self.subTest(name=name, target=target), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                path = root / name
                path.parent.mkdir(parents=True)
                path.symlink_to(target)
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertNotEqual(GATE.main(root), 0)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "etc/machine-id").mkdir(parents=True)
            self.assertEqual(GATE.machine_identities(root), ["etc/machine-id"])
            with self.assertRaises(OSError):
                NORMALIZE.normalize(root)

    def test_relative_dbus_link_is_checked_against_image_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            NORMALIZE.normalize(root)
            bus = root / "var/lib/dbus/machine-id"
            bus.unlink()
            bus.symlink_to("../../../etc/machine-id")
            self.assertEqual(GATE.machine_identities(root), [])
            identity = root / "etc/machine-id"
            identity.chmod(0o644)
            identity.write_text("synthetic-machine-identity")
            self.assertEqual(GATE.machine_identities(root),
                             ["etc/machine-id", "var/lib/dbus/machine-id"])


class SandboxStartupIdentityTests(unittest.TestCase):
    def test_independent_concurrent_boots_and_retry_keep_one_valid_winner(self):
        with tempfile.TemporaryDirectory() as scratch:
            first, second = (Path(scratch) / name for name in ("first", "second"))
            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as workers:
                list(workers.map(START.initialize, [first, second, first, second]))
            left = (first / "machine-id").read_bytes()
            right = (second / "machine-id").read_bytes()
            self.assertTrue(re.fullmatch(rb"[0-9a-f]{32}\n", left) is not None)
            self.assertTrue(re.fullmatch(rb"[0-9a-f]{32}\n", right) is not None)
            self.assertTrue(left != right)
            START.initialize(first)
            self.assertTrue(left == (first / "machine-id").read_bytes())
            self.assertEqual((first / "machine-id").stat().st_mode & 0o777, 0o444)

    def test_invalid_existing_state_is_refused_without_overwriting(self):
        with tempfile.TemporaryDirectory() as scratch:
            directory = Path(scratch) / "state"
            directory.mkdir(mode=0o700)
            target = directory / "machine-id"
            for raw in (b"", b"0" * 32 + b"\n", b"wrong", b"a" * 34):
                target.write_bytes(raw)
                target.chmod(0o444)
                with self.assertRaises(ValueError):
                    START.initialize(directory)
                self.assertTrue(target.read_bytes() == raw)
                target.unlink()
            target.symlink_to(Path(scratch) / "absent")
            with self.assertRaises(OSError):
                START.initialize(directory)
            target.unlink()
            directory.chmod(0o777)
            with self.assertRaises(ValueError):
                START.initialize(directory)
            directory.rmdir()
            directory.symlink_to(scratch)
            with self.assertRaises(OSError):
                START.initialize(directory)

    def test_build_gate_accepts_only_the_uninitialized_private_ipc_target(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            NORMALIZE.normalize(root)
            canonical = root / "etc/machine-id"
            canonical.unlink()
            canonical.symlink_to("/dev/shm/cma-machine-id/machine-id")
            self.assertEqual(GATE.machine_identities(root), [])
            state = root / "dev/shm/cma-machine-id"
            state.mkdir(parents=True)
            self.assertTrue(GATE.machine_identities(root))
            state.rmdir()
            (root / "dev/shm").rmdir()
            (root / "dev/shm").symlink_to(scratch)
            self.assertTrue(GATE.machine_identities(root))


if __name__ == "__main__":
    unittest.main()

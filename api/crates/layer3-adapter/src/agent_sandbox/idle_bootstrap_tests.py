import importlib.util
import os
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("idle", Path(__file__).with_name("idle_bootstrap.py"))
idle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(idle)


class BootstrapProof(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.runtime = self.root / "1"
        (self.root / "77").mkdir()
        for name in ["fd", "task/1", "task/2", "net"]:
            (self.runtime / name).mkdir(parents=True)
        self.stat(self.runtime, 100)
        self.stat(self.runtime / "task/1", 100)
        self.stat(self.runtime / "task/2", 101)
        (self.runtime / "task/1/comm").write_text("layer4-runtime\n")
        (self.runtime / "task/2/comm").write_text("tokio-rt-worker\n")
        (self.runtime / "exe").symlink_to(idle.BINARY)
        (self.runtime / "cmdline").write_bytes((idle.BINARY + "\0--wait-for-assignment\0").encode())
        for index, target in enumerate(["pipe:[1]", "pipe:[2]", "pipe:[3]",
                                        "anon_inode:[eventpoll]", "anon_inode:[eventfd]",
                                        "socket:[10]", "socket:[11]", "socket:[12]"]):
            (self.runtime / "fd" / str(index)).symlink_to(target)
        (self.runtime / "net/unix").write_text(
            "Num RefCount Protocol Flags Type St Inode Path\n"
            "0000: 00000002 00000000 00010000 0001 01 10 " + idle.SOCKET + "\n"
            "0001: 00000003 00000000 00000000 0001 03 11\n"
            "0002: 00000003 00000000 00000000 0001 03 12\n")

    def tearDown(self):
        self.directory.cleanup()

    @staticmethod
    def stat(directory, start):
        fields = ["S", "0"] + ["0"] * 17 + [str(start)]
        (directory / "stat").write_text("1 (runtime) " + " ".join(fields))

    def test_complete_idle_inventory(self):
        proof = idle.observe(self.root, 77)
        self.assertEqual(proof["identity"], {"parent": 0, "start": 100})
        self.assertEqual(len(proof["threads"]), 2)

    def test_supervisor_reaper_proves_assignment_even_without_children(self):
        (self.runtime / "task/2/comm").write_text("cma-adopted-chi\n")
        with self.assertRaises(idle.Refused):
            idle.observe(self.root, 77)

    def test_any_other_process_refuses(self):
        (self.root / "88").mkdir()
        with self.assertRaises(idle.Refused):
            idle.observe(self.root, 77)

    def test_assignment_peer_or_network_socket_refuses(self):
        (self.runtime / "fd/8").symlink_to("socket:[13]")
        with self.assertRaises(idle.Refused):
            idle.observe(self.root, 77)

    def test_missing_worker_or_descriptor_refuses(self):
        (self.runtime / "fd/5").unlink()
        with self.assertRaises(idle.Refused):
            idle.observe(self.root, 77)

    def test_tenant_file_writer_refuses(self):
        (self.runtime / "fd/8").symlink_to("/home/cma/workspace")
        with self.assertRaises(idle.Refused):
            idle.observe(self.root, 77)

    def test_assignment_listener_must_be_exact(self):
        path = self.runtime / "net/unix"
        path.write_text(path.read_text().replace(idle.SOCKET, "/tmp/foreign.sock"))
        with self.assertRaises(idle.Refused):
            idle.observe(self.root, 77)

    def test_foreign_executable_or_args_refuse(self):
        (self.runtime / "cmdline").write_bytes(b"layer4-runtime\0")
        with self.assertRaises(idle.Refused):
            idle.observe(self.root, 77)

    def test_unreadable_inventory_does_not_silently_drop_a_thread(self):
        (self.runtime / "task/3").mkdir()
        with self.assertRaises(OSError):
            idle.observe(self.root, 77)


if __name__ == "__main__":
    unittest.main()

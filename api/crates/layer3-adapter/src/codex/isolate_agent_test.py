"""Exercise fixed relay destinations and launch confinement without services."""
import importlib.util
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock


SCRIPT = pathlib.Path(__file__).with_name("isolate_agent.py")
spec = importlib.util.spec_from_file_location("isolate_agent", SCRIPT)
isolate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(isolate)


class IsolationTests(unittest.TestCase):
    def test_namespace_exposes_only_own_writable_paths_and_fixed_proxy_socket_dir(self):
        home = "/home/cma/.cma/agents/agt_private/home"
        cwd = "/home/cma/work/.worktrees/wkt_private"
        with mock.patch.object(isolate.os.path, "isdir", return_value=True):
            command = isolate.namespace_command(home, cwd, "/tmp/private-sockets", "source")
        self.assertEqual(command[0], "/usr/bin/bwrap")
        for namespace in ["--unshare-user", "--unshare-pid", "--unshare-net", "--unshare-ipc"]:
            self.assertIn(namespace, command)
        mounts = [command[index + 1:index + 3] for index, arg in enumerate(command) if arg == "--bind"]
        self.assertEqual(mounts, [[home, home], [cwd, cwd]])
        self.assertIn(["--proc", "/proc"], [command[i:i+2] for i in range(len(command)-1)])
        self.assertNotIn("--share-net", command)
        self.assertNotIn("/var/lib/cma-runtime", command)
        self.assertNotIn("/home/cma/.cma/provider/codex/auth.json", command)
        self.assertNotIn("/home/cma/work/.git", command)
        self.assertEqual(command[-7:], ["/usr/bin/python3", "-I", "-c", "source", "inner", home, cwd])

    def test_only_fixed_proxy_destinations_are_registered(self):
        self.assertEqual(isolate.PROXIES, (
            ("model", "127.0.0.1", 9091),
            ("platform", "127.0.0.2", 9092),
            ("tools", "127.0.0.1", 9093),
        ))

    def test_relay_duplex_forwards_to_fixed_server_even_with_destination_headers(self):
        with socket.socket() as upstream:
            upstream.bind(("127.0.0.1", 0))
            upstream.listen()
            address = upstream.getsockname()
            received = []

            def serve():
                client, _ = upstream.accept()
                with client:
                    payload = b""
                    while chunk := client.recv(4096):
                        payload += chunk
                    received.append(payload)
                    client.sendall(b"fixture response")

            thread = threading.Thread(target=serve, daemon=True)
            thread.start()
            with tempfile.TemporaryDirectory(prefix="cma-relay-test-", dir="/tmp") as root:
                with mock.patch.object(isolate, "PROXIES", (("fixture", *address),)):
                    with isolate.relays(root, inside=False):
                        with socket.socket(socket.AF_UNIX) as client:
                            client.settimeout(3)
                            client.connect(str(pathlib.Path(root) / "fixture.sock"))
                            payload = b"CONNECT 127.0.0.1:9090 HTTP/1.1\r\nHost: other\r\n\r\n"
                            client.sendall(payload)
                            client.shutdown(socket.SHUT_WR)
                            self.assertEqual(client.recv(4096), b"fixture response")
            thread.join(timeout=3)
            self.assertEqual(received, [payload])

    def test_absent_bwrap_fails_without_launching_an_unconfined_provider(self):
        with mock.patch.object(isolate, "BWRAP", "/missing/cma-test-bwrap"):
            command = isolate.namespace_command("/tmp/home", "/tmp/work", "/tmp/proxies", "source")
        with self.assertRaises(FileNotFoundError):
            isolate.wait_child(command)

    def test_python_isolated_mode_ignores_agent_module_shadowing(self):
        with tempfile.TemporaryDirectory(prefix="cma-shadow-test-", dir="/tmp") as root:
            pathlib.Path(root, "socket.py").write_text("raise RuntimeError('agent module escaped')")
            env = dict(os.environ, PYTHONPATH=root)
            result = subprocess.run(
                [os.sys.executable, "-I", "-c", "import socket; print(socket.AF_UNIX)"],
                cwd=root, env=env, capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    @unittest.skipUnless(os.environ.get("CMA_ISOLATION_NATIVE_TEST") == "1", "requires Linux runtime bwrap")
    def test_native_namespace_hides_sibling_files_processes_and_control_socket(self):
        self.assertEqual(sys.platform, "linux")
        with tempfile.TemporaryDirectory(prefix="cma-native-isolation-", dir="/tmp") as root:
            home = pathlib.Path(root, "home")
            cwd = pathlib.Path(root, "work")
            home.mkdir()
            cwd.mkdir()
            sibling = pathlib.Path(root, "sibling-transcript")
            sibling.write_text("sibling-private-fixture")
            pathlib.Path(home, "mine").write_text("own-history")
            socket_root = pathlib.Path(root, "sockets")
            socket_root.mkdir()
            with socket.socket() as host_control:
                host_control.bind(("0.0.0.0", 0))
                host_control.listen()
                port = host_control.getsockname()[1]
                checks = """
import os, pathlib, socket, sys
home, sibling, port, host_pid_namespace = sys.argv[1:]
assert pathlib.Path(home, 'mine').read_text() == 'own-history'
assert not pathlib.Path(sibling).exists()
assert not pathlib.Path('/var/lib/cma-runtime').exists()
assert os.readlink('/proc/self/ns/pid') != host_pid_namespace
with socket.socket() as probe:
    probe.settimeout(1)
    assert probe.connect_ex(('127.0.0.1', int(port))) != 0
pathlib.Path(home, 'written').write_text('private-write')
"""
                command = isolate.namespace_command(str(home), str(cwd), str(socket_root), "unused")
                command = command[:command.index("--") + 1] + [
                    isolate.PYTHON, "-I", "-c", checks, str(home), str(sibling), str(port),
                    os.readlink("/proc/self/ns/pid"),
                ]
                result = subprocess.run(command, capture_output=True, timeout=20)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(pathlib.Path(home, "written").read_text(), "private-write")


if __name__ == "__main__":
    unittest.main()

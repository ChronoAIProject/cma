"""Exercise fixed relay destinations and launch confinement without services."""
import contextlib
import importlib.util
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import types
import unittest
from unittest import mock


SCRIPT = pathlib.Path(__file__).with_name("isolate_agent.py")
spec = importlib.util.spec_from_file_location("isolate_agent", SCRIPT)
isolate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(isolate)


class PlatformHttpTests(unittest.TestCase):
    prefix = "/api/v1/proxy/s/"
    action = prefix + "cma/api/v1/bot-setups/actions"

    def exchange(self, payload, reply=b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"):
        """Exercise the real gate with disposable, credential-free socket pairs."""
        client, gate = socket.socketpair()
        upstream, peer = socket.socketpair()
        with client, gate, upstream, peer:
            for stream in (client, gate, upstream, peer):
                stream.settimeout(3)
            peer.sendall(reply)
            peer.shutdown(socket.SHUT_WR)
            server = types.SimpleNamespace(destination=("127.0.0.2", 9092))

            def handle():
                try:
                    isolate.PlatformHttp(gate, None, server)
                finally:
                    gate.close()

            with mock.patch.object(isolate.socket, "create_connection", return_value=upstream) as connect:
                thread = threading.Thread(target=handle, daemon=True)
                thread.start()
                with contextlib.suppress(BrokenPipeError, ConnectionResetError):
                    client.sendall(payload)
                with contextlib.suppress(OSError):
                    client.shutdown(socket.SHUT_WR)
                result = bytearray()
                with contextlib.suppress(ConnectionResetError):
                    while chunk := client.recv(65536):
                        result.extend(chunk)
                thread.join(timeout=3)
                self.assertFalse(thread.is_alive())
                forwarded = bytearray()
                if connect.called:
                    connect.assert_called_once_with(("127.0.0.2", 9092), timeout=10)
                    while chunk := peer.recv(65536):
                        forwarded.extend(chunk)
            return bytes(result), bytes(forwarded)

    def test_platform_gate_is_on_host_unix_socket_not_bypassable_local_listener(self):
        with tempfile.TemporaryDirectory(prefix="cma-platform-test-", dir="/tmp") as root:
            with mock.patch.object(isolate, "PROXIES", (("platform", "127.0.0.2", 9092),)):
                with mock.patch.object(isolate, "UnixServer", wraps=isolate.UnixServer) as build:
                    with isolate.relays(root, inside=False):
                        self.assertIs(build.call_args_list[0].args[1], isolate.PlatformHttp)

    def test_both_platform_listeners_relay_to_the_same_restricted_socket(self):
        platform = (("platform", "127.0.0.2", 9092),)
        with mock.patch.object(isolate, "PROXIES", platform), \
                mock.patch.object(isolate, "TcpServer") as server, \
                mock.patch.object(isolate.threading, "Thread"):
            legacy, canonical, https = (mock.Mock() for _ in range(3))
            server.side_effect = [legacy, canonical, https]
            with isolate.relays("/fixture", inside=True):
                server.assert_has_calls([
                    mock.call(("127.0.0.2", 9092), isolate.Relay),
                    mock.call(("127.0.0.1", 9092), isolate.Relay),
                ])
                self.assertEqual(legacy.destination, "/fixture/platform.sock")
                self.assertEqual(canonical.destination, legacy.destination)

    def test_only_delegated_cma_actions_and_explicit_service_slugs_are_allowed(self):
        for path in isolate.CMA_OPERATIONS:
            target = self.prefix + "cma/" + path
            response, forwarded = self.exchange(
                f'POST {target} HTTP/1.1\r\nHost: ignored\r\nContent-Length: 2\r\n\r\n{{}}'.encode())
            self.assertIn(b"200", response)
            self.assertTrue(forwarded.startswith(f"POST {target} HTTP/1.1".encode()))
        for slug in ("chrono-llm-public", "ornn-api"):
            for method in ("GET", "HEAD", "POST", "PATCH", "PUT", "DELETE", "OPTIONS"):
                isolate.platform_target(method, self.prefix + slug + "/v1/resource?q=hello%20world")

    def test_sibling_conversation_management_catalogue_and_aliases_are_denied(self):
        for target in (
            self.prefix + "cma/api/v1/provider/bindings/binding/conversations/sibling",
            self.prefix + "cma/api/v1/agents/sibling",
            self.prefix + "cma/api/v1/organizations",
            self.prefix + "cmaeg/v1/channels",
            self.prefix + "other-service/api/v1/agents",
            "/api/v1/proxy/00000000-0000-0000-0000-000000000001/api/v1/agents",
            "/api/v1/proxy/services", "/api/v1/user-services", "/api/v1/auth/me",
            self.prefix + "cma/api/v1/bot-setups/actions/",
            self.action + "?owner=organization",
        ):
            with self.subTest(target=target):
                response, forwarded = self.exchange(f"POST {target} HTTP/1.1\r\nHost: local\r\n\r\n".encode())
                self.assertIn(b"403", response)
                self.assertFalse(forwarded)
        for method in ("GET", "PUT", "PATCH", "DELETE", "OPTIONS"):
            with self.assertRaises(ValueError):
                isolate.platform_target(method, self.action)

    def test_route_normalization_and_explicit_connection_overrides_fail_closed(self):
        for target in (
            "https://nyx.example" + self.action, "/" + self.action,
            self.prefix + "ornn-api/../cma/api/v1/agents",
            self.prefix + "ornn-api/%2e%2e/cma/api/v1/agents",
            self.prefix + "ornn-api/%252e%252e/cma/api/v1/agents",
            self.prefix + "ornn-api/%2f../cma", self.prefix + "ornn-api/%255c../cma",
            self.prefix + "ornn-api//cma", self.prefix + "ornn-api/a;../cma",
            self.prefix + "ornn-api/a%3f../cma", self.prefix + "ornn-api/a%23../cma",
            self.prefix + "ornn-api/%00", self.prefix + "ornn-api/%0d%0a",
            self.prefix + "ornn-api/%zz", self.prefix + "ornn-api/%",
            self.prefix + "ornn-api/%" + "25" * 10 + "2e",
            self.prefix + "%63ma/api/v1/bot-setups/actions",
            self.prefix + "ornn-api/path#fragment",
            self.prefix + "ornn-api/path?_nyxid_via=other",
            self.prefix + "ornn-api/path?%5fnyxid_via=other",
            self.prefix + "ornn-api/path?%255fnyxid_via=other",
            self.prefix + "ornn-api/path?q=1%26_NYXID_VIA=other",
        ):
            with self.subTest(target=target), self.assertRaises(ValueError):
                isolate.platform_target("POST", target)
        isolate.platform_target("GET", self.prefix + "ornn-api/skills/name%20with%20spaces")
        isolate.platform_target("GET", self.prefix + "ornn-api/skills?q=100%25")

    def test_request_deadline_is_total_even_when_a_client_keeps_sending_header_bytes(self):
        client = mock.Mock()
        client.recv.return_value = b"P"
        with mock.patch.object(isolate.time, "monotonic", side_effect=(100, 110, 120, 131)):
            with self.assertRaisesRegex(ValueError, "deadline expired"):
                isolate.platform_request(client)
        self.assertEqual(client.recv.call_count, 2)
        self.assertEqual(client.settimeout.call_args_list, [mock.call(20), mock.call(10)])

    def test_reserializes_body_and_idempotency_without_caller_identity_or_override_headers(self):
        body = b'{"event_ref":"fixture-event","text":"plain \\u2603"}'
        request = (f"POST {self.action} HTTP/1.1\r\nHost: attacker\r\n"
                   "Content-Type: application/json\r\nIdempotency-Key: action-fixture\r\n"
                   "Authorization: Bearer caller-fixture\r\nCookie: session=caller-fixture\r\n"
                   "X-Nyxid-Identity-Token: caller-fixture\r\nX-Nyxid-Delegation-Token: caller-fixture\r\n"
                   "X-Nyxid-Debug-Upstream: true\r\nX-HTTP-Method-Override: GET\r\n"
                   "Forwarded: host=attacker\r\nConnection: keep-alive\r\n"
                   f"Content-Length: {len(body)}\r\n\r\n").encode() + body
        response, forwarded = self.exchange(request)
        self.assertIn(b"200", response)
        headers, sent_body = forwarded.split(b"\r\n\r\n", 1)
        self.assertEqual(sent_body, body)
        self.assertIn(b"Host: 127.0.0.2:9092", headers)
        self.assertIn(b"Connection: close", headers)
        self.assertIn(b"idempotency-key: action-fixture", headers)
        self.assertIn(b"content-type: application/json", headers)
        for forbidden in (b"caller-fixture", b"attacker", b"x-nyxid", b"cookie", b"override", b"forwarded"):
            self.assertNotIn(forbidden, headers.lower())

    def test_request_smuggling_upgrade_framing_and_incomplete_bodies_never_reach_upstream(self):
        for extra in (
            b"Content-Length: 0\r\nContent-Length: 0\r\n",
            b"Content-Length: 0\r\ncontent-length: 1\r\n",
            b"Transfer-Encoding: chunked\r\n", b"Content-Length: 0\r\nTransfer-Encoding: chunked\r\n",
            b"Content-Length: +1\r\n", b"Content-Length: 1, 1\r\n", b"Content-Length: -1\r\n",
            b"Content-Length: 16777217\r\n", b"Content-Length: 999999999999999999999\r\n",
            b"Content-Length: 1\r\n", b"Upgrade: websocket\r\n", b"Connection: upgrade\r\n",
            b"Connection: keep-alive, x-override\r\n", b"Expect: 100-continue\r\n",
            b"Trailer: authorization\r\n", b"TE: trailers\r\n", b"Content-Encoding: gzip\r\n",
            b"Host: duplicate\r\n", b" X-Folded: value\r\n", b"X-Bad : value\r\n",
            b"X-Control: value\x00\r\n", b"X-Injection: value\nInjected: bad\r\n",
            b"X-Long: " + b"a" * 16384 + b"\r\n",
        ):
            with self.subTest(header=extra[:80]):
                response, forwarded = self.exchange(
                    f"POST {self.action} HTTP/1.1\r\nHost: local\r\n".encode() + extra + b"\r\n")
                # Closing an oversized request with unread data can reset its
                # socket before the peer receives the refusal response.
                if response or len(extra) < 16384:
                    self.assertIn(b"403", response)
                self.assertFalse(forwarded)
        for line in (f"CONNECT {self.action} HTTP/1.1", f"POST {self.action} HTTP/1.0",
                     f"POST {self.action} HTTP/2.0", f"POST  {self.action} HTTP/1.1"):
            response, forwarded = self.exchange(line.encode() + b"\r\nHost: local\r\n\r\n")
            self.assertIn(b"403", response)
            self.assertFalse(forwarded)

    def test_batched_pipeline_is_rejected_before_authorized_first_request(self):
        payload = (f"POST {self.action} HTTP/1.1\r\nHost: local\r\nContent-Length: 2\r\n\r\n{{}}"
                   f"GET {self.prefix}cma/api/v1/agents HTTP/1.1\r\nHost: local\r\n\r\n")
        response, forwarded = self.exchange(payload.encode())
        self.assertIn(b"403", response)
        self.assertFalse(forwarded)

    def test_response_upgrade_and_credential_failure_never_tunnel_or_retry(self):
        request = f"POST {self.action} HTTP/1.1\r\nHost: local\r\n\r\n".encode()
        for reply, expected in (
            (b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: websocket\r\n\r\n", b"403"),
            (b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n", b"503"),
            (b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n", b"401"),
        ):
            response, forwarded = self.exchange(request, reply)
            self.assertIn(expected, response)
            self.assertEqual(forwarded.count(b"HTTP/1.1"), 1)
            self.assertNotIn(b"Upgrade", response)

    def test_chunked_model_response_streams_before_completion_and_drops_late_client_bytes(self):
        client, gate = socket.socketpair()
        upstream, peer = socket.socketpair()
        with client, gate, upstream, peer:
            for stream in (client, gate, upstream, peer):
                stream.settimeout(3)
            server = types.SimpleNamespace(destination=("127.0.0.2", 9092))

            def handle():
                try:
                    isolate.PlatformHttp(gate, None, server)
                finally:
                    gate.close()

            with mock.patch.object(isolate.socket, "create_connection", return_value=upstream):
                thread = threading.Thread(target=handle, daemon=True)
                thread.start()
                client.sendall(f"POST {self.prefix}chrono-llm-public/v1/responses HTTP/1.1\r\nHost: local\r\n\r\n".encode())
                sent = bytearray()
                while b"\r\n\r\n" not in sent:
                    chunk = peer.recv(65536)
                    self.assertTrue(chunk, "upstream closed before request headers")
                    sent.extend(chunk)
                peer.sendall(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/event-stream\r\n\r\n"
                             b"c\r\ndata:first\n\n\r\n")
                result = bytearray()
                while b"data:first\n\n" not in result:
                    chunk = client.recv(65536)
                    self.assertTrue(chunk, "stream closed before the first event")
                    result.extend(chunk)
                # This is deliberately sent after the first operation reached
                # upstream. A raw relay would lend it the same credential.
                client.sendall(f"GET {self.prefix}cma/api/v1/agents HTTP/1.1\r\nHost: local\r\n\r\n".encode())
                peer.sendall(b"d\r\ndata:second\n\n\r\n0\r\n\r\n")
                # Linux may reset on closing this deliberately malformed
                # client with its second request unread. The full response
                # must still have arrived; a reset never counts as completion.
                with contextlib.suppress(ConnectionResetError):
                    while chunk := client.recv(65536):
                        result.extend(chunk)
                thread.join(timeout=3)
                self.assertFalse(thread.is_alive())
                self.assertEqual(result.split(b"\r\n\r\n", 1)[1], b"data:first\n\ndata:second\n\n")
                self.assertIn(b"content-type: text/event-stream", result)
                self.assertNotIn(b"Transfer-Encoding", result)
                self.assertEqual(peer.recv(65536), b"")


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

    def test_https_rejects_private_metadata_reserved_and_mixed_dns_answers(self):
        public = (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", 443))
        for address in [
            "127.0.0.1", "10.2.3.4", "172.16.0.1", "192.168.1.1",
            "169.254.169.254", "100.100.100.200", "0.0.0.0", "224.0.0.1",
            "::1", "fe80::1", "fc00::1", "fec0::1", "ff02::1", "::ffff:127.0.0.1",
        ]:
            family = socket.AF_INET6 if ":" in address else socket.AF_INET
            sockaddr = (address, 443, 0, 0) if family == socket.AF_INET6 else (address, 443)
            private = (family, socket.SOCK_STREAM, 6, "", sockaddr)
            for answers in ([private], [public, private]):
                with self.subTest(address=address, mixed=len(answers) > 1):
                    with mock.patch.object(isolate.socket, "getaddrinfo", return_value=answers):
                        with self.assertRaises(ValueError):
                            isolate.public_addresses("weather.example:443")

    def test_https_rejects_bad_authorities_and_plain_http_ports_before_dns(self):
        for authority in [
            "127.0.0.1:9090", "weather.example:80", "weather.example",
            "user:password@weather.example:443", "weather.example:443/path",
            "weather.example:443?x=1", "weather.example:443#fragment",
            "weather.example:443\r\n", "[fe80::1%eth0]:443",
        ]:
            with self.subTest(authority=authority):
                with mock.patch.object(isolate.socket, "getaddrinfo") as resolve:
                    with self.assertRaises(ValueError):
                        isolate.public_addresses(authority)
                    resolve.assert_not_called()

    def test_https_connect_pins_numeric_ip_after_single_dns_resolution(self):
        answers = [(socket.AF_INET, socket.SOCK_STREAM, 6, "", ("93.184.216.34", 443))]
        upstream = mock.Mock()
        with mock.patch.object(isolate.socket, "getaddrinfo", return_value=answers) as resolve:
            with mock.patch.object(isolate.socket, "socket", return_value=upstream):
                self.assertIs(isolate.connect_public("weather.example:443"), upstream)
                resolve.assert_called_once_with("weather.example", 443, type=socket.SOCK_STREAM)
                upstream.connect.assert_called_once_with(("93.184.216.34", 443))

    def test_https_connect_handler_tunnels_data_and_plain_http_is_explicitly_refused(self):
        with tempfile.TemporaryDirectory(prefix="cma-https-test-", dir="/tmp") as root:
            with mock.patch.object(isolate, "PROXIES", ()):
                with isolate.relays(root, inside=False):
                    for method, expected in [("GET https://weather.example/ HTTP/1.1", b"405"),
                                             ("CONNECT weather.example:443 HTTP/1.1", b"200")]:
                        remote, peer = socket.socketpair()
                        with remote, peer, mock.patch.object(isolate, "connect_public", return_value=remote):
                            with socket.socket(socket.AF_UNIX) as client:
                                client.settimeout(3)
                                peer.settimeout(3)
                                client.connect(str(pathlib.Path(root) / "https.sock"))
                                client.sendall(method.encode() + b"\r\nHost: weather.example\r\n\r\n")
                                self.assertIn(expected, client.recv(4096))
                                if expected == b"200":
                                    client.sendall(b"TLS fixture request")
                                    self.assertEqual(peer.recv(4096), b"TLS fixture request")
                                    peer.sendall(b"TLS fixture reply")
                                    self.assertEqual(client.recv(4096), b"TLS fixture reply")

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

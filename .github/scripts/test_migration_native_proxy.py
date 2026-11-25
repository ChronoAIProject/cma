"""Relay preserves bytes and closes both directions on EOF or connection error."""
import socket
import socketserver
import threading
import unittest
from unittest.mock import Mock, patch

from migration_native_proxy import Relay, Server, serve


class NativeProxyTests(unittest.TestCase):
    def test_bidirectional_real_socket_relay_and_join(self):
        client, accepted = socket.socketpair()
        upstream, application = socket.socketpair()
        for connection in (client, accepted, upstream, application):
            connection.settimeout(2)
            self.addCleanup(connection.close)
        errors = []
        def run():
            try:
                Relay(accepted, ('127.0.0.1', 0), None)
            except Exception as error:
                errors.append(error)
        with patch('migration_native_proxy.socket.create_connection', return_value=upstream) as connect:
            worker = threading.Thread(target=run)
            worker.start()
            client.sendall(b'GET /ready HTTP/1.1\r\n\r\n')
            self.assertEqual(application.recv(100), b'GET /ready HTTP/1.1\r\n\r\n')
            application.sendall(b'HTTP/1.1 200 OK\r\n\r\n')
            self.assertEqual(client.recv(100), b'HTTP/1.1 200 OK\r\n\r\n')
            client.shutdown(socket.SHUT_WR)
            self.assertEqual(application.recv(1), b'')
            application.shutdown(socket.SHUT_WR)
            worker.join(3)
            self.assertFalse(worker.is_alive())
            connect.assert_called_once_with(('api', 8080), timeout=10)
        self.assertEqual(errors, [])

    def test_connect_failure_closes_client_and_logs_only_exception_type(self):
        request = Mock()
        with patch('migration_native_proxy.socket.create_connection', side_effect=OSError('private-value')), \
                self.assertLogs('migration_native_proxy', level='WARNING') as logs:
            Relay(request, ('127.0.0.1', 0), None)
        request.close.assert_called_once()
        self.assertNotIn('private-value', ''.join(logs.output))

    def test_socket_errors_and_unjoined_pump_are_explicit(self):
        request, upstream = Mock(), Mock()
        request.recv.side_effect = OSError()
        request.shutdown.side_effect = OSError()
        upstream.recv.side_effect = OSError()
        upstream.shutdown.side_effect = OSError()
        with patch('migration_native_proxy.socket.create_connection', return_value=upstream), \
                self.assertLogs('migration_native_proxy', level='DEBUG'):
            Relay(request, ('127.0.0.1', 0), None)
        upstream.close.assert_called_once()
        request.close.assert_called_once()
        upstream.recv.side_effect = None
        upstream.recv.return_value = b''
        with patch('migration_native_proxy.socket.create_connection', return_value=upstream), \
                patch('migration_native_proxy.threading.Thread') as thread:
            thread.return_value.is_alive.return_value = True
            with self.assertRaisesRegex(RuntimeError, 'not_joined'):
                Relay(request, ('127.0.0.1', 0), None)

    def test_tls_handshake_failure_closes_raw_socket(self):
        server = object.__new__(Server)
        server.context = Mock()
        connection = Mock()
        with patch.object(socketserver.ThreadingTCPServer, 'get_request', return_value=(connection, ('local', 0))):
            self.assertEqual(server.get_request(), (server.context.wrap_socket.return_value, ('local', 0)))
            connection.settimeout.assert_called_with(10)
            server.context.wrap_socket.side_effect = OSError()
            with self.assertRaises(OSError):
                server.get_request()
            connection.close.assert_called_once()

    def test_server_uses_verified_certificate_pair(self):
        with patch('migration_native_proxy.ssl.SSLContext') as context, \
                patch('migration_native_proxy.Server') as server:
            serve('certificate-path', 'key-path')
            context.return_value.load_cert_chain.assert_called_once_with('certificate-path', 'key-path')
            server.assert_called_once_with(('0.0.0.0', 8443), Relay)
            server.return_value.__enter__.return_value.serve_forever.assert_called_once()


if __name__ == '__main__':
    unittest.main()

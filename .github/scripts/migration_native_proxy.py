"""Local TLS termination for the real AM public API, including streaming MCP."""
import logging
import socket
import socketserver
import ssl
import threading

LOG = logging.getLogger(__name__)


class Relay(socketserver.BaseRequestHandler):
    def handle(self):
        upstream = None
        pump = None
        try:
            self.request.settimeout(1800)
            upstream = socket.create_connection(('api', 8080), timeout=10)
            upstream.settimeout(1800)

            def upload():
                try:
                    while data := self.request.recv(65536):
                        upstream.sendall(data)
                except OSError as error:
                    LOG.debug('native_proxy_upload_closed type=%s', type(error).__name__)
                finally:
                    try:
                        upstream.shutdown(socket.SHUT_WR)
                    except OSError as error:
                        LOG.debug('native_proxy_half_close type=%s', type(error).__name__)

            pump = threading.Thread(target=upload, daemon=True)
            pump.start()
            while data := upstream.recv(65536):
                self.request.sendall(data)
        except OSError as error:
            LOG.warning('native_proxy_relay_closed type=%s', type(error).__name__)
        finally:
            for connection in [self.request, upstream]:
                if connection is not None:
                    try:
                        connection.shutdown(socket.SHUT_RDWR)
                    except OSError as error:
                        LOG.debug('native_proxy_close type=%s', type(error).__name__)
                    connection.close()
            if pump:
                pump.join(timeout=2)
                if pump.is_alive():
                    raise RuntimeError('native_proxy_upload_not_joined')


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True

    def get_request(self):
        connection, address = super().get_request()
        connection.settimeout(10)
        try:
            return self.context.wrap_socket(connection, server_side=True), address
        except OSError:
            connection.close()
            raise


def serve(certificate, key):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(certificate, key)
    with Server(('0.0.0.0', 8443), Relay) as server:
        server.context = context
        server.serve_forever()


if __name__ == '__main__':
    serve('/private/provider.crt', '/private/provider.key')

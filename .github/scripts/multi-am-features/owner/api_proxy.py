"""Local TLS termination for the real AM public API, including streaming MCP."""
import socket
import socketserver
import ssl
import threading


class Relay(socketserver.BaseRequestHandler):
    def handle(self):
        upstream = None
        pump = None
        try:
            self.request.settimeout(1800)
            upstream = socket.create_connection(('am-public', 8080), timeout=10)
            upstream.settimeout(1800)

            def upload():
                try:
                    while data := self.request.recv(65536):
                        upstream.sendall(data)
                except OSError:
                    pass
                finally:
                    try:
                        upstream.shutdown(socket.SHUT_WR)
                    except OSError:
                        pass

            pump = threading.Thread(target=upload, daemon=True)
            pump.start()
            while data := upstream.recv(65536):
                self.request.sendall(data)
        except OSError:
            pass
        finally:
            for connection in [self.request, upstream]:
                if connection is not None:
                    try:
                        connection.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    connection.close()
            if pump:
                pump.join(timeout=2)


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


if __name__ == '__main__':
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain('/private/provider.crt', '/private/provider.key')
    with Server(('0.0.0.0', 8443), Relay) as server:
        server.context = context
        server.serve_forever()

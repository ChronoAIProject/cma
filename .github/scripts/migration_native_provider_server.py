"""Expose TLS model traffic and a loopback-only control port over shared state."""
import os
import ssl
import threading
from http.server import ThreadingHTTPServer

from migration_native_provider import Handler


def serve(certificate, key, *, server_type=ThreadingHTTPServer):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(certificate, key)
    # Port-forward can reach this listener; a Kubernetes Service cannot expose
    # it to another Pod. Both listeners share the same bounded journey state.
    with server_type(('127.0.0.1', 8081), Handler) as control:
        with server_type(('0.0.0.0', 8080), Handler) as model:
            model.socket = context.wrap_socket(model.socket, server_side=True)
            worker = threading.Thread(target=control.serve_forever, daemon=True)
            worker.start()
            try:
                model.serve_forever()
            finally:
                control.shutdown()
                worker.join(timeout=5)
                if worker.is_alive():
                    raise RuntimeError('native_provider_control_not_joined')


if __name__ == '__main__':
    serve(os.environ['WS4_PROVIDER_TLS_CERT'], os.environ['WS4_PROVIDER_TLS_KEY'])

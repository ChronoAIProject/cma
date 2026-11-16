#!/usr/bin/env python3
"""Disposable CI admission: bind the exact desktop image to the tested sandbox profile."""
import base64
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import ssl
from urllib.parse import urlsplit

NAMESPACE = 'cma-native-am'
IMAGE = os.environ['DESKTOP_IMAGE']
PROFILE = {'type': 'Localhost', 'localhostProfile': 'cma-ci/desktop-amd64.json'}


def admit(request):
    uid = request['uid']
    response = {'uid': uid, 'allowed': True}
    if (request.get('namespace') != NAMESPACE or request.get('operation') != 'CREATE'
            or request.get('resource') != {'group': '', 'version': 'v1', 'resource': 'pods'}
            or request.get('subResource')):
        return response
    containers = request['object'].get('spec', {}).get('containers', [])
    selected = [(i, c) for i, c in enumerate(containers) if c.get('name') == 'runtime' and c.get('image') == IMAGE]
    if not selected:
        return response
    if len(selected) != 1 or len(containers) != 1:
        return {'uid': uid, 'allowed': False, 'status': {'message': 'unexpected local runtime container topology'}}
    index, runtime = selected[0]
    context = runtime.get('securityContext', {})
    if not (context.get('runAsUser') == 10001 and context.get('runAsNonRoot') is True
            and context.get('allowPrivilegeEscalation') is False and context.get('privileged', False) is False
            and context.get('capabilities', {}).get('drop') == ['ALL']
            and not context.get('capabilities', {}).get('add')):
        return {'uid': uid, 'allowed': False, 'status': {'message': 'local desktop requires original nonroot empty-capability context'}}
    old = context.get('seccompProfile')
    if old == PROFILE:
        return response
    if old != {'type': 'RuntimeDefault'}:
        return {'uid': uid, 'allowed': False, 'status': {'message': 'unexpected existing desktop seccomp policy'}}
    path = f'/spec/containers/{index}/securityContext/seccompProfile'
    patch = [{'op': 'test', 'path': path, 'value': old}, {'op': 'replace', 'path': path, 'value': PROFILE}]
    response.update(patchType='JSONPatch', patch=base64.b64encode(json.dumps(patch, separators=(',', ':')).encode()).decode())
    return response


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.send_response(200 if self.path == '/livez' else 404)
        self.end_headers()

    def do_POST(self):
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if urlsplit(self.path).path != '/mutate' or not 0 < length <= 2 * 1024 * 1024:
                raise ValueError('invalid request framing')
            raw = self.rfile.read(length)
            if len(raw) != length:
                raise ValueError('truncated request')
            review = json.loads(raw)
            if review.get('apiVersion') != 'admission.k8s.io/v1' or review.get('kind') != 'AdmissionReview':
                raise ValueError('unsupported admission review')
            response = admit(review['request'])
            body = json.dumps({'apiVersion': 'admission.k8s.io/v1', 'kind': 'AdmissionReview', 'response': response}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            print(json.dumps({'uid': response['uid'], 'allowed': response['allowed'], 'patched': 'patch' in response}), flush=True)
        except (KeyError, ValueError, TypeError) as error:
            print(json.dumps({'rejected': type(error).__name__, 'reason': str(error), 'path': self.path,
                'content_length': self.headers.get('Content-Length'), 'transfer_encoding': self.headers.get('Transfer-Encoding'),
                'review_type': {k: review.get(k) for k in ('apiVersion', 'kind')} if 'review' in locals() and isinstance(review, dict) else None}), flush=True)
            self.send_error(400, 'invalid admission request')


class Server(ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 4

    def get_request(self):
        sock, address = super().get_request()
        sock.settimeout(5)
        try:
            return self.tls.wrap_socket(sock, server_side=True), address
        except BaseException:
            sock.close()
            raise


if __name__ == '__main__':
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.minimum_version = ssl.TLSVersion.TLSv1_2
    tls.load_cert_chain('/tls/tls.crt', '/tls/tls.key')
    with Server(('0.0.0.0', 8443), Handler) as server:
        server.tls = tls
        server.serve_forever()

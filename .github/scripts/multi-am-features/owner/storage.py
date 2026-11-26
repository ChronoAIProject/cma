"""Bounded storage-service dependency; AM document policy and Mongo remain real."""
from contextlib import closing
import hashlib
import hmac
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
from pathlib import Path
import re
import ssl
import threading
from urllib.parse import parse_qs, urlsplit
import zipfile

PREFIX = '/api/v1/proxy/s/chrono-storage-service/api/buckets/cma-ci-documents/objects'
LIMIT = 2 * 1024 * 1024
SKILL_PATH = re.compile(r'/api/v1/proxy/s/ornn-api/api/v1/skills/'
                        r'(two-am-(?:before|after)-restart)(/versions/1\.0/download)?')
MODEL_RESPONSES_PATH = '/api/v1/proxy/s/chrono-llm-public/responses'


def skill_archive(skill):
    content = io.BytesIO()
    with zipfile.ZipFile(content, 'w') as archive:
        archive.writestr(zipfile.ZipInfo('SKILL.md'),
                         f'---\nname: {skill}\ndescription: Verify a disposable Agent skill.\n---\n')
    return content.getvalue()


class Storage(BaseHTTPRequestHandler):
    config = None
    objects = {}
    lock = threading.Lock()

    def log_message(self, *_):
        pass

    def reply(self, status, body, media_type='application/json'):
        raw = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(status)
        self.send_header('Content-Type', media_type)
        self.send_header('Content-Length', str(len(raw)))
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        self.wfile.write(raw)

    def body(self):
        self.connection.settimeout(5)
        size = int(self.headers.get('Content-Length', '0'))
        if not 0 <= size <= LIMIT:
            raise ValueError('body_bound')
        value = self.rfile.read(size)
        if len(value) != size:
            raise ValueError('incomplete_body')
        return value

    def authorized(self):
        return hmac.compare_digest(self.headers.get('Authorization', ''), 'Bearer ' + self.config['access'])

    def object_key(self):
        values = parse_qs(urlsplit(self.path).query)
        key = values.get('key', [])
        if len(key) != 1 or not re.fullmatch(r'documents/[a-f0-9]{64}/doc_[a-f0-9]{32}/[a-f0-9]{64}\.md', key[0]):
            raise ValueError('content_addressed_key_required')
        return key[0], values

    def do_POST(self):
        try:
            body = self.body()
            if self.path == MODEL_RESPONSES_PATH:
                return self.model_response(body)
            if self.path == '/oauth/token':
                form = parse_qs(body.decode(), strict_parsing=True)
                if (form.get('grant_type') != ['client_credentials']
                        or form.get('client_id') != [self.config['client_id']]
                        or form.get('client_secret') != [self.config['client_secret']]
                        or form.get('scope') not in [['proxy object:create object:read object:download'],
                                                     ['proxy object:read object:download']]):
                    return self.reply(401, {'error': 'invalid_client'})
                return self.reply(200, {'access_token': self.config['access'], 'expires_in': 3600})
            if not self.authorized():
                return self.reply(401, {'error': 'unauthorized'})
            if urlsplit(self.path).path != PREFIX:
                return self.reply(404, {'error': 'unsupported_route'})
            key, query = self.object_key()
            digest = hashlib.sha256(body).hexdigest()
            if (not body or query.get('mutationGeneration') != ['1']
                    or query.get('contentSha256') != [digest]
                    or query.get('contentType') != ['text/markdown; charset=utf-8']
                    or not key.endswith('/' + digest + '.md')):
                return self.reply(409, {'error': 'content_fence_mismatch'})
            with self.lock:
                if key in self.objects and self.objects[key] != body:
                    return self.reply(409, {'error': 'content_fence_mismatch'})
                if key not in self.objects and (len(self.objects) >= 100
                        or sum(map(len, self.objects.values())) + len(body) > 64 * 1024 * 1024):
                    return self.reply(429, {'error': 'fixture_capacity'})
                self.objects[key] = body
            return self.reply(200, {'data': {'key': key, 'contentLength': len(body), 'generation': 1,
                                           'digest': digest, 'fenceVersion': 1}})
        except (UnicodeError, ValueError, KeyError):
            return self.reply(400, {'error': 'invalid_request'})

    def do_GET(self):
        url = urlsplit(self.path)
        if match := SKILL_PATH.fullmatch(url.path):
            return self.skill(match[1], match[2] is not None, url.query)
        if not self.authorized():
            return self.reply(401, {'error': 'unauthorized'})
        if url.path != PREFIX + '/download':
            return self.reply(404, {'error': 'unsupported_route'})
        try:
            key, _ = self.object_key()
            with self.lock:
                data = self.objects.get(key)
            return self.reply(404, {'error': 'missing'}) if data is None else self.reply(200, data, 'text/markdown; charset=utf-8')
        except ValueError:
            return self.reply(400, {'error': 'invalid_request'})

    def check_grant(self, scope):
        connection = http.client.HTTPConnection('identity', 8080, timeout=3)
        try:
            connection.request('GET', f'/fixture/{scope}-grant',
                               headers={'Authorization': self.headers.get('Authorization', '')})
            return 200 if connection.getresponse().status == 200 else 401
        except (OSError, http.client.HTTPException):
            return 503
        finally:
            connection.close()

    def model_response(self, body):
        if len(body) > 20 * 1024 or self.headers.get('Content-Type') != 'application/json':
            return self.reply(400, {'error': 'invalid_suggestion_request'})
        request = json.loads(body)
        if not isinstance(request, dict) or request.get('stream') is not False:
            return self.reply(404, {'error': 'unsupported_route'})
        text = request.get('text')
        output_format = text.get('format') if isinstance(text, dict) else None
        suggestions = isinstance(output_format, dict) and output_format.get('name') == 'contextual_suggestions'
        title = 'text' not in request and request.get('store') is False
        if not (suggestions or title):
            return self.reply(404, {'error': 'unsupported_route'})
        grant = self.check_grant('llm')
        if grant != 200:
            return self.reply(grant, {'error': 'fixture_model_grant_unavailable'})
        connection = http.client.HTTPSConnection('provider.localhost.direct', 8080,
                                                  timeout=45 if suggestions else 10)
        try:
            with closing(connection):
                connection.request('POST', '/v1/responses', body, {'Content-Type': 'application/json'})
                response = connection.getresponse()
                result = response.read(24 * 1024 + 1)
                status = response.status
                media_type = response.getheader('Content-Type')
        except (OSError, http.client.HTTPException):
            return self.reply(503, {'error': 'fixture_model_unavailable'})
        if media_type != 'application/json' or len(result) > 24 * 1024:
            return self.reply(502, {'error': 'invalid_suggestion_result'})
        return self.reply(status, result)

    def skill(self, skill, download, query):
        if query != ('' if download else 'version=1.0'):
            return self.reply(404, {'error': 'unknown_skill_version'})
        grant = self.check_grant('ornn')
        if grant != 200:
            return self.reply(grant, {'error': 'fixture_authority_unavailable' if grant == 503 else 'unauthorized'})
        material = skill_archive(skill)
        if download:
            return self.reply(200, material, 'application/zip')
        return self.reply(200, {'data': {'guid': skill, 'name': skill, 'version': '1.0',
                                       'skillHash': hashlib.sha256(material).hexdigest(), 'isPrivate': False}})


if __name__ == '__main__':
    Storage.config = json.loads(Path('/private/storage.json').read_text())
    server = ThreadingHTTPServer(('0.0.0.0', 8443), Storage)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain('/private/provider.crt', '/private/provider.key')
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()

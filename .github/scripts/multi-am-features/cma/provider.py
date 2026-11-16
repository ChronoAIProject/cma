#!/usr/bin/env python3
"""Local OpenAI Responses SSE backend for the sole WS4 real-runtime journey.

This replaces only the external model service. AM, runtime, durable ownership,
and public response/control routes remain real. Never retain prompt/auth bytes.
"""
import hashlib
import hmac
import json
import os
import select
import ssl
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

LOCK = threading.Lock()
CASES = {}
ACTIVE = None
BOOTSTRAP = {'request_count': 0, 'completed_effect_count': 0, 'disconnect_count': 0,
             'active_streams': 0, 'request_hashes': [], 'response_hashes': [], 'events': [],
             'marker': 'WS4_READY', 'release': threading.Event()}
BOOTSTRAP['release'].set()


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


def arm(case_id, marker):
    global ACTIVE
    if not isinstance(case_id, str) or not 1 <= len(case_id) <= 128:
        raise ValueError('case_id_invalid')
    if not isinstance(marker, str) or not 8 <= len(marker) <= 256:
        raise ValueError('marker_invalid')
    with LOCK:
        if case_id in CASES:
            raise ValueError('case_already_exists')
        CASES[case_id] = {'marker': marker, 'release': threading.Event(),
                         'request_count': 0, 'completed_effect_count': 0,
                         'disconnect_count': 0, 'active_streams': 0,
                         'request_hashes': [], 'response_hashes': [], 'events': []}
        ACTIVE = case_id


def snapshot(case_id):
    with LOCK:
        case = CASES[case_id]
        return {'case_id': case_id,
                **{key: case[key] for key in ('request_count', 'completed_effect_count',
                    'disconnect_count', 'active_streams', 'request_hashes', 'response_hashes', 'events')},
                'released': case['release'].is_set(), 'synthetic_effect': 'one_text_completion'}


def response_events(response_id, model, marker):
    item_id = 'msg_' + digest(response_id)[:24]
    text = 'WS4 complete ' + marker
    item = {'id': item_id, 'type': 'message', 'status': 'completed', 'role': 'assistant',
            'content': [{'type': 'output_text', 'text': text, 'annotations': []}]}
    response = {'id': response_id, 'object': 'response', 'created_at': int(time.time()),
                'status': 'in_progress', 'model': model, 'output': []}
    frames = [
        {'type': 'response.created', 'response': response},
        {'type': 'response.in_progress', 'response': response},
        {'type': 'response.output_item.added', 'output_index': 0,
         'item': {**item, 'status': 'in_progress', 'content': []}},
        {'type': 'response.content_part.added', 'item_id': item_id, 'output_index': 0,
         'content_index': 0, 'part': {'type': 'output_text', 'text': '', 'annotations': []}},
        {'type': 'response.output_text.delta', 'item_id': item_id, 'output_index': 0,
         'content_index': 0, 'delta': text},
        {'type': 'response.output_text.done', 'item_id': item_id, 'output_index': 0,
         'content_index': 0, 'text': text},
        {'type': 'response.content_part.done', 'item_id': item_id, 'output_index': 0,
         'content_index': 0, 'part': item['content'][0]},
        {'type': 'response.output_item.done', 'output_index': 0, 'item': item},
        {'type': 'response.completed', 'response': {**response, 'status': 'completed',
         'output': [item], 'usage': {'input_tokens': 1, 'output_tokens': 1, 'total_tokens': 2}}},
    ]
    return [{**frame, 'sequence_number': i} for i, frame in enumerate(frames)]


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *_args):
        pass

    def peer_closed(self):
        # A TLS peer may send close_notify while retaining its TCP socket.
        # Successful heartbeat writes do not prove that peer is still reading.
        # Read only when ready, and count EOF itself, not socket readability.
        connection = self.connection
        if not select.select([connection], [], [], 0)[0]:
            return False
        timeout = connection.gettimeout()
        try:
            connection.setblocking(False)
            return connection.recv(1) == b''
        except (ssl.SSLWantReadError, ssl.SSLWantWriteError, BlockingIOError):
            return False
        finally:
            connection.settimeout(timeout)

    def reply(self, status, value):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authorized_control(self):
        token = os.environ.get('WS4_PROVIDER_CONTROL_TOKEN', '')
        return len(token) >= 32 and hmac.compare_digest(
            self.headers.get('Authorization', ''), 'Bearer ' + token)

    def authorized_model(self):
        token = os.environ.get('WS4_PROVIDER_BEARER', '')
        return not token or (len(token) >= 32 and hmac.compare_digest(
            self.headers.get('Authorization', ''), 'Bearer ' + token))

    def body(self):
        length = int(self.headers.get('Content-Length', '0'))
        if not 0 < length <= 4 * 1024 * 1024:
            raise ValueError('body_limit')
        return json.loads(self.rfile.read(length))

    def do_GET(self):
        parts = urlsplit(self.path)
        if parts.path == '/health':
            if os.environ.get('WS4_PROVIDER_BEARER') and not (self.authorized_control() or self.authorized_model()):
                return self.reply(401, {'error': 'unauthorized'})
            return self.reply(200, {'ok': True, 'protocol': 'openai_responses_sse'})
        if parts.path == '/__ws4/state':
            if not self.authorized_control():
                return self.reply(401, {'error': 'unauthorized'})
            try:
                return self.reply(200, snapshot(parse_qs(parts.query)['case_id'][0]))
            except (KeyError, IndexError):
                return self.reply(404, {'error': 'case_unknown'})
        if parts.path == '/__ws4/bootstrap':
            if not self.authorized_control():
                return self.reply(401, {'error': 'unauthorized'})
            with LOCK:
                value = {key: BOOTSTRAP[key] for key in ('request_count', 'completed_effect_count', 'disconnect_count', 'active_streams')}
            return self.reply(200, value)
        return self.reply(404, {'error': 'route_unknown'})

    def do_POST(self):
        path = urlsplit(self.path).path
        try:
            body = self.body()
        except (ValueError, TypeError, json.JSONDecodeError):
            return self.reply(400, {'error': 'invalid_body'})
        if path.startswith('/__ws4/'):
            if not self.authorized_control():
                return self.reply(401, {'error': 'unauthorized'})
            try:
                case_id = body['case_id']
                if path == '/__ws4/arm':
                    arm(case_id, body['marker'])
                elif path == '/__ws4/release':
                    with LOCK:
                        CASES[case_id]['release'].set()
                else:
                    return self.reply(404, {'error': 'route_unknown'})
                return self.reply(200, snapshot(case_id))
            except (KeyError, ValueError):
                return self.reply(409, {'error': 'case_conflict'})
        if path not in ('/responses', '/v1/responses'):
            return self.reply(404, {'error': 'route_unknown'})
        if not self.authorized_model():
            return self.reply(401, {'error': 'unauthorized'})
        if body.get('stream') is not True:
            return self.reply(400, {'error': 'stream_required'})
        encoded = json.dumps(body.get('input'), sort_keys=True)
        with LOCK:
            case_id = ACTIVE
            case = CASES.get(case_id)
            if case is None or case['marker'] not in encoded:
                # Real Agent creation can issue its readiness prompt before an
                # explicit journey turn exists. Keep those requests separate.
                case = BOOTSTRAP if os.environ.get('WS4_PROVIDER_ALLOW_BOOTSTRAP') == 'true' else None
                case_id = '__bootstrap__'
            if case is not None:
                case['request_count'] += 1
                ordinal = case['request_count']
                response_id = 'resp_ws4_' + digest(case_id + ':' + str(ordinal))[:24]
                case['request_hashes'].append(digest(encoded))
                case['response_hashes'].append(digest(response_id))
                case['active_streams'] += 1
                case['events'].append({'event': 'request_admitted', 'ordinal': ordinal, 'at_ns': time.time_ns()})
        if case is None:
            return self.reply(409, {'error': 'no_matching_armed_case'})
        frames = response_events(response_id, body.get('model', 'ws4-local'), case['marker'])
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Cache-Control', 'no-cache')
        self.send_header('Transfer-Encoding', 'chunked')
        self.send_header('Connection', 'close')
        self.send_header('x-request-id', response_id)
        self.end_headers()
        def chunk(data):
            self.wfile.write(('%x\r\n' % len(data)).encode() + data + b'\r\n')
            self.wfile.flush()
        def emit(frame):
            chunk(('event: ' + frame['type'] + '\ndata: ' + json.dumps(frame) + '\n\n').encode())
        try:
            for frame in frames[:2]:
                emit(frame)
            deadline = time.monotonic() + 600
            while not case['release'].wait(0.5):
                if self.peer_closed():
                    raise ConnectionResetError('peer_read_eof')
                if time.monotonic() > deadline:
                    raise TimeoutError('held_stream_deadline')
                chunk(b': ws4-held\n\n')
            for frame in frames[2:]:
                emit(frame)
            # A finite HTTP body must finish before TLS teardown. SSE's final
            # event alone does not make close-delimited TLS EOF unambiguous.
            self.wfile.write(b'0\r\n\r\n')
            self.wfile.flush()
            with LOCK:
                case['completed_effect_count'] += 1
                case['events'].append({'event': 'completion_flushed', 'ordinal': ordinal, 'at_ns': time.time_ns()})
        except (BrokenPipeError, ConnectionResetError, OSError, TimeoutError) as error:
            with LOCK:
                case['disconnect_count'] += 1
                case['events'].append({'event': 'stream_disconnected', 'ordinal': ordinal,
                                       'at_ns': time.time_ns(), 'error_type': type(error).__name__,
                                       'peer_read_eof': str(error) == 'peer_read_eof'})
        finally:
            with LOCK:
                case['active_streams'] -= 1
            self.close_connection = True


if __name__ == '__main__':
    server = ThreadingHTTPServer((os.environ.get('WS4_PROVIDER_BIND', '0.0.0.0'),
                                 int(os.environ.get('PORT', '8080'))), Handler)
    cert = os.environ.get('WS4_PROVIDER_TLS_CERT')
    key = os.environ.get('WS4_PROVIDER_TLS_KEY')
    if bool(cert) != bool(key):
        raise RuntimeError('tls_cert_key_pair_required')
    if cert:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(cert, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()

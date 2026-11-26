#!/usr/bin/env python3
"""Local OpenAI Responses SSE backend for the sole WS4 real-runtime journey.

This replaces only the external model service. AM, runtime, durable ownership,
and public response/control routes remain real. Never retain prompt/auth bytes.
"""
import hashlib
import hmac
import json
import os
import re
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


def arm(case_id, marker, access_request=None, suggestions=False, configuration=None, skill=None):
    global ACTIVE
    if not isinstance(case_id, str) or not 1 <= len(case_id) <= 128:
        raise ValueError('case_id_invalid')
    if not isinstance(marker, str) or not 8 <= len(marker) <= 256:
        raise ValueError('marker_invalid')
    if access_request is not None and (not isinstance(access_request, str)
                                       or not re.fullmatch(r'[a-f0-9-]{36}', access_request)):
        raise ValueError('fixture_access_request_key_invalid')
    if type(suggestions) is not bool or (suggestions and access_request is not None):
        raise ValueError('fixture_suggestion_case_invalid')
    if suggestions and not re.fullmatch(r'WS4_SUGGEST_[a-f0-9]{32}', marker):
        raise ValueError('fixture_suggestion_marker_invalid')
    if configuration is not None:
        if (suggestions or access_request is not None or not isinstance(configuration, dict)
                or set(configuration) not in ({'effort'}, {'effort', 'instructions'})
                or configuration['effort'] not in ('minimal', 'low', 'medium', 'high', 'xhigh')
                or ('instructions' in configuration and (not isinstance(configuration['instructions'], str)
                    or not re.fullmatch(r'WS4_INSTRUCTIONS_[a-f0-9]{32}', configuration['instructions'])))):
            raise ValueError('fixture_configuration_invalid')
    if skill is not None:
        if (suggestions or access_request is not None or configuration is not None
                or not isinstance(skill, dict) or set(skill) != {'id', 'present'}
                or skill['id'] not in ('two-am-before-restart', 'two-am-after-restart')
                or type(skill['present']) is not bool):
            raise ValueError('fixture_skill_invalid')
    with LOCK:
        if case_id in CASES:
            raise ValueError('case_already_exists')
        CASES[case_id] = {'marker': marker, 'release': threading.Event(), 'access_request': access_request,
                         'access_call_id': 'call_' + digest(case_id)[:24], 'access_tool_calls': 0,
                         'access_receipt': None,
                         'suggestions': suggestions, 'suggestion_release': threading.Event(),
                         'configuration': configuration, 'configuration_matches': 0,
                         'skill': skill, 'skill_matches': 0,
                         'suggestion_requests': 0, 'suggestion_responses': 0,
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
                'access_tool_calls': case['access_tool_calls'], 'access_receipt': case['access_receipt'],
                'suggestion_requests': case['suggestion_requests'], 'suggestion_responses': case['suggestion_responses'],
                'suggestion_released': case['suggestion_release'].is_set(),
                'configuration_matches': case['configuration_matches'],
                'skill_matches': case['skill_matches'],
                'released': case['release'].is_set(),
                'synthetic_effect': 'one_consent_proposal' if case['access_request'] else 'one_text_completion'}


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


def configuration_matches(expected, body):
    reasoning = body.get('reasoning')
    if not isinstance(reasoning, dict) or reasoning.get('effort') != expected['effort']:
        return False
    marker = expected.get('instructions')
    if marker is None:
        return True
    inputs = body.get('input')
    if not isinstance(inputs, list):
        return False
    # The pinned Codex client projects AGENTS.md as its own structured
    # user-role context message. An ordinary user prompt is a separate item.
    for item in inputs:
        if (not isinstance(item, dict) or item.get('role') != 'user'
                or item.get('type') != 'message' or not isinstance(item.get('content'), list)):
            continue
        for part in item['content']:
            if not isinstance(part, dict) or part.get('type') != 'input_text':
                continue
            text = part.get('text')
            if not isinstance(text, str) or 'AGENTS.md instructions' not in text.split('\n', 1)[0]:
                continue
            _, opened, rest = text.partition('<INSTRUCTIONS>')
            instructions, closed, _ = rest.partition('</INSTRUCTIONS>')
            if opened and closed and 'Two-AM instructions ' + marker in instructions.splitlines():
                return True
    return False


def skill_matches(expected, body):
    inputs = body.get('input')
    if not isinstance(inputs, list):
        return False
    for item in reversed(inputs):
        if not isinstance(item, dict) or item.get('role') != 'developer' or not isinstance(item.get('content'), list):
            continue
        for part in item['content']:
            text = part.get('text') if isinstance(part, dict) else None
            if not isinstance(text, str):
                continue
            _, opened, rest = text.partition('<skills_instructions>')
            catalog, closed, _ = rest.partition('</skills_instructions>')
            if opened and closed:
                pattern = r'(?m)^- ' + re.escape(expected['id']) + r':\s'
                return len(re.findall(pattern, catalog)) == int(expected['present'])
    return False


def access_tool_events(response_id, request, request_key, call_id):
    name = 'mcp__cma_access__agent_key_scope_revision'
    tools = request.get('tools') or [tool for item in request.get('input', [])
                                    if item.get('type') == 'additional_tools' for tool in item.get('tools', [])]
    arguments = {'request_key': request_key, 'services': ['fixture-access-service'],
                 'reason': 'Synthetic consent check ' + request_key}
    item = {'id': 'fc_' + digest(response_id)[:24], 'call_id': call_id, 'status': 'completed'}
    if any(tool.get('name') == name for tool in tools):
        item.update(type='function_call', name=name, arguments=json.dumps(arguments))
    elif any(tool.get('name') == 'functions' and any(child.get('name') == 'exec' for child in tool.get('tools', []))
             for tool in tools):
        invocation = 'const entry = ALL_TOOLS.find(tool => tool.name === ' + json.dumps(name) + ');'
        invocation += 'text(await tools[entry.name](' + json.dumps(arguments) + '));'
        item.update(type='custom_tool_call', name='exec', namespace='functions', input=invocation)
    else:
        raise ValueError('fixture_access_tool_undeclared')
    response = {'id': response_id, 'object': 'response', 'created_at': int(time.time()), 'status': 'in_progress',
                'model': request.get('model', 'ws4-local'), 'output': []}
    frames = [{'type': 'response.created', 'response': response},
              {'type': 'response.in_progress', 'response': response},
              {'type': 'response.output_item.added', 'output_index': 0, 'item': {**item, 'status': 'in_progress'}},
              {'type': 'response.output_item.done', 'output_index': 0, 'item': item},
              {'type': 'response.completed', 'response': {**response, 'status': 'completed', 'output': [item],
                   'usage': {'input_tokens': 1, 'output_tokens': 1, 'total_tokens': 2}}}]
    return [{**frame, 'sequence_number': index} for index, frame in enumerate(frames)]


def access_receipt(output):
    # Codex tool transports can wrap structuredContent in one or more JSON
    # strings. Keep only the request digest and supported consent state.
    value = json.dumps(output).replace('\\', '')
    identifiers = set(re.findall(r'"request_id"\s*:\s*"(aar_[a-f0-9]{64})"', value))
    states = set(re.findall(r'"status"\s*:\s*"(pending|denied)"', value))
    if len(identifiers) != 1 or len(states) != 1:
        return None
    return {'request_sha256': digest(identifiers.pop()), 'status': states.pop()}


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
                    arm(case_id, body['marker'], body.get('access_request'), body.get('suggestions', False),
                        body.get('configuration'), body.get('skill'))
                elif path in ('/__ws4/release', '/__ws4/release-suggestion'):
                    with LOCK:
                        gate = 'release' if path == '/__ws4/release' else 'suggestion_release'
                        CASES[case_id][gate].set()
                else:
                    return self.reply(404, {'error': 'route_unknown'})
                return self.reply(200, snapshot(case_id))
            except (KeyError, ValueError):
                return self.reply(409, {'error': 'case_conflict'})
        if path not in ('/responses', '/v1/responses'):
            return self.reply(404, {'error': 'route_unknown'})
        if not self.authorized_model():
            return self.reply(401, {'error': 'unauthorized'})
        text = body.get('text')
        format_spec = text.get('format') if isinstance(text, dict) else None
        if (body.get('stream') is False and isinstance(format_spec, dict)
                and format_spec.get('name') == 'contextual_suggestions'):
            return self.suggestion(body)
        if body.get('stream') is not True:
            return self.reply(400, {'error': 'stream_required'})
        encoded = json.dumps(body.get('input'), sort_keys=True)
        frames = None
        with LOCK:
            case_id = ACTIVE
            case = CASES.get(case_id)
            tool_result = None
            if case and case['access_request'] and case['request_count'] == 1 and isinstance(body.get('input'), list):
                tool_result = next((item for item in body['input'] if isinstance(item, dict)
                                    and item.get('type') in ('function_call_output', 'custom_tool_call_output')
                                    and item.get('call_id') == case['access_call_id']), None)
            if case and case['access_request'] and case['marker'] in encoded and case['request_count'] == 0:
                try:
                    frames = access_tool_events('resp_ws4_' + digest(case_id + ':1')[:24], body,
                                                case['access_request'], case['access_call_id'])
                except (ValueError, TypeError, AttributeError):
                    return self.reply(409, {'error': 'fixture_access_tool_undeclared'})
                case['access_tool_calls'] += 1
            elif tool_result:
                case['access_receipt'] = access_receipt(tool_result.get('output'))
            elif case and case['access_request'] and case['marker'] in encoded:
                return self.reply(409, {'error': 'fixture_access_tool_output_missing'})
            elif case is None or case['marker'] not in encoded:
                # Real Agent creation can issue its readiness prompt before an
                # explicit journey turn exists. Keep those requests separate.
                case = BOOTSTRAP if os.environ.get('WS4_PROVIDER_ALLOW_BOOTSTRAP') == 'true' else None
                case_id = '__bootstrap__'
            if case is not None:
                case['request_count'] += 1
                expected = case.get('configuration')
                if expected is not None and configuration_matches(expected, body):
                    case['configuration_matches'] += 1
                skill = case.get('skill')
                if skill is not None and skill_matches(skill, body):
                    case['skill_matches'] += 1
                ordinal = case['request_count']
                response_id = 'resp_ws4_' + digest(case_id + ':' + str(ordinal))[:24]
                case['request_hashes'].append(digest(encoded))
                case['response_hashes'].append(digest(response_id))
                case['active_streams'] += 1
                case['events'].append({'event': 'request_admitted', 'ordinal': ordinal, 'at_ns': time.time_ns()})
        if case is None:
            return self.reply(409, {'error': 'no_matching_armed_case'})
        frames = frames or response_events(response_id, body.get('model', 'ws4-local'), case['marker'])
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

    def suggestion(self, body):
        messages = body.get('input')
        if not isinstance(messages, list) or len(messages) != 1 or not isinstance(messages[0], dict):
            return self.reply(400, {'error': 'invalid_suggestion_input'})
        try:
            answer = json.loads(messages[0].get('content', '')).get('answer')
        except (TypeError, ValueError, AttributeError):
            return self.reply(400, {'error': 'invalid_suggestion_context'})
        if not isinstance(answer, str):
            return self.reply(400, {'error': 'invalid_suggestion_answer'})
        marker = re.search(r'\bWS4_SUGGEST_[a-f0-9]{32}\b', answer)
        items = []
        if marker:
            with LOCK:
                case = next((case for case in CASES.values()
                             if case['suggestions'] and case['marker'] == marker[0]), None)
                if case:
                    case['suggestion_requests'] += 1
                    duplicate = case['suggestion_requests'] != 1
            if not case:
                return self.reply(409, {'error': 'no_matching_suggestion_case'})
            if duplicate:
                return self.reply(409, {'error': 'duplicate_suggestion_generation'})
            if not case['suggestion_release'].wait(40):
                return self.reply(504, {'error': 'suggestion_barrier_timeout'})
            items.append({'text': 'Explain the fixture result in one sentence.', 'evidence': marker[0]})
            with LOCK:
                case['suggestion_responses'] += 1
        content = {'type': 'output_text', 'text': json.dumps({'suggestions': items})}
        return self.reply(200, {'status': 'completed', 'model': body.get('model', 'ws4-local'),
            'output': [{'type': 'message', 'role': 'assistant', 'content': [content]}],
            'usage': {'input_tokens': 1, 'output_tokens': 1}})


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

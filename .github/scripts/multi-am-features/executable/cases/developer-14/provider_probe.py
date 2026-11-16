"""Observational extension of the existing held-response provider.

Run only as the sole parent's selected provider command in a successor fixture.
AM/runtime replies and hold/release behavior remain owned by the original
handler. Only allowlisted model metadata and hashes leave this probe.
"""
import hashlib
import importlib.util
import os
from pathlib import Path
import re
import ssl
from urllib.parse import parse_qs, urlsplit

SOURCE = Path(__file__).resolve().parents[3] / 'cma/provider.py'
spec = importlib.util.spec_from_file_location('configuration_original_provider', SOURCE)
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
METADATA = {}


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()


def text_parts(value):
    if isinstance(value, str):
        return [value]
    if isinstance(value, list):
        return [part for item in value for part in text_parts(item)]
    if isinstance(value, dict):
        return text_parts(value.get('text', value.get('content', [])))
    return []


def metadata(body, case_id):
    instructions = text_parts(body.get('instructions'))
    input_items = body.get('input')
    if isinstance(input_items, list):
        for item in input_items:
            if isinstance(item, dict) and item.get('role') in ('system', 'developer'):
                instructions.extend(text_parts(item.get('content')))
    model = body.get('model')
    effort = body.get('reasoning', {}).get('effort') if isinstance(body.get('reasoning'), dict) else None
    return {'case_id_sha256': digest(case_id),
            'instruction_sha256': sorted({digest(x) for x in instructions}),
            'instruction_marker_sha256': sorted({digest(marker) for text in instructions
                for marker in re.findall(r'PROMPT-[a-f0-9-]{36}', text)}),
            'model': model if isinstance(model, str) and re.fullmatch(r'[-a-zA-Z0-9_./:]{1,160}', model) else None,
            'reasoning_effort': effort if effort in ('minimal', 'low', 'medium', 'high', 'xhigh', 'x_high') else None}


class Handler(base.Handler):
    def body(self):
        body = super().body()
        if urlsplit(self.path).path in ('/responses', '/v1/responses') and self.authorized_model():
            # Match the real armed request exactly as the original provider does.
            encoded = base.json.dumps(body.get('input'), sort_keys=True)
            with base.LOCK:
                case_id = base.ACTIVE
                case = base.CASES.get(case_id)
                if case is not None and case['marker'] in encoded:
                    METADATA.setdefault(case_id, []).append(metadata(body, case_id))
        return body

    def do_GET(self):
        path = urlsplit(self.path)
        if path.path == '/__ws4/configuration-metadata':
            if not self.authorized_control():
                return self.reply(401, {'error': 'unauthorized'})
            case_id = parse_qs(path.query).get('case_id', [None])[0]
            with base.LOCK:
                rows = METADATA.get(case_id)
                return self.reply(404, {'error': 'case_unknown'}) if rows is None else self.reply(200, list(rows))
        return super().do_GET()


def main():
    server = base.ThreadingHTTPServer((os.environ.get('WS4_PROVIDER_BIND', '0.0.0.0'),
                                      int(os.environ.get('PORT', '8080'))), Handler)
    cert, key = os.environ.get('WS4_PROVIDER_TLS_CERT'), os.environ.get('WS4_PROVIDER_TLS_KEY')
    if bool(cert) != bool(key):
        raise RuntimeError('tls_cert_key_pair_required')
    if cert:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.minimum_version = ssl.TLSVersion.TLSv1_2
        context.load_cert_chain(cert, key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == '__main__':
    main()

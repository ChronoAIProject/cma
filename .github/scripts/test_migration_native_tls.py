"""Real in-memory TLS handshakes plus deterministic in-cluster probe behavior."""
from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import socket
import ssl
import subprocess
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

from migration_native_config import validate_tls
from migration_native_process import NativeProcessError
from migration_native_readiness import TLS_PROBE


class NativeTlsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()

    def pair(self, hosts, suffix=''):
        paths = {'certificate': str(self.root/('certificate'+suffix+'.pem')),
                 'private_key': str(self.root/('key'+suffix+'.pem'))}
        # Runtime-generated synthetic key material is removed by addCleanup;
        # no fixture private key bytes enter source, output or version control.
        config = self.root/('openssl'+suffix+'.cnf')
        config.write_text('[req]\ndistinguished_name=dn\nx509_extensions=extensions\nprompt=no\n'
                          '[dn]\nCN=migration-fixture\n[extensions]\nsubjectAltName='+
                          ','.join('DNS:'+name for name in hosts)+'\n')
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
                        '-keyout', paths['private_key'], '-out', paths['certificate'], '-config', str(config)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
        return paths

    def test_actual_certificate_key_hostname_handshake_and_expiry_budget(self):
        pair = self.pair(['api.localhost.direct', 'provider.localhost.direct'])
        validate_tls(pair)
        with patch('time.time', return_value=time.time()+23*3600):
            with self.assertRaisesRegex(NativeProcessError, 'validity_window'): validate_tls(pair)
        other = self.pair(['api.localhost.direct'], '-other')
        with self.assertRaises(ssl.SSLCertVerificationError): validate_tls(other)
        with self.assertRaises(ssl.SSLError): validate_tls({**pair, 'private_key': other['private_key']})

    def probe(self, *, dns=None, body=None, status=200, changed_url=False):
        opener = Mock()
        def response(request, timeout):
            self.assertEqual(timeout, 15)
            value = Mock();value.status = status
            value.url = 'https://foreign.invalid' if changed_url else request.full_url
            value.read.return_value = body if body is not None else (
                b'{"protocol":"openai_responses_sse"}' if request.full_url.endswith('/health') else b'ok')
            value.__enter__ = Mock(return_value=value);value.__exit__ = Mock(return_value=False)
            return value
        opener.open.side_effect = response
        addresses = {'api.localhost.direct': '10.96.0.10', 'provider.localhost.direct': '10.96.0.11'}
        def resolved(host, _port):
            return [(None, None, None, None, (addresses[host], 0))]
        output = io.StringIO()
        with patch.dict(os.environ, {'NATIVE_API_SERVICE_IP': addresses['api.localhost.direct'],
                                     'NATIVE_PROVIDER_SERVICE_IP': addresses['provider.localhost.direct']}), \
                patch('socket.getaddrinfo', side_effect=dns or resolved), \
                patch('urllib.request.build_opener', return_value=opener) as factory, \
                patch('time.sleep') as sleep, redirect_stdout(output):
            exec(TLS_PROBE, {})
        handlers = factory.call_args.args
        self.assertEqual(handlers[0].proxies, {})
        self.assertIsNone(handlers[1]().redirect_request(None, None, None, None, None, None))
        context = handlers[2]._context
        self.assertTrue(context.check_hostname)
        self.assertEqual(context.verify_mode, ssl.CERT_REQUIRED)
        return json.loads(output.getvalue()), sleep.call_count

    def test_probe_verifies_system_trust_without_proxy_or_redirect_override(self):
        result, retries = self.probe()
        self.assertEqual(retries, 0)
        self.assertEqual(len(result['checks']), 2)
        self.assertTrue(all(row['verified_tls'] for row in result['checks']))

    def test_probe_dns_and_response_failures_do_not_pass(self):
        with self.assertRaisesRegex(RuntimeError, 'dns_reload'):
            self.probe(dns=Mock(side_effect=socket.gaierror('fixture dns failure')))
        for kwargs in ({'body': b'x'*4097}, {'status': 503}, {'changed_url': True}, {'body': b'{}'}):
            with self.subTest(kwargs=list(kwargs)):
                with self.assertRaises(RuntimeError): self.probe(**kwargs)


if __name__ == '__main__': unittest.main()

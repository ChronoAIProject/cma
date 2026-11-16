import copy
from email.message import Message
import json
import unittest
from browser_session import capture_browser_session


def headers(**values):
    result = Message()
    for key, value in values.items():
        result[key] = value
    return result


class BrowserSessionTests(unittest.TestCase):
    def setUp(self):
        self.calls = []
        self.evidence = []
        self.cookie = 'cma_session=private-test-cookie; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=3600'
        self.topo = {'ams': {am: {'origin': 'http://127.0.0.1:' + port, 'pod_uid': am+'-uid',
            'instance_id': am+'-instance', 'container_id': am+'-container', 'ready': True}
            for am, port in [('a', '10001'), ('b', '10002')]}}
        self.subject = 'expected-actor'
        self.request_id = 'request-id'
        self.replace = False
        fixture = self

        class Client:
            def http_raw(self, origin, method, path, **kwargs):
                fixture.calls.append((origin, method, path, kwargs))
                if path.startswith('/auth/callback'):
                    return 302, headers(**{'Set-Cookie': fixture.cookie}), b''
                if fixture.replace:
                    fixture.topo['ams']['a']['pod_uid'] = 'new-pod'
                return 200, headers(**{'x-request-id': fixture.request_id}), json.dumps({
                    'authenticated': True, 'subject': fixture.subject}).encode()

        self.client = Client()

    def invoke(self, login=None):
        def real_flow_double(client, origin, **kwargs):
            client.http_raw(origin, 'GET', '/auth/callback?code=single-use')
        return capture_browser_session(login=login or real_flow_double, client=self.client,
            actor='userweb', subject='expected-actor', topology=lambda: copy.deepcopy(self.topo),
            retain=self.evidence.append)

    def test_cookie_on_both_members_no_bearer_and_no_secret_in_evidence(self):
        result = self.invoke()
        self.assertEqual(len(self.calls), 3)
        self.assertEqual([r['serving']['am'] for r in self.evidence], ['a', 'b'])
        self.assertTrue(all(r['verdict'] == 'PASS' for r in self.evidence))
        self.assertEqual(result['storageState']['cookies'][0]['value'], 'private-test-cookie')
        self.assertTrue(result['storageState']['cookies'][0]['secure'])
        for call in self.calls[1:]:
            self.assertNotIn('Authorization', call[3]['headers'])
            self.assertEqual(call[3]['headers']['Cookie'], 'cma_session=private-test-cookie')
        self.assertNotIn('private-test-cookie', json.dumps(self.evidence))

    def test_wrong_actor_rejected_after_one_verification(self):
        self.subject = 'different-actor'
        with self.assertRaisesRegex(ValueError, 'identity_or_dispatch'):
            self.invoke()
        self.assertEqual(len(self.calls), 2)
        self.assertEqual(self.evidence[0]['verdict'], 'UNPROVED')

    def test_replaced_process_rejected_without_replay(self):
        self.replace = True
        with self.assertRaisesRegex(ValueError, 'identity_or_dispatch'):
            self.invoke()
        self.assertEqual(len(self.calls), 2)
        self.assertEqual(self.evidence[0]['attribution']['verdict'], 'UNPROVED')

    def test_missing_request_id_rejected(self):
        self.request_id = ''
        with self.assertRaisesRegex(ValueError, 'identity_or_dispatch'):
            self.invoke()
        self.assertEqual(len(self.calls), 2)

    def test_deleted_or_missing_cookie_rejected_before_session_requests(self):
        for cookie in ['cma_session=deleted; Path=/; HttpOnly; Max-Age=0', 'cma_login_state=bound; Path=/']:
            with self.subTest(cookie=cookie):
                self.calls.clear();self.cookie = cookie
                with self.assertRaisesRegex(ValueError, 'issued_cookie'):
                    self.invoke()
                self.assertEqual(len(self.calls), 1)

    def test_ambiguous_callbacks_rejected(self):
        def double(client, origin, **kwargs):
            for _ in range(2):
                client.http_raw(origin, 'GET', '/auth/callback?code=single-use')
        with self.assertRaisesRegex(ValueError, 'exact_callback'):
            self.invoke(double)
        self.assertEqual(len(self.calls), 2)

    def test_nonlocal_dispatch_rejected_before_login(self):
        self.topo['ams']['a']['origin'] = 'https://production.invalid'
        with self.assertRaisesRegex(ValueError, 'local_pod_forward'):
            self.invoke()
        self.assertFalse(self.calls)


if __name__ == '__main__':
    unittest.main()

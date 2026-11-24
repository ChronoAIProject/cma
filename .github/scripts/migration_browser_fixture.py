"""Real PKCE browser custody across selected AM images; no database injection."""
from http.cookies import SimpleCookie
import json
import os
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlsplit, urlunsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

OWNER = 'native-am-fixture-owner'
PUBLIC = 'https://api.localhost.direct'
COLLECTIONS = ['auth_browser_logins', 'auth_login_attempts', 'auth_subjects']
SHAPES = ['linked-browser-session', 'encrypted-subject-custody', 'pending-pkce-attempt',
          'revoked-browser-session']


def local_origin(value):
    parsed = urlsplit(value)
    if (parsed.scheme != 'http' or parsed.hostname != '127.0.0.1'
            or not parsed.port or parsed.username or parsed.password
            or parsed.path or parsed.query or parsed.fragment):
        raise ValueError('fixture requires a local HTTP port-forward origin')
    return value


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, msg, headers, new_url):
        return None


class BrowserFixture:
    defer_pending_callback = False

    def __init__(self, api, identity, state_file):
        self.api, self.identity = local_origin(api), local_origin(identity)
        self.state_file = Path(state_file)
        self.cookies = {}
        self.revoked_cookies = {}
        self.observations = []
        self.pending_callback = None
        self.opener = build_opener(NoRedirect)

    def raw(self, method, url, headers=None):
        try:
            with self.opener.open(Request(url, method=method, headers=headers or {}), timeout=30) as response:
                return response.status, response.headers, response.read()
        except HTTPError as error:
            return error.code, error.headers, error.read()

    def api_request(self, path, cookies=None, expected=(200,)):
        jar = self.cookies if cookies is None else cookies
        headers = {'Host': 'api.localhost.direct', 'Origin': PUBLIC,
                   'Cookie': '; '.join(name + '=' + value for name, value in jar.items())}
        status, response, body = self.raw('GET', self.api + path, headers)
        self.observations.append({'route': urlsplit(path).path, 'status': status,
                                  'request_id': response.get('x-request-id')})
        if status not in expected:
            raise RuntimeError('browser_route_failed_' + urlsplit(path).path + '_' + str(status))
        return response, body

    @staticmethod
    def remember(headers, jar):
        parsed = SimpleCookie()
        for value in headers.get_all('Set-Cookie', []):
            parsed.load(value)
        for name, item in parsed.items():
            if item.value:
                jar[name] = item.value
            else:
                jar.pop(name, None)

    def begin(self):
        headers, _ = self.api_request('/auth/login', cookies={}, expected=(302, 303))
        authorization = urlsplit(headers.get('Location', ''))
        if (authorization.scheme != 'http' or authorization.netloc != 'identity:8080'
                or authorization.path != '/authorize' or not authorization.query or authorization.fragment):
            raise RuntimeError('provider_authorization_redirect_mismatch')
        cookies = {}
        self.remember(headers, cookies)
        if not any(name.startswith('cma_login_') for name in cookies):
            raise RuntimeError('provider_browser_binding_missing')
        return {'authorization': authorization.geturl(), 'cookies': cookies}

    def complete(self, attempt, check_once=False):
        authorization = urlsplit(attempt['authorization'])
        if authorization.scheme != 'http' or authorization.netloc != 'identity:8080' or authorization.path != '/authorize':
            raise RuntimeError('retained_authorization_redirect_mismatch')
        target = self.identity + authorization.path + '?' + authorization.query
        status, response, _ = self.raw('GET', target)
        if status != 302:
            raise RuntimeError('provider_authorization_failed_' + str(status))
        callback = urlsplit(response.get('Location', ''))
        if (callback.scheme != 'https' or callback.netloc != 'api.localhost.direct'
                or callback.path != '/auth/callback' or not callback.query or callback.fragment):
            raise RuntimeError('provider_callback_redirect_mismatch')
        path = urlunsplit(('', '', callback.path, callback.query, ''))
        jar = dict(attempt['cookies'])
        headers, _ = self.api_request(path, cookies=jar, expected=(302, 303))
        self.remember(headers, jar)
        if not jar.get('cma_session'):
            raise RuntimeError('provider_session_missing')
        self.cookies = jar
        if check_once:
            _, body = self.api_request(path, cookies=attempt['cookies'], expected=(400,))
            if json.loads(body).get('code') != 'login_state_invalid':
                raise RuntimeError('consumed_login_attempt_not_rejected')
        return self.check_session()

    def check_session(self):
        _, body = self.api_request('/auth/session')
        value = json.loads(body)
        if value.get('authenticated') is not True or value.get('subject') != OWNER:
            raise RuntimeError('browser_subject_changed')
        return {key: value.get(key) for key in ('authenticated', 'subject', 'capabilities', 'visitor')}

    def check_revoked_session(self):
        # A fresh login can refresh the same subject's durable tokens. It must
        # never revive a prior browser session, including after an image switch.
        if (not isinstance(self.revoked_cookies, dict)
                or not self.revoked_cookies.get('cma_session')
                or self.revoked_cookies['cma_session'] == self.cookies.get('cma_session')):
            raise RuntimeError('revoked_browser_fixture_missing_or_live')
        _, body = self.api_request('/auth/session', cookies=self.revoked_cookies, expected=(401,))
        value = json.loads(body)
        if not isinstance(value, dict) or value.get('authenticated') is not False:
            raise RuntimeError('revoked_browser_session_not_anonymous')

    def revoke_session(self):
        self.revoked_cookies = dict(self.cookies)
        headers, _ = self.api_request('/auth/logout', expected=(303,))
        cleared = dict(self.cookies)
        self.remember(headers, cleared)
        if headers.get('Location') != '/' or cleared.get('cma_session'):
            raise RuntimeError('browser_logout_contract_mismatch')
        self.cookies = {}
        self.check_revoked_session()

    def start(self, stage):
        if stage in ('old-write', 'new-write'):
            self.complete(self.begin())
            self.revoke_session()
            self.complete(self.begin())
        else:
            state = json.loads(self.state_file.read_text())
            writer = 'old-write' if stage == 'new-read-old-write' else 'new-write'
            if state['writer'] != writer:
                raise RuntimeError('browser_writer_identity_mismatch')
            self.cookies = state['session_cookies']
            self.revoked_cookies = state['revoked_session_cookies']
            self.check_revoked_session()
            actual = self.check_session()
            if actual != state['session']:
                raise RuntimeError('retained_browser_authority_changed')
            if self.defer_pending_callback:
                # Deferred commands bind the original sign-in incarnation.
                # Verify them before a successful callback rotates that scope.
                self.pending_callback = state['pending']
            else:
                self.complete(state['pending'], check_once=True)
        return self.cookies

    def finish(self, stage):
        if self.pending_callback is not None:
            self.complete(self.pending_callback, check_once=True)
            self.pending_callback = None
        session = self.check_session()
        self.check_revoked_session()
        if stage in ('old-write', 'new-write'):
            # Leave the encrypted verifier and browser reservation in Mongo.
            # The next AM must consume them using the original binding cookie.
            pending = self.begin()
            state = {'writer': stage, 'session_cookies': self.cookies,
                     'revoked_session_cookies': self.revoked_cookies,
                     'session': session, 'pending': pending}
            with os.fdopen(os.open(self.state_file, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600), 'w') as output:
                os.fchmod(output.fileno(), 0o600)
                output.write(json.dumps(state))
        return {'session_owner_preserved': True,
                'revoked_session_rejected': True,
                'writer_login_or_retained_callback': True,
                'consumed_attempt_rejected': stage not in ('old-write', 'new-write')}

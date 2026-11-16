"""Real PKCE browser custody across selected AM images; no database injection."""
from http.cookies import SimpleCookie
import json
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlsplit, urlunsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

OWNER = 'native-am-fixture-owner'
PUBLIC = 'https://api.localhost.direct'
COLLECTIONS = ['auth_browser_logins', 'auth_login_attempts', 'auth_subjects']
SHAPES = ['linked-browser-session', 'encrypted-subject-custody', 'pending-pkce-attempt']


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
    def __init__(self, api, identity, state_file):
        self.api, self.identity = local_origin(api), local_origin(identity)
        self.state_file = Path(state_file)
        self.cookies = {}
        self.observations = []
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

    def start(self, stage):
        if stage in ('old-write', 'new-write'):
            self.complete(self.begin())
        else:
            state = json.loads(self.state_file.read_text())
            writer = 'old-write' if stage == 'new-read-old-write' else 'new-write'
            if state['writer'] != writer:
                raise RuntimeError('browser_writer_identity_mismatch')
            self.cookies = state['session_cookies']
            actual = self.check_session()
            if actual != state['session']:
                raise RuntimeError('retained_browser_authority_changed')
            self.complete(state['pending'], check_once=True)
        return self.cookies

    def finish(self, stage):
        session = self.check_session()
        if stage in ('old-write', 'new-write'):
            # Leave the encrypted verifier and browser reservation in Mongo.
            # The next AM must consume them using the original binding cookie.
            pending = self.begin()
            state = {'writer': stage, 'session_cookies': self.cookies,
                     'session': session, 'pending': pending}
            self.state_file.write_text(json.dumps(state))
            self.state_file.chmod(0o600)
        return {'session_owner_preserved': True,
                'writer_login_or_retained_callback': True,
                'consumed_attempt_rejected': stage not in ('old-write', 'new-write')}

"""Real AM browser-bound PKCE callback establishes renewable owner custody."""
from http.cookies import SimpleCookie
from urllib.parse import urlsplit
from common import utc, write


def login(client, am_origin, public_host='native-am.local.test', *, mark_stage,
          admin_bearer=None, expected_subject=None):
    headers = {'Host': public_host}
    mark_stage('am_login')
    status, first, _ = client.http_raw(am_origin, 'GET', '/auth/login', headers=headers)
    if status not in [302, 303, 307]:
        raise RuntimeError('am_login_refused_' + str(status))
    authorization = urlsplit(first.get('Location', ''))
    if authorization.scheme != 'http' or authorization.netloc != 'identity:8080' or authorization.path != '/authorize':
        raise RuntimeError('local_authorization_redirect_mismatch')
    cookies = SimpleCookie()
    for value in first.get_all('Set-Cookie', []):
        cookies.load(value)
    binding = '; '.join(k + '=' + v.value for k, v in cookies.items() if k.startswith('cma_login_'))
    if not binding:
        raise RuntimeError('real_browser_binding_missing')
    mark_stage('identity_forward')
    identity_origin = client.forward('identity', 8080)
    mark_stage('identity_authorize')
    authorize_path = authorization.path + '?' + authorization.query
    if admin_bearer is None:
        status, second, _ = client.http_raw(identity_origin, 'GET', authorize_path)
    else:
        status, second, _ = client.http_raw(identity_origin, 'GET', authorize_path,
                                            headers={'Authorization': 'Bearer ' + admin_bearer})
    if status != 302:
        raise RuntimeError('identity_authorization_refused')
    callback = urlsplit(second.get('Location', ''))
    if callback.scheme != 'https' or callback.netloc != public_host or callback.path != '/auth/callback':
        raise RuntimeError('local_callback_mismatch')
    mark_stage('am_callback')
    callback_status, final, _ = client.http_raw(am_origin, 'GET', callback.path + '?' + callback.query,
                                               headers={**headers, 'Cookie': binding})
    if callback_status not in [302, 303] or not final.get_all('Set-Cookie', []):
        raise RuntimeError('real_am_callback_failed_' + str(callback_status))
    session = SimpleCookie()
    for value in final.get_all('Set-Cookie', []):
        session.load(value)
    cookie = session.get('cma_session')
    if (cookie is None or not cookie.value or not cookie['httponly'] or not cookie['secure']
            or cookie['path'] != '/' or cookie['samesite'].lower() != 'lax'
            or not cookie['max-age'].isdigit() or int(cookie['max-age']) <= 0):
        raise RuntimeError('real_browser_session_cookie_missing')
    if expected_subject is not None:
        mark_stage('am_session_subject')
        status, body = client.http(am_origin, 'GET', '/auth/session',
                                   headers={**headers, 'Cookie': 'cma_session=' + cookie.value})
        if status != 200 or not body or body.get('authenticated') is not True or body.get('subject') != expected_subject:
            raise RuntimeError('real_browser_session_subject_mismatch')
    evidence = 'owner-login.json' if admin_bearer is None else 'document-recipient-login.json'
    write(client.output / evidence, {'at': utc(), 'flow': 'real_am_browser_pkce_callback',
        'identity_provider': 'local_dependency', 'authorization_code_used_once': True,
        'callback_status': callback_status, 'secret_cookies_and_code_retained': False,
        'database_authority_injection': False})
    return cookie.value

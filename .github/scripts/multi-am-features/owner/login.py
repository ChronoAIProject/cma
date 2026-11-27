"""Real AM browser-bound PKCE callback establishes renewable owner custody."""
from http.cookies import SimpleCookie
from urllib.parse import urlsplit
from common import utc, write


def login(client, am_origin, public_host='native-am.local.test', *, mark_stage):
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
    status, second, _ = client.http_raw(identity_origin, 'GET', authorization.path + '?' + authorization.query)
    if status != 302:
        raise RuntimeError('identity_authorization_refused')
    callback = urlsplit(second.get('Location', ''))
    if callback.scheme != 'https' or callback.netloc != public_host or callback.path != '/auth/callback':
        raise RuntimeError('local_callback_mismatch')
    mark_stage('am_callback')
    status, final, _ = client.http_raw(am_origin, 'GET', callback.path + '?' + callback.query,
                                      headers={**headers, 'Cookie': binding})
    if status not in [302, 303] or not final.get_all('Set-Cookie', []):
        raise RuntimeError('real_am_callback_failed_' + str(status))
    session = SimpleCookie()
    for value in final.get_all('Set-Cookie', []):
        session.load(value)
    cookie = session.get('cma_session')
    if (cookie is None or not cookie.value or not cookie['httponly'] or not cookie['secure']
            or cookie['path'] != '/' or cookie['samesite'].lower() != 'lax'
            or not cookie['max-age'].isdigit() or int(cookie['max-age']) <= 0):
        raise RuntimeError('real_browser_session_cookie_missing')
    write(client.output / 'owner-login.json', {'at': utc(), 'flow': 'real_am_browser_pkce_callback',
        'identity_provider': 'local_dependency', 'authorization_code_used_once': True,
        'callback_status': status, 'secret_cookies_and_code_retained': False,
        'database_authority_injection': False})
    return cookie.value

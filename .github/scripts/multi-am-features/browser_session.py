"""Capture a real AM PKCE cookie for a later parent-owned browser lease.

Call only inside the admitted parent, serially per actor. Credentials stay in
the returned in-memory lease; the evidence callback receives no cookie value.
This module launches no processes and never injects database authority.
"""
from datetime import datetime, timezone
from http.cookies import SimpleCookie
import json
import time
from urllib.parse import urlsplit


def _stamp():
    return datetime.now(timezone.utc).isoformat()


def _binding(topology, am):
    pod = topology["ams"][am]
    fields = ("origin", "pod_uid", "instance_id", "container_id")
    if pod.get("ready") is not True or pod.get("draining") is True:
        raise ValueError("session_serving_not_ready")
    if not all(isinstance(pod.get(k), str) and pod[k].strip() for k in fields):
        raise ValueError("session_serving_identity_missing")
    url = urlsplit(pod["origin"])
    if (url.scheme != "http" or url.hostname not in ("127.0.0.1", "localhost")
            or url.username or url.password or url.path not in ("", "/")
            or url.query or url.fragment):
        raise ValueError("session_local_pod_forward_required")
    return {"schema": "c275-serving-v1", "am": am, **{k: pod[k] for k in fields},
            "ready": True, "observed_at": _stamp()}


def capture_browser_session(*, login, client, actor, subject, topology, retain):
    """Reuse owner/login.py, then verify the same cookie on both current AMs.

    An ambiguous callback or changed serving identity is terminal for this
    attempt. No authorization code, callback, or accepted request is replayed.
    """
    if not isinstance(actor, str) or not actor or not isinstance(subject, str) or not subject:
        raise ValueError("session_actor_binding_required")
    before = _binding(topology(), "a")
    callbacks = []

    class Capture:
        def __getattr__(self, name):
            return getattr(client, name)

        def http_raw(self, origin, method, path, **kwargs):
            response = client.http_raw(origin, method, path, **kwargs)
            if urlsplit(path).path == "/auth/callback":
                callbacks.append(response)
            return response

    login(Capture(), before["origin"], public_host="api.localhost.direct", actor=actor)
    if len(callbacks) != 1 or callbacks[0][0] not in (302, 303):
        raise ValueError("session_exact_callback_required")
    cookies = SimpleCookie()
    for header in callbacks[0][1].get_all("Set-Cookie", []):
        cookies.load(header)
    cookie = cookies.get("cma_session")
    if (cookie is None or not cookie.value or not cookie["httponly"]
            or cookie["path"] != "/" or not cookie["max-age"].isdigit()
            or int(cookie["max-age"]) <= 0):
        raise ValueError("session_valid_issued_cookie_required")
    expires = time.time() + int(cookie["max-age"])
    for am in ("a", "b"):
        serving = _binding(topology(), am)
        started = _stamp()
        status, headers, raw = client.http_raw(serving["origin"], "GET", "/auth/session",
            headers={"Host": "api.localhost.direct", "Cookie": "cma_session=" + cookie.value})
        ended = _stamp()
        after = _binding(topology(), am)
        fields = ("am", "origin", "pod_uid", "instance_id", "container_id")
        exact = all(serving[k] == after[k] for k in fields)
        try:
            body = json.loads(raw)
        except (ValueError, TypeError):
            body = {}
        request_id = headers.get("x-request-id")
        identified = status == 200 and body.get("authenticated") is True and body.get("subject") == subject
        proved = exact and identified and isinstance(request_id, str) and bool(request_id.strip())
        retain({"kind": "browser_cookie_session", "actor": actor, "status": status,
            "subject_matched": identified, "request_id": request_id,
            "serving": serving, "serving_after": after, "started_at": started, "ended_at": ended,
            "attribution": {"verdict": "PASS" if exact and request_id else "UNPROVED",
                            "basis": "pod_forward_before_after"},
            "verdict": "PASS" if proved else "UNPROVED", "credentials_retained": False})
        if not proved:
            raise ValueError("session_cookie_identity_or_dispatch_unproved")
    same_site = cookie["samesite"].capitalize() or "Lax"
    if same_site not in ("Strict", "Lax", "None"):
        raise ValueError("session_cookie_samesite_invalid")
    return {"subject": subject, "sessionOrigin": "https://api.localhost.direct",
        "storageState": {"cookies": [{"name": "cma_session", "value": cookie.value,
            "domain": "api.localhost.direct", "path": "/", "expires": expires,
            "httpOnly": True, "secure": bool(cookie["secure"]), "sameSite": same_site}],
            "origins": []}}

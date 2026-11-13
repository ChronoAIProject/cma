"""Track browser callback uncertainty before sign-in can create authority."""

import http.cookiejar
import time

from safety import Refusal, write_private
import session_journal


def begin(p, actor):
    journal = session_journal.load(p)
    old = journal.setdefault("oauth", {}).get(actor, {})
    if old.get("status") not in (None, "cleaned"):
        raise Refusal("prior_cma_consent_attempt_requires_cleanup")
    journal["oauth"][actor] = {"status": "started"}
    write_private(session_journal.NAME, journal)


def before_callback(p, actor):
    journal = session_journal.load(p)
    journal["oauth"][actor] = {"status": "callback_submitted", "submitted_at": int(time.time())}
    write_private(session_journal.NAME, journal)


class DurableCookies(http.cookiejar.CookieJar):
    def __init__(self, p, actor):
        super().__init__()
        self.p = p
        self.actor = actor

    def set_cookie(self, cookie, *args, **kwargs):
        if cookie.name == "cma_session" and cookie.domain == "cma.nyxid.ai":
            journal = session_journal.load(self.p)
            entry = journal["oauth"][self.actor]
            entry.update(status="cookie_known", cookie=cookie.name + "=" + cookie.value)
            # Persist the response cookie before the HTTP helper returns.
            write_private(session_journal.NAME, journal)
        return super().set_cookie(cookie, *args, **kwargs)


def known_cookie(p, actor):
    entry = session_journal.load(p).get("oauth", {}).get(actor, {})
    if entry.get("status") == "cookie_known":
        return entry["cookie"]
    return p.accounts[actor].get("cma_cookie")


def cleaned(p, actor):
    journal = session_journal.load(p)
    entry = journal.get("oauth", {}).get(actor, {})
    if entry.get("status") == "callback_submitted":
        # A wholly lost callback reply has no supported cookie lookup endpoint.
        # Revoke consent, keep the obligation, and never claim unseen replay proof.
        raise Refusal("unknown_callback_cookie_requires_supported_reconciliation; do_not_sign_in_again")
    journal.setdefault("oauth", {})[actor] = {"status": "cleaned"}
    write_private(session_journal.NAME, journal)

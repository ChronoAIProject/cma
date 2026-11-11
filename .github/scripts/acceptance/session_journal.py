"""Exact self-session receipts; no account-wide session inventory or deletion."""

from identity import fingerprint, ordinary_profile
from safety import CMA_ORIGIN, NYX_ORIGIN, Refusal, decode, read_private, write_private

NAME = "session-journal.json"


def load(p):
    journal = read_private(NAME)
    expected = {a: p.approval["actors"][a]["subject_sha256"] for a in ("a", "b")}
    if (not journal or journal.get("version") != 1 or journal.get("subjects") != expected
            or journal.get("complete_history") is not True):
        raise Refusal("session_history_missing_or_uncertain")
    for key, entry in journal["tokens"].items():
        if (fingerprint(entry["token"]) != key or entry["actor"] not in expected
                or entry["subject_sha256"] != expected[entry["actor"]]):
            raise Refusal("session_journal_binding_mismatch")
    return journal


def register_current(p):
    if not p.identities_bound:
        raise Refusal("verified_identity_required_for_session_journal")
    journal = load(p)
    for actor in ("a", "b"):
        token = p.accounts[actor]["access_token"]; key = fingerprint(token)
        if key in journal["tokens"]:
            if journal["tokens"][key]["status"] == "revoked":
                raise Refusal("previously_revoked_bearer_became_live")
            continue
        pending = [e for e in journal["tokens"].values() if e["actor"] == actor and e["status"] != "revoked"]
        if len(pending) >= 5:
            raise Refusal("tracked_session_cleanup_required")
        journal["tokens"][key] = {"actor": actor, "token": token,
                                  "subject_sha256": p.approval["actors"][actor]["subject_sha256"],
                                  "status": "active"}
    write_private(NAME, journal)


def revoke_one(p, key):
    journal = load(p); entry = journal["tokens"][key]
    if entry["status"] == "revoked":
        return
    actor = entry["actor"]; token = entry["token"]
    response = p.http.request(NYX_ORIGIN, "/api/v1/users/me", token=token)
    if response[0] == 200:
        user = decode(response[2])
        if not ordinary_profile(user, entry["subject_sha256"]):
            raise Refusal("cleanup_session_identity_mismatch")
        response = p.http.request(CMA_ORIGIN, "/auth/session", token=token)
        if response[0] != 200:
            raise Refusal("cleanup_session_authority_unavailable")
        current = decode(response[2])
        if (current.get("authenticated") is not True
                or fingerprint(current.get("subject", "")) != entry["subject_sha256"]
                or set(current.get("capabilities", [])) != set(p.approval["capabilities"])):
            raise Refusal("cleanup_session_privilege_mismatch")
        entry["status"] = "submitted"
        write_private(NAME, journal)
        # Submitted is uncertain. On resume, a live token reaches this same
        # self-only endpoint again; a 401 confirms no remaining authority.
        response = p.http.request(NYX_ORIGIN, "/api/v1/auth/logout", "POST", token=token)
        if response[0] not in (200, 401):
            raise Refusal("self_session_logout_pending")
        response = p.http.request(NYX_ORIGIN, "/api/v1/users/me", token=token)
    if response[0] != 401:
        raise Refusal("self_session_revocation_unconfirmed")
    entry["status"] = "revoked"
    write_private(NAME, journal)
    p.record("final_stale_nyxid_session_denied_" + actor, "PASS", [1549], response)


def revoke_cma(p, actor):
    account = p.accounts[actor]
    client = account.get("cma_client_id", "")
    if fingerprint(client) != p.approval["cma_client_sha256"]:
        raise Refusal("cleanup_consent_client_not_approved_cma")
    import oauth_journal
    token = account["access_token"]; cookie = oauth_journal.known_cookie(p, actor)
    if cookie:
        response = p.http.request(CMA_ORIGIN, "/auth/session", headers={"Cookie": cookie})
        if response[0] == 200:
            session = decode(response[2])
            if (fingerprint(session.get("subject", "")) != p.approval["actors"][actor]["subject_sha256"]
                    or set(session.get("capabilities", [])) != set(p.approval["capabilities"])):
                raise Refusal("cleanup_cma_cookie_identity_mismatch")
            response = p.http.request(CMA_ORIGIN, "/auth/logout", headers={"Cookie": cookie})
            if response[0] != 303:
                raise Refusal("cma_cookie_logout_pending")
            response = p.http.request(CMA_ORIGIN, "/auth/session", headers={"Cookie": cookie})
        if response[0] != 401:
            raise Refusal("cma_cookie_revocation_unconfirmed")
        p.record("final_stale_cma_cookie_denied_" + actor, "PASS", [1549], response)
    response = p.http.request(NYX_ORIGIN, "/api/v1/users/me/consents", token=token)
    if response[0] != 200:
        raise Refusal("consent_cleanup_requires_live_approved_control_session")
    if any(c.get("client_id") == client for c in decode(response[2]).get("consents", [])):
        response = p.http.request(NYX_ORIGIN, "/api/v1/users/me/consents/" + client, "DELETE", token=token)
        if response[0] not in (200, 404):
            raise Refusal("cma_consent_revoke_pending")
        response = p.http.request(NYX_ORIGIN, "/api/v1/users/me/consents", token=token)
    if response[0] != 200 or any(c.get("client_id") == client for c in decode(response[2]).get("consents", [])):
        raise Refusal("cma_consent_revocation_unconfirmed")
    p.record("final_cma_consent_absent_" + actor, "PASS", [1549], response)
    oauth_journal.cleaned(p, actor)
    p.state.setdefault("consent_cleanup", {})[actor] = "confirmed_absent"
    p.save()

"""Leave both dedicated identities signed out; replay their stale credentials."""

from safety import CMA_ORIGIN, NYX_ORIGIN, Refusal, decode, write_private


def run(p):
    for tenant in ("a", "b"):
        account = p.accounts[tenant]
        if p.state.get("signed_out", {}).get(tenant):
            p.record("final_session_state_" + tenant, "PASS", [1549],
                     reason="retained_verified_signed_out_state; no_new_login")
            continue
        token = account["access_token"]
        cookie = account.get("cma_cookie")
        if not cookie:
            raise Refusal("known_cma_cookie_required_for_cleanup")
        progress = p.state.setdefault("finish_progress", {}).setdefault(tenant, {})
        if not progress:
            response = p.http.request(CMA_ORIGIN, "/auth/session", headers={"Cookie": cookie})
            p.assert_case("final_cookie_owner_control_" + tenant,
                          response[0] == 200 and decode(response[2]).get("subject") == account["subject"],
                          [1549], response, True)
            progress["started"] = True
            p.save()
        # Revoke just the dedicated account's CMA OAuth authority, then its
        # browser cookie and all its own NyxID sessions. No admin credential.
        if not progress.get("consent_removed"):
            response = p.http.request(NYX_ORIGIN, "/api/v1/users/me/consents/" + account["cma_client_id"],
                                      "DELETE", token=token)
            p.assert_case("final_cma_consent_revoke_" + tenant, response[0] in (200, 404), [1549], response)
            response = p.http.request(NYX_ORIGIN, "/api/v1/users/me/consents", token=token)
            consents = decode(response[2]).get("consents", [])
            p.assert_case("final_cma_consent_absent_" + tenant, response[0] == 200
                          and all(c.get("client_id") != account["cma_client_id"] for c in consents),
                          [1549], response, True)
            progress["consent_removed"] = True
            p.save()
        if not progress.get("cookie_removed"):
            response = p.http.request(CMA_ORIGIN, "/auth/logout", headers={"Cookie": cookie})
            p.assert_case("final_cma_cookie_logout_" + tenant, response[0] == 303, [1549], response)
            progress["cookie_removed"] = True
            p.save()
        response = p.http.request(CMA_ORIGIN, "/auth/session", headers={"Cookie": cookie})
        p.assert_case("final_stale_cma_cookie_denied_" + tenant, response[0] == 401,
                      [1549], response, True)
        if not progress.get("current_revoke_submitted"):
            response = p.http.request(NYX_ORIGIN, "/api/v1/sessions", token=token)
            if response[0] != 200:
                raise Refusal("own_session_inventory_unavailable")
            sessions = decode(response[2])
            if len(sessions) > 30 or sum(s.get("is_current") is True for s in sessions) != 1:
                raise Refusal("unexpected_own_session_inventory")
            for session in sessions:
                if session.get("is_current"):
                    continue
                response = p.http.request(NYX_ORIGIN, "/api/v1/sessions/" + session["id"], "DELETE", token=token)
                if response[0] != 204:
                    raise Refusal("old_test_session_revoke_failed")
            response = p.http.request(NYX_ORIGIN, "/api/v1/sessions", token=token)
            remaining = decode(response[2])
            p.assert_case("final_only_current_session_" + tenant,
                          response[0] == 200 and len(remaining) == 1 and remaining[0].get("is_current") is True,
                          [1549], response, True)
            progress["current_revoke_submitted"] = True
            p.save()
            response = p.http.request(NYX_ORIGIN, "/api/v1/sessions/" + remaining[0]["id"], "DELETE", token=token)
            p.assert_case("final_current_session_revoke_" + tenant, response[0] == 204, [1549], response)
        response = p.http.request(NYX_ORIGIN, "/api/v1/users/me", token=token)
        p.assert_case("final_stale_nyxid_session_denied_" + tenant, response[0] == 401,
                      [1549], response, True)
        response = p.http.request(CMA_ORIGIN, "/api/v1/documents/" + p.state["fixtures"]["document_" + tenant]["id"] + "/source", token=token)
        p.assert_case("final_stale_cma_bearer_denied_" + tenant, response[0] == 401,
                      [1549], response, True)
        p.state.setdefault("signed_out", {})[tenant] = True
        account["session_state"] = "signed_out_all_nyxid_sessions_revoked_cma_consent_revoked"
        write_private("accounts.json", p.accounts)
        p.save()

"""Revoke only a dedicated account's current NyxID session and replay it."""

from safety import CMA_ORIGIN, NYX_ORIGIN, decode, write_private


def run(p):
    if "document_a" not in p.state["fixtures"]:
        p.record("stale_session", "BLOCKED", [1549], reason="owned_document_fixture_required")
        return
    path = "/api/v1/documents/" + p.state["fixtures"]["document_a"]["id"] + "/source"
    account = p.accounts["a"]
    if not p.state.get("session_revoked"):
        p.login("a")
        response = p.request("a", path)
        p.assert_case("session_owner_control_before_revoke", response[0] == 200, [1549], response)
        account["stale_access_token"] = account["access_token"]
        write_private("accounts.json", p.accounts)
        response = p.http.request(NYX_ORIGIN, "/api/v1/auth/logout", "POST", token=account["stale_access_token"])
        p.assert_case("nyxid_test_session_revoked", response[0] == 200, [1549], response)
        p.state["session_revoked"] = True
        p.save()
    stale = account["stale_access_token"]
    response = p.http.request(NYX_ORIGIN, "/api/v1/users/me", token=stale)
    p.assert_case("nyxid_stale_session_denied", response[0] == 401, [1549], response, True)
    response = p.http.request(CMA_ORIGIN, path, token=stale)
    p.assert_case("cma_stale_session_denied", response[0] == 401
                  and decode(response[2]).get("code") == "not_authenticated", [1549], response, True)
    p.login("a")
    response = p.request("a", path)
    p.assert_case("session_owner_control_after_relogin", response[0] == 200, [1549], response)

"""Resumable cleanup of only approved, journaled test sessions."""

import session_journal
from safety import CMA_ORIGIN, Refusal, write_private


def run(p):
    if not p.identities_bound:
        raise Refusal("both_live_identities_required_before_cleanup")
    with p.http.cleanup_budget():
        for actor in ("a", "b"):
            session_journal.revoke_cma(p, actor)
        for key in list(session_journal.load(p)["tokens"]):
            session_journal.revoke_one(p, key)
        journal = session_journal.load(p)
        if any(e["status"] != "revoked" for e in journal["tokens"].values()):
            raise Refusal("tracked_session_cleanup_pending")
        for actor in ("a", "b"):
            account = p.accounts[actor]
            response = p.http.request(CMA_ORIGIN, "/api/v1/sandboxes?limit=1", token=account["access_token"])
            p.assert_case("final_stale_cma_bearer_denied_" + actor, response[0] == 401,
                          [1549], response, True)
            p.state.setdefault("signed_out", {})[actor] = True
            account["session_state"] = "signed_out_tracked_sessions_and_cma_consent_revoked"
        write_private("accounts.json", p.accounts)
        p.save()

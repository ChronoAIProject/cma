"""Read-only reuse of the reviewed account store and existing launch receipt."""

import re

import identity
from safety import CMA_ORIGIN, Refusal, decode


def bind_browser(p):
    identity.bind_live(p)
    for actor in ("a", "b"):
        cookie = p.accounts[actor].get("cma_cookie", "")
        if not re.fullmatch(r"cma_session=[^;\s]+", cookie):
            raise Refusal("fresh_dedicated_cookie_required")
        response = p.http.request(
            CMA_ORIGIN, "/auth/session", headers={"Cookie": cookie}
        )
        session = decode(response[2]) if response[0] == 200 else {}
        if (
            session.get("authenticated") is not True
            or identity.fingerprint(session.get("subject", ""))
            != p.approval["actors"][actor]["subject_sha256"]
            or set(session.get("capabilities", [])) != set(p.approval["capabilities"])
        ):
            raise Refusal("fresh_dedicated_cookie_required")


def existing_fixture(p):
    # No arbitrary ID override: only the existing approved campaign's owned receipt.
    receipt = p.state.get("fixtures", {}).get("launch_a", {}).get("resolution", {})
    agent = receipt.get("agent", {}).get("id", "")
    sandbox = receipt.get("sandbox", {}).get("id", "")
    if (
        p.state.get("cleaned_sandboxes", {}).get("a")
        or not re.fullmatch(r"[A-Za-z0-9_-]+", agent)
        or not re.fullmatch(r"[A-Za-z0-9_-]+", sandbox)
    ):
        raise Refusal("existing_dedicated_agent_required")
    cookie = {"Cookie": p.accounts["a"]["cma_cookie"]}
    parent = p.http.request(CMA_ORIGIN, "/api/v1/sandboxes/" + sandbox, headers=cookie)
    item = p.http.request(CMA_ORIGIN, "/api/v1/agents/" + agent, headers=cookie)
    if parent[0] != 200 or item[0] != 200:
        raise Refusal("existing_fixture_not_readable")
    owner = decode(parent[2])
    detail = decode(item[2])
    if (
        identity.fingerprint(owner.get("owner", ""))
        != p.approval["actors"]["a"]["subject_sha256"]
        or owner.get("id") != sandbox
        or detail.get("id") != agent
        or detail.get("sandbox", {}).get("id") != sandbox
        or detail.get("status") != "ready"
    ):
        raise Refusal("existing_settled_owned_fixture_required")
    return agent

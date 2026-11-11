"""Owner-approved ordinary CMA consent, bound to the returned signed request."""

import hashlib
import http.cookiejar
import urllib.parse
import urllib.request

from safety import CMA_ORIGIN, NoRedirect, Refusal, decode, write_private

OAUTH_ORIGIN = "https://api.nyxid.ai"
SCOPES = "openid profile email roles proxy offline_access"


def fields(response):
    return {k.lower(): v for k, v in response[1].items()}


def run(p):
    for tenant in ("a", "b"):
        p.login(tenant)
        jar = http.cookiejar.CookieJar()
        p.http.opener = urllib.request.build_opener(NoRedirect(), urllib.request.HTTPCookieProcessor(jar))
        response = p.http.request(CMA_ORIGIN, "/auth/login")
        if response[0] != 303:
            raise Refusal("cma_login_redirect_required")
        authorization = urllib.parse.urlsplit(fields(response).get("location", ""))
        query = urllib.parse.parse_qs(authorization.query)
        if (authorization.scheme + "://" + authorization.netloc != OAUTH_ORIGIN
                or authorization.path != "/oauth/authorize"
                or query.get("scope") != [SCOPES]
                or query.get("redirect_uri") != [CMA_ORIGIN + "/auth/callback"]
                or query.get("code_challenge_method") != ["S256"]):
            raise Refusal("unexpected_cma_oauth_request")
        account = p.accounts[tenant]
        account["cma_client_id"] = query["client_id"][0]
        write_private("accounts.json", p.accounts)
        response = p.http.request(OAUTH_ORIGIN, authorization.path + "?" + authorization.query,
                                  token=account["access_token"])
        if response[0] == 403 and decode(response[2]).get("error_code") == 3003:
            consent = urllib.parse.urlsplit(decode(response[2])["consent_url"])
            consent_query = urllib.parse.parse_qs(consent.query)
            if (consent.scheme + "://" + consent.netloc != "https://nyxid.ai"
                    or consent.path != "/oauth-consent"
                    or any(consent_query.get(key) != query.get(key)
                           for key in ("client_id", "redirect_uri", "scope", "state", "code_challenge"))):
                raise Refusal("consent_not_bound_to_cma_login")
            response = p.http.request("https://nyxid.ai", consent.path + "?" + consent.query,
                                      token=account["access_token"])
            if response[0] != 200:
                raise Refusal("returned_consent_page_unavailable")
            names = ("response_type", "client_id", "redirect_uri", "scope", "state",
                     "code_challenge", "code_challenge_method", "consent_request")
            form = {key: consent_query[key][0] for key in names}
            # Normal sign-in only. Never expand connected-service or node access.
            form.update(decision="allow", allow_all_services="false")
            response = p.http.request(OAUTH_ORIGIN, "/oauth/authorize/decision", "POST",
                                      token=account["access_token"], form=form)
        if response[0] in (302, 303):
            callback = fields(response).get("location", "")
        elif response[0] == 200:
            callback = decode(response[2]).get("redirect_url", "")
        else:
            p.record("ordinary_cma_consent_" + tenant, "BLOCKED", [1536], response,
                     "cma_consent_not_accepted")
            continue
        target = urllib.parse.urlsplit(callback)
        if (target.scheme + "://" + target.netloc != CMA_ORIGIN or target.path != "/auth/callback"
                or urllib.parse.parse_qs(target.query).get("state") != query.get("state")):
            raise Refusal("callback_not_bound_to_cma_login")
        response = p.http.request(CMA_ORIGIN, target.path + "?" + target.query)
        if response[0] != 303:
            p.record("ordinary_cma_consent_" + tenant, "BLOCKED", [1536], response,
                     "cma_callback_not_complete")
            continue
        account["cma_cookie"] = "; ".join(c.name + "=" + c.value for c in jar
                                          if c.domain == "cma.nyxid.ai" and c.name == "cma_session")
        account["cma_consent_scopes"] = SCOPES.split()
        write_private("accounts.json", p.accounts)
        response = p.http.request(CMA_ORIGIN, "/auth/session")
        session = decode(response[2])
        p.assert_case("ordinary_cma_consent_" + tenant,
                      response[0] == 200 and session.get("subject") == account["subject"]
                      and set(session.get("capabilities", [])) == {
                          "create_sandbox", "use_own_sandboxes", "delete_own_sandbox", "launch_triggers"},
                      [1536, 1549], response, True)
        p.state.setdefault("consent_evidence", {})[tenant] = {
            "account_sha256": hashlib.sha256(account["subject"].encode()).hexdigest(),
            "client": account["cma_client_id"], "scopes": SCOPES.split(),
        }
        p.save()
    p.http.opener = urllib.request.build_opener(NoRedirect())

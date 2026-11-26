"""Exact canonical and retained origins; redirects never carry credentials."""

from safety import CMA_ORIGIN, decode

# The former web origin permanently redirects to the same path on the
# canonical origin. The harness never follows a redirect, so no bearer leaves.
REDIRECTING_ORIGIN = "https://bot.chrono-ai.fun"


def check(p, case, good, response):
    p.record(case, "PASS" if good else "FAIL", [1291, 1597], response)
    if not good:
        print("BUG: " + case + "; domain compatibility contract failed", flush=True)


def run(p):
    # bot-api.chrono-ai.fun is retired (#1343); only the canonical origin serves the API.
    for number, origin in enumerate((CMA_ORIGIN,)):
        response = p.http.request(origin, "/openapi.json", limit=1024 * 1024)
        fields = {k.lower(): v for k, v in response[1].items()}
        valid = response[0] == 200 and "json" in fields.get("content-type", "").lower()
        if valid:
            valid = isinstance(decode(response[2]).get("paths"), dict)
        check(p, "domain_discovery_" + str(number), valid, response)
        for index, browser_origin in enumerate((CMA_ORIGIN, "https://bot.chrono-ai.fun", "https://acceptance.invalid", "null")):
            response = p.http.request(origin, "/api/v1/sandboxes", "OPTIONS", headers={
                "Origin": browser_origin, "Access-Control-Request-Method": "GET",
                "Access-Control-Request-Headers": "authorization",
            })
            headers = {k.lower(): v for k, v in response[1].items()}
            if index < 2:
                good = (response[0] == 200 and headers.get("access-control-allow-origin") == browser_origin
                        and headers.get("access-control-allow-credentials") == "true"
                        and "GET" in headers.get("access-control-allow-methods", "")
                        and "authorization" in headers.get("access-control-allow-headers", "").lower()
                        and {"origin", "access-control-request-method", "access-control-request-headers"}
                        <= {v.strip().lower() for v in headers.get("vary", "").split(",")})
            else:
                good = "access-control-allow-origin" not in headers and "access-control-allow-credentials" not in headers
            check(p, "cors_" + str(number) + "_" + str(index), good, response)
        response = p.http.request(origin, "/api/v1/sandboxes?limit=1",
                                  token=p.accounts["a"]["access_token"])
        check(p, "domain_owned_rest_" + str(number), response[0] == 200, response)
    for number, origin in enumerate((CMA_ORIGIN,)):
        for service, route in (("agent", "/agent-manager/openapi.json"),
                               ("image", "/image-manager/openapi.json"),
                               ("gateway", "/event-gateway/openapi.json")):
            response = p.http.request(origin, route)
            headers = {k.lower(): v for k, v in response[1].items()}
            valid = (response[0] == 200 and "json" in headers.get("content-type", "").lower())
            if valid:
                valid = isinstance(decode(response[2]).get("paths"), dict)
            check(p, "domain_service_" + str(number) + "_" + service, valid, response)
    for name, path in (("discovery", "/openapi.json"), ("agent", "/agent-manager/openapi.json"),
                       ("gateway", "/event-gateway/openapi.json"),
                       ("rest", "/api/v1/sandboxes?limit=1")):
        response = p.http.request(REDIRECTING_ORIGIN, path)
        headers = {k.lower(): v for k, v in response[1].items()}
        check(p, "domain_redirect_" + name,
              response[0] == 301 and headers.get("location") == CMA_ORIGIN + path, response)
    response = p.http.request("https://bot-gateway.chrono-ai.fun", "/openapi.json")
    valid = response[0] == 200 and isinstance(decode(response[2]).get("paths"), dict)
    check(p, "retained_gateway_discovery", valid, response)
    p.record("desktop_origin_upgrade", "BLOCKED", [1291, 1597],
             reason="no_test_owned_ready_desktop_fixture; CORS_does_not_prove_WebSocket_origin")

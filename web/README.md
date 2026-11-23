# CMA Web

The web component serves the frontend and proxies public service routes on the
same origin. Its nginx readiness fallback is covered by a disposable local
upstream and the same nginx runtime image used by the component Dockerfile.

## Anonymous readiness

Monitors read Agent Manager readiness with a direct, anonymous
`GET https://cma.nyxid.ai/ready`. No Cookie, Authorization header, login or
NyxID proxy connection is required. The web server forwards the exact `/ready`
path to Agent Manager and preserves its JSON body and HTTP status.

A successful observation requires **all** of: HTTP 200, an `application/json`
Content-Type (an optional charset is allowed), a JSON object with
`status: "ready"`, an explicit `reason: null`, and an explicit
`dependency.reason: null`. HTTP 200 containing the frontend HTML is a failed
observation. Redirects, invalid JSON, missing fields and other status codes also
fail this assertion. Checking this response contract does not authenticate the
caller or grant business permissions.

HTTP 503 with `status: "not-ready"` means Agent Manager is not ready. Its top-level
`reason` is `store` or `workers`; `dependency.reason` describes the cached Mongo
check (`pending`, `unavailable`, `deadline`, `stale`, or null). Worker unavailability
can return 503 while `dependency.reason` is null. Readiness retains the existing
five-second Mongo freshness limit and required local worker checks. `/livez`
serves process liveness separately.

Public edge routing is maintained outside this repository. A web artifact and
local tests alone do not prove that the public URL has adopted the change.
After deploying the route or web artifact, verify the public URL with an anonymous
read, including Content-Type and JSON assertions. Use local fixtures to test 503
and HTML rejection; do not inject a production dependency failure. Probe scheduling,
proxy access and monitoring data acceptance remain separate integration work.

## Readiness regression checks

With Docker running, execute from the repository root:

```sh
python3 web/scripts/readiness-proxy-tests.py
```

The suite verifies anonymous 200 JSON, preserved store/worker 503 responses,
exact-path boundaries, and rejection of SPA HTML 200 and incomplete JSON.

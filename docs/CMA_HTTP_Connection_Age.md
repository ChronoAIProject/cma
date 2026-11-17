# AM persistent connection retirement

Issue #2261 addresses a connection-level load-spread risk. A ClusterIP chooses a
backend for a TCP connection. EG's continuously used HTTP/2 pools can retain that
choice indefinitely; `sessionAffinity: None` and a 90-second **idle** expiry do
not redistribute requests on an existing busy connection. Browser HTTP/2 to a
managed Gateway does not by itself prove affinity between that Gateway and AM.
Observe the public and internal request counters separately before attribution.

`CHRONO_HTTP_CONNECTION_MAX_AGE_SECONDS` opts AM into connection retirement.
Unset or `0` uses the existing `axum::serve` path. Enabled values are 30–3600
seconds; the initial proposed value is 60. The checked-in ConfigMap stays at 0.
Enabling the setting requires the compatible AM image and lead approval.

HTTP/2 starts graceful GOAWAY at the age limit. Accepted streams continue; new
requests can establish a new connection and obtain a new Service backend.
HTTP/1 sends `Connection: close` with the first ordinary response whose headers
are produced after the limit. It finishes that response, then closes. An idle
HTTP/1 socket is deliberately not killed by a timer: that races a non-replayable
request and can reset it. Consequently an idle connection can remain open past
the age limit, and its next response is the final one. HTTP/1 WebSocket upgrade
responses retain their `Connection: upgrade` header; upgraded sockets are not
subject to this admission-age policy. Active SSE bodies are never cancelled by
age. The existing process termination signal, owner drain, and 660-second HTTP
shutdown deadline remain authoritative. Retirement does not replay requests.

The implementation preserves Axum's TCP `ConnectInfo` and HTTP/2 extended
CONNECT support. It uses a single outer middleware layer per connection rather
than rebuilding the route table. Reverting the setting to 0 on a separately
approved rollout selects the original serving implementation; no ownership,
lease, retained-data or provider-authority change is required.

Tests in `api/crates/cma/src/http_server/tests.rs` use real loopback sockets for
both protocols: reused connections, continuous traffic, connection retirement,
active streaming, WebSocket upgrades before and after age, graceful process
shutdown, and writes across retirements with transport retries disabled. A
TCP proxy selects between two actual test servers on each connection; it proves
renewed backend selection, not Kubernetes/GFE or CMA feature acceptance.

MR5 acceptance is a complete 30-minute same-process production window in which
neither of the two Ready AMs serves more than 70% of the counted requests. Retain
raw counter samples, Pod UID/container ID/image/revision, readiness and endpoint
membership. Reject counter resets, identity changes and observation gaps. Report
all requests and public/non-probe traffic separately; directed owner forwarding
is not an ingress balancing failure. Record concurrent verification traffic.
Connection retirement creates another selection opportunity, not a guarantee of
50/50 routing, so passing socket tests cannot substitute for this observation.

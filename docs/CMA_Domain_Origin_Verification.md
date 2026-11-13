# CMA production and retained origin verification

The routing and desktop-origin implementation proposed in PR #1291 is already
in develop through PR #1292 (`e08bcc540`). Later changes retain desktop connection
cleanup and the Event Gateway console route. Refreshing the evidence must use
current develop; merging the stale branch must not remove those changes.

This document records the source contract and the remaining production checks
for #1597. It supplements `CMA_Domain_Migration.md`; its historical observations
do not establish the state of the current deployed candidate.

## Source contract

| Host or origin | Required behavior |
| --- | --- |
| `https://cma.nyxid.ai` | Current CMA public origin; exact desktop browser Origin admitted after authentication and ownership checks |
| `https://bot.chrono-ai.fun` | Retained app/API origin; exact desktop browser Origin remains admitted |
| `https://bot-api.chrono-ai.fun` | Retained direct API entry point, routed to Agent Manager |
| `https://bot-gateway.chrono-ai.fun` | Retained Event Gateway entry point; preserve its existing protocol and webhook routes |
| Lookalike hosts, HTTP browser origin, unexpected port, origin with a path | Desktop cookie WebSocket denied; no binary stream starts |
| Missing or duplicate Origin with a cookie | Desktop WebSocket denied |
| Permitted Origin with missing authentication or a foreign owner | Denied before opening the desktop |

The app and API path ownership is declared in `infra/api/httproute.yaml` and
`infra/cmaeg/httproute.yaml`. The source checks compare complete route rules,
listeners and backends, including retained routes and the new origin. Production
desktop origins derive from the configured public URL and exact CORS allowlist,
not from request Host or forwarded headers. Do not add wildcard origins.
The source contract checks the canonical public URL and both exact configured
browser origins; mutation tests reject dropped, duplicate or expanded origins.

## Local regression evidence

The public contract corpus `desktop-production-origins.json` covers both real
browser-origin strings with loopback HTTP WebSocket handshakes and binary relay.
It includes owner success, anonymous and foreign-owner denial, and five invalid
origin forms. These tests do not connect to the production hosts.

Run the existing desktop corpus as well, preserving missing/duplicate-Origin,
cookie, direct-client, capability, paused-runtime and connection-busy coverage:

```sh
CMA_PUBLIC_CONTRACT_FILE=desktop-production-origins.json cargo test --locked -p layer1-public shared_public_contract_matches_real_backend
CMA_PUBLIC_CONTRACT_FILE=desktop.json cargo test --locked -p layer1-public shared_public_contract_matches_real_backend
cargo test --locked -p layer1-public desktop
ruby .github/scripts/check-http-route-backends.rb
ruby .github/scripts/ci-contract-tests.rb release
```

Cargo commands run from `api/`; Ruby commands run from the repository root.
Use the sprint's shared build lock and at most two Cargo jobs. The PR records
the exact source SHA and results. Source checks do not attest DNS, certificate
state, live Gateway attachment or deployed application configuration.

## Production acceptance through the release owner

The release owner runs these checks and the lead relays aggregate evidence.
Application contributors do not operate production from this workstream.

1. Record the deployed source SHA, API/web image digests, configuration revision,
   environment, time window, sample count and raw evidence location.
2. Verify DNS/TLS and the canonical backend health for all four retained/current
   hosts. Confirm actual Gateway acceptance and resolved backend references;
   an alias health result alone is insufficient.
3. In a real authenticated browser, open the same permitted desktop from each
   app origin, confirm a successful binary connection, then disconnect and
   reconnect. Check denied origin and foreign-owner cases using approved fixtures.
4. Exercise authenticated REST and repository SDK reads through current and old
   API hosts. Preserve content types and typed errors; HTML proxy failures must
   not be treated as successful JSON. Verify existing callback and protocol paths
   remain correctly routed without redirecting retained API/webhook traffic.
5. Attach the results to #1597 and the PR #1291 handoff. An absent browser, SDK,
   legacy-host or deployment-identity result remains pending evidence.

Any live DNS, certificate, Cloudflare, Gateway or configuration change belongs
to the release/infrastructure owner. No such change is authorized by passing
the local regression corpus alone.

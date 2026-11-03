# CMA migration to cma.nyxid.ai

This is a staged migration of the public CMA origin. The product remains CMA.
Infrastructure owns the Cloudflare DNS record; CMA owns the HTTPRoutes,
application configuration, client URLs and rollout verification. Routing can
be prepared before DNS. A working page on the new host does not establish that
OAuth, browser mutations, proxy credentials or channel setup work there.

The owner explicitly requires retaining `bot.chrono-ai.fun` for existing
clients. Keep its DNS, TLS and direct service routes throughout migration.
Browser redirection is a later navigation change, not permission to redirect
all traffic or retire the old origin.

## Observed baseline — 2026-11-03

| Item | Observed value |
|---|---|
| Kubernetes context | `gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke` |
| Application namespace | `chrono-bot` |
| Shared Gateway | `gateway-infra/chronoai-gateway` |
| Gateway address | `136.69.78.214`; re-read before configuring DNS |
| New-domain listener | `https-nyxid`, HTTPS 443, hostname `*.nyxid.ai` |
| TLS certificate | `gateway-infra/internal-nyxid-ai-cert`; Let's Encrypt YR1, SAN `*.nyxid.ai`, expires 2027-01-26 |
| Existing listeners | `https` for retained CMA hosts; `http` for HTTPS redirects |
| Current CMA origin | `https://bot.chrono-ai.fun` |
| Current canonical Gateway base | `https://bot.chrono-ai.fun/event-gateway` |
| Retained Gateway origin | `https://bot-gateway.chrono-ai.fun` |
| DNS | No address returned for `cma.nyxid.ai` at inspection; infra team owns the record |
| NyxID discovery | `https://api.nyxid.ai/.well-known/openid-configuration` advertises new-domain endpoints but issuer `https://nyx-api.chrono-ai.fun` |

There is no CMA `Ingress` resource. The ingress layer is Gateway API; do not
introduce a second Ingress controller or change the shared Gateway for this task.
Both existing public route rule sets match the checked-in routing rules.
Production image digests differ from this checkout, so applying all of `infra/`
would risk rolling back unrelated application changes. Apply only the three
new route objects in phase 1.

Legacy `cma-api`, `cmaeg` and `cmaeg-http-redirect` HTTPRoutes also exist live.
They overlap retained hosts and are outside this additive change. Inventory
their ownership before any later cleanup; do not delete them during cutover.

## Phase 1 — additive routing, before DNS or application cutover

The existing manifest files now also declare these independent objects:

| New HTTPRoute | Listener | Backend behavior |
|---|---|---|
| `cma-web-nyxid` | `https-nyxid` | Same path ownership as `cma-web`: SPA, Agent Manager, Image Manager and the Agent Manager login bridge |
| `cma-event-gateway-public-nyxid` | `https-nyxid` | Same `/event-gateway/*` protocol rewrites as the existing public Gateway route |
| `cma-nyxid-http-redirect` | `http` | `http://cma.nyxid.ai/*` to the same host/path over HTTPS |

All three bind exactly `cma.nyxid.ai`. The SPA page `/event-gateway` remains
with web; `/api/event-gateway` remains the authenticated Agent Manager bridge.
Direct API/auth routing preserves the load balancer's client chain and the
existing 660-second backend stream timeout. No application Pod restart is
needed for this phase. Existing hostnames, Services, selectors and ConfigMaps
remain effective.

Run from the repository root. The extraction avoids applying old deployments,
ConfigMaps or other routes from this checkout:

```sh
CMA_CONTEXT=gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke
CMA_ROLLOUT_DIR=$(mktemp -d)
ruby -r yaml -r json -e '
  names = %w[cma-web-nyxid cma-nyxid-http-redirect cma-event-gateway-public-nyxid]
  items = ARGV.flat_map { |path| YAML.load_stream(File.read(path)) }
    .select { |doc| names.include?(doc.dig("metadata", "name")) }
  abort "expected exactly three routes" unless items.length == 3
  puts JSON.pretty_generate({apiVersion: "v1", kind: "List", items: items})
' infra/api/httproute.yaml infra/cmaeg/httproute.yaml > "$CMA_ROLLOUT_DIR/routes.json"

ruby .github/scripts/check-http-route-backends.rb
ruby .github/scripts/ci-contract.rb release
ruby .github/scripts/ci-contract-tests.rb release
kubectl --context "$CMA_CONTEXT" -n chrono-bot apply --dry-run=server \
  --field-manager=cma-domain-migration -f "$CMA_ROLLOUT_DIR/routes.json"
kubectl --context "$CMA_CONTEXT" -n chrono-bot apply \
  --field-manager=cma-domain-migration -f "$CMA_ROLLOUT_DIR/routes.json"
kubectl --context "$CMA_CONTEXT" -n chrono-bot get httproute \
  cma-web-nyxid cma-event-gateway-public-nyxid cma-nyxid-http-redirect -o yaml
```

Require `Accepted=True`, `ResolvedRefs=True` and the GKE reconciliation condition
for the current generation and intended listener. Also require the shared
Gateway listener to remain healthy. Verify direct TLS/SNI with the current
Gateway IP, without disabling certificate verification:

```sh
curl --noproxy '*' --resolve cma.nyxid.ai:443:136.69.78.214 \
  --connect-timeout 5 --max-time 20 -I https://cma.nyxid.ai/
curl --noproxy '*' --resolve cma.nyxid.ai:443:136.69.78.214 \
  --connect-timeout 5 --max-time 20 https://cma.nyxid.ai/agent-manager/openapi.json
curl --noproxy '*' --resolve cma.nyxid.ai:443:136.69.78.214 \
  --connect-timeout 5 --max-time 20 https://cma.nyxid.ai/event-gateway/openapi.json
curl --noproxy '*' --resolve cma.nyxid.ai:80:136.69.78.214 \
  --connect-timeout 5 --max-time 20 -I 'http://cma.nyxid.ai/t/check?source=migration'
```

Compare public discovery responses, API unauthenticated refusals and legacy
console redirects with the old host. Check that the redirect preserves path
and query. Existing advertised URLs are expected during this phase. Do not
complete new-host login or consume one-use setup links as a routing probe:
host-only transaction cookies and Origin checks still belong to the old host.

## URL inventory and cutover changes

### CMA application origin — phase 2

| File / setting | Current | Target / action |
|---|---|---|
| `infra/api/configmap.yaml`: `CHRONO_SERVER_PUBLIC_URL` | `https://bot.chrono-ai.fun` | `https://cma.nyxid.ai`; affects OAuth callbacks, generated public links, discovery, runtime tool URLs and signed proxy JWT audience |
| Same: `CHRONO_SERVER_CORS_ORIGIN` | `https://bot.chrono-ai.fun` | Retain old origin and add `https://cma.nyxid.ai`; exact browser Origin checks use both |
| `infra/image-manager/configmap.yaml`: `CMA_IM_WEB_ORIGIN` | `https://bot.chrono-ai.fun` | `https://cma.nyxid.ai` before activating or rolling Image Manager |
| `infra/web/configmap.yaml`: `CHRONO_API_BASE_URL`, `CHRONO_PUBLIC_ORIGIN` | Old API / web hosts | Target `https://cma.nyxid.ai` for both; these are currently unused metadata, not build/runtime inputs in the checked-in frontend |
| NyxID CMA OAuth client | `c0cd33d2-27ba-4b8d-b38e-562364fadb72` | Read current registration; add new `/auth/callback` and `/connection-return` URIs, preserving existing entries |
| NyxID CMA service registration | Existing origin / proxy JWT audience | Inspect and coordinate upstream URL, discovery URL and signed audience with the new CMA origin |
| Image Manager OAuth client, if active | Exact browser callbacks | Inspect its actual client registration; add `https://cma.nyxid.ai/api/image-manager/auth/callback` if that independent browser flow is enabled |
| `sdk/typescript/src/react.ts` and `sdk/typescript/docs/reference.md` | Trigger button default is old origin | Change default and regression expectation in `tests/react.test.ts` when new-host login is ready |
| `README.md` and published Trigger snippets | Old production links | Update maintained entry links; retain old API/badge/open paths for already embedded snippets |
| `docs/API_Agent_Manager.md`, `docs/CMA_Service_Connection_Callback.md`, `docs/CMA_Deployment_Readiness.md` | Old discovery / callback examples | Update current operational examples with phase 2; preserve dated historical evidence |
| `docs/Release.md` | Release checks explicitly name the old hosts | Extend current routing and origin-verification procedures to include the new host and retained-host checks |

`web/nginx.conf` uses `server_name _`, relative browser requests and in-cluster
Service destinations. It needs no hostname replacement for phase 1. Do not
route browser requests through `api.nyxid.ai` or add cross-domain cookies.

Before phase 2, infra confirms Cloudflare proxy mode and A record, HTTPS/TLS
configuration, Host/SNI preservation, and cache bypass for authenticated API,
OAuth, SSE and Gateway traffic. Repeat checks through normal public DNS: direct
origin checks do not prove Cloudflare behavior.

Read current NyxID registrations rather than replacing them from a template.
Keep client/service IDs, secrets, scopes and resource ownership. The new origin
previously changed the `CHRONO_SERVER_PUBLIC_URL`-derived proxy JWT audience.
The migration now provides `CHRONO_NYXID_PROXY_AUDIENCE` and pins it to
`https://bot.chrono-ai.fun`, independently of the browser public URL. Existing
NyxID connections retain their signed audience; issuer, service and signature
checks remain strict. A signed regression rejects the new browser origin as an
audience when the retained service audience is explicitly configured. Changing
NyxID service audiences is a separate coordinated migration.

Publish the application-origin changes in a separate reviewed change against
the current production source and images. Stage NyxID callback registrations
first, deploy required compatibility readers, and only then change public URL
and CORS together. Environment ConfigMap edits require an explicit control-plane
rollout; keep current immutable images unless the compatibility change requires
a new verified image. Keep active Sandbox Pods, homes and conversations running.
Do not apply this checkout's older deployment/image or runtime defaults.

Use a tested browser-navigation redirect after cutover, starting with 302 and
preserving the complete path and query. Do not install it during phase 1:
the new host still uses the old application's login/Origin configuration.

| Request on `bot.chrono-ai.fun` | Compatibility behavior after cutover |
|---|---|
| GET/HEAD for ordinary SPA pages, shared pages and `/t/...` landings | Temporary redirect to the same path/query at `https://cma.nyxid.ai` |
| `/api/*`, including Trigger badge/open links and SDK requests | Direct service, same method/body/authentication; never a blanket cross-host redirect |
| `/event-gateway/v1/*`, `/event-gateway/mcp`, protocol docs/discovery | Direct Gateway service; retain old callbacks and protocol clients |
| `/event-gateway/connect*` and `/event-gateway/legacy-console*` | Preserve existing setup/console behavior and capability cookies |
| `/openapi.json`, `/agent-manager/openapi.json`, `/image-manager/openapi.json`, `/.well-known/agent-card.json` | Direct discovery, retaining backend ownership |
| `/auth/*`, `/connection-return`, `/assets/*` | Preserve direct routes during transition; handle login navigation explicitly and retain assets for old open tabs |
| Any POST/PUT/PATCH/DELETE | No browser-navigation redirect |

Implement redirects with explicit GET/HEAD matches after preserving the more
specific direct-service paths; test that protocol requests cannot fall into the
SPA catch-all. A 307/308 by itself is not a substitute for this split: even a
method-preserving cross-host redirect can lose Authorization or be refused by
a webhook client. Consider 301 only for stable browser pages after the rollback
window; old API routes remain in service independently.

Before changing the canonical origin, let existing OAuth attempts drain or
require a fresh login. Forwarding a callback code does not transfer the
host-only state cookie. Verify old-host sign-in navigation starts on the new
host before setting its transaction cookie. Old open tabs may need a reload
and login at the new origin. Browser local storage, IndexedDB and unsent drafts
do not move with DNS.

Phase-2 acceptance: real owner login/logout, session renewal, service connection
return, an authorized API read/write, streamed conversation and reconnect,
Trigger landing plus explicit launch, and existing Agent continuity. Check
NyxID proxy requests separately from browser login. Image Manager checks apply
only when deployed; its checked-in replica count is zero. On the inspected
deployment, `/image-manager/openapi.json` returns 503 on the old origin; record
that baseline rather than treating it as a newly introduced routing failure.

### Event Gateway canonical URLs — phase 3, requires compatibility work

| File / setting | Target / action |
|---|---|
| `infra/api/configmap.yaml`: `CHRONO_CMAEG_PUBLIC_API_URL` | `https://cma.nyxid.ai/event-gateway` |
| `infra/cmaeg/configmap.yaml`: `CMAEG_PUBLIC_API_URL` | Same target; coordinate with Agent Manager readers |
| `CMAEG_ADVERTISE_PUBLIC_API_URL` | Retain staged advertisement control; changing this flag alone does not add a third accepted URL base |
| `CHRONO_CMAEG_PUBLIC_URL`, `CMAEG_PUBLIC_URL` | Keep `https://bot-gateway.chrono-ai.fun` for retained console, callbacks and sealed identities |
| NyxID `cmaeg` service catalog | New REST base `/event-gateway/v1`, OpenAPI `/event-gateway/openapi.json`, MCP `/event-gateway/mcp` |
| Public guides | Update maintained production links in `CMAEG_Integration.md`, `CMAEG_Architecture.md`, `CMAEG_Protocol.md`, `CMAEG_Operations.md`, `CMA_Operations.md`, `Shared_Login.md`, API guides and Bot Father skill references |
| `api/crates/cmaeg/src/http/docs.rs` | Update canonical link rewriting and download preamble together with the Markdown sources |
| `web/src/services/EventGatewayDocs.tsx` | Recognize both old and new documentation origins; retain legacy link normalization and update its tests |

There will be **three** supported bases after migration: the new canonical base,
the old prefixed `bot.chrono-ai.fun/event-gateway` base, and the original
`bot-gateway.chrono-ai.fun` base. The migration adds bounded exact-base lists
`CHRONO_CMAEG_PUBLIC_API_ALIASES` and `CMAEG_PUBLIC_API_ALIASES`. Keeping the
HTTPRoutes alone is insufficient. The compatibility implementation covers:

- `api/crates/cma/src/gateway/client.rs`: accept the old prefixed callback and
  custom-event pair alongside the two configured bases, including retained
  callback validation. A pair must still share one exact allowed base and channel.
- `api/crates/cmaeg/src/connections/binding.rs` and `webhook.rs`: recognize all
  three exact Telegram webhook addresses for activation and retirement. Otherwise
  an existing integration can be refused or its webhook left installed.
- `api/crates/cmaeg/src/http/connection_browser.rs`: preserve old prefixed setup
  links and tabs, strict Origin checks and host-only capability cookies. Forms
  accept only configured origins for their exact path prefix. Do not redirect
  consumed one-use links or broaden Origin checks to every `*.nyxid.ai` host.
- Audit retained runtime/MCP endpoint validation and signed callback audiences;
  preserve exact old identities. Do not rewrite ChannelPointers or credentials.
- Add focused mixed-generation tests covering old/new advertisements, old live
  setup tabs, retained callbacks, webhook activation/retirement, cross-origin
  rejection and rollback with already-issued new links.

Deploy compatibility readers across Agent Manager and Gateway first, with old
advertisement and `https://cma.nyxid.ai/event-gateway` in both alias lists.
Pin the retained proxy audience before changing CMA's browser origin. Verify
all old and draining Pods have exited before enabling new advertisement. Then
change both canonical bases to `https://cma.nyxid.ai/event-gateway` and both alias
lists to `https://bot.chrono-ai.fun/event-gateway`, using the same verified
compatible images. Validate before updating the NyxID catalog. Existing
webhooks do not need forced re-registration to adopt the new UI domain. Phase 2
can keep the Gateway's old canonical base while this separate work is completed.

### NyxID provider URLs — separate coordinated migration

Do not treat every `chrono-ai.fun` occurrence as a CMA URL. New discovery currently
retains the old issuer; changing a transport base can also change identity checks.

| Setting / source | Intended destination | Constraint |
|---|---|---|
| `CHRONO_NYXID_PUBLIC_URL` | Actual NyxID browser origin, expected `https://nyxid.ai` | Confirm browser signup/connect routes before switching |
| `web/src/profiles/ReferenceLink.tsx` | Same browser origin for `/services/{id}` | Hard-coded old NyxID UI link; update with the browser-origin change |
| `CHRONO_NYXID_BASE_URL` | Eventually `https://api.nyxid.ai` | Also feeds issuer-bound organization/resource contexts; requires provider compatibility |
| `CHRONO_NYXID_PROXY_ISSUER` | Provider-declared issuer | Keep the current declared issuer until NyxID changes it deliberately |
| `CHRONO_NYXID_PROXY_JWKS_URL` | Eventually new discovery's `jwks_uri` | Verify keys and retained/new token validation independently of issuer |
| `CMAEG_NYXID_BASE_URL`, `CMA_IM_NYXID_ISSUER` | Coordinated NyxID endpoints | Shared service auth derives issuer and proxy resource URI from this base and compares introspection `iss` exactly |
| `CMAEG_PROVIDERS` entry `nyxbot` | Eventually `https://api.nyxid.ai` | Verify provider discovery, service authentication and calls at new endpoints |
| `CHRONO_PLATFORM_BASE_URL`, `CHRONO_UPSTREAM_BASE_URL` | New API origin / same proxy path | Prove storage, skills, model calls and scoped AgentKey support; retained runtimes may keep their old endpoint configuration |

Keep internal `cma-api.chrono-bot.svc` authority, Service aliases, service slugs,
container registries and database names. Old URLs in dated acceptance evidence
remain historical facts. Example/test domains do not need production renaming.

## Rollback and completion

Before each phase record the current route specs, affected ConfigMap keys,
NyxID registrations, Pod UIDs and active image digests in protected operational
storage. Never copy secrets into this document or Git.

| Point reached | Rollback |
|---|---|
| Phase 1 only, no new links advertised | Delete only `cma-web-nyxid`, `cma-event-gateway-public-nyxid`, `cma-nyxid-http-redirect`; coordinate DNS visibility with infra. Existing routes continue serving. |
| CMA canonical origin changed | Restore the previous public URL/CORS and coordinated NyxID proxy audience; disable old-page redirects; roll the same compatible control-plane images. Keep both DNS/routes and additive OAuth registrations. Users may need another login. |
| Gateway has issued new links | Keep all three host/base routes and compatibility readers. Advertisement may return to the old base, but do not roll back to a binary that rejects new callbacks or remove new routes. |

Stop promotion on new auth/CSRF failures, unexpected redirects, rejected retained
callbacks, webhook conflicts, stream regressions or Agent continuity failures.
Compare with the measured pre-rollout baseline. A return to old advertisement
does not make previously issued links disappear.

Completion requires public DNS/TLS, the phase-2 owner flows, signed proxy access,
phase-3 retained/new channel checks and stable existing Agents. Keep legacy
endpoints until actual integration inventory and observed traffic support their
retirement; an arbitrary elapsed time is not evidence that old webhooks are gone.

## Execution evidence

Phase 1 source checks pass: exact HTTPRoute backend ownership, release workflow
and source contracts, and 93 unsafe workflow/routing mutations rejected.
Kubernetes server-side dry-run passed before applying only the three new routes.

Applied on 2026-11-03 at approximately 14:40 Asia/Singapore. All three HTTPRoutes
report `Accepted=True`, `ResolvedRefs=True`, `Reconciled=True` for generation 1;
the shared Gateway and `https-nyxid` listener report `Programmed=True`.
Load balancer propagation initially returned 404; successful origin verification
was completed at 14:44 Asia/Singapore with certificate verification enabled.

| Direct-origin probe at `cma.nyxid.ai` | Result |
|---|---|
| `/`, `/event-gateway` | 200; same response body as old host |
| `/agent-manager/openapi.json`, `/event-gateway/openapi.json`, `/event-gateway/docs` | 200; same response body as old host |
| `/auth/session`, `/api/v1/agents`, `/api/agent-manager/v1/agents`, `/event-gateway/v1/channels` without credentials | 401; same refusal as old host |
| `/event-gateway/legacy-console` | 307 to retained `https://bot-gateway.chrono-ai.fun/console` |
| `/image-manager/openapi.json` | 503 on both hosts, matching the pre-existing unavailable service |
| HTTP `/t/check?source=migration` | 301 to HTTPS on `cma.nyxid.ai` with the same path/query; GFE may spell the default port as `:443` |

All nine existing HTTPRoute specs matched the pre-change snapshot. The four
observed application Pod identities and restart counts were unchanged. No
application ConfigMap, deployment image, OAuth registration or DNS was changed.
Public DNS still returned NXDOMAIN during the initial 14:44 verification.
The subsequent public-path verification after infra configured Cloudflare is
recorded below. Authenticated login, writes, streaming and retained channel
behavior on the new origin have not been claimed as verified.

The application-origin, browser redirect, Gateway advertisement and NyxID
provider cutovers remain the gated future phases above. In particular, the
old host is still directly served and no cross-domain redirect is active.

### Cloudflare public-path verification — 2026-11-03 14:54 Asia/Singapore

Infra reported the Cloudflare record configured. Normal public DNS now resolves
`cma.nyxid.ai` to Cloudflare edge IPv4 addresses `104.20.18.161` and
`172.66.161.189`, and also returns Cloudflare edge IPv6 addresses. These are
proxy addresses, not a change to the IPv4 origin recorded above.

Requests used normal DNS without an origin-IP override and with TLS certificate
verification enabled. All 11 paths in the direct-origin table were rechecked
through Cloudflare on both the new and old web hosts; status codes and response
bodies matched. Responses carried `server: cloudflare` and
`cf-cache-status: DYNAMIC`; the tested pages/documents with `no-store` retained
that directive. No `cf-mitigated` challenge response or unexpected cross-host
redirect was observed. HTTP-to-HTTPS redirection preserved path and query.

The retained direct API and Gateway discovery URLs also returned 200:

- `https://bot-api.chrono-ai.fun/agent-manager/openapi.json`
- `https://bot-gateway.chrono-ai.fun/openapi.json`

An unauthenticated GET of `https://cma.nyxid.ai/auth/login`, without following
or completing the authorization flow, returned 303 to
`https://api.nyxid.ai/oauth/authorize` with
`redirect_uri=https://bot.chrono-ai.fun/auth/callback`. No state, cookie value or
authorization credential is recorded here. This confirms the remaining
application cutover: new-host browser transaction cookies do not transfer to
the old callback host. Do not enable old-page redirects before phase-2 login
acceptance succeeds.

The live Agent Manager ConfigMap still has the old `CHRONO_SERVER_PUBLIC_URL`,
`CHRONO_SERVER_CORS_ORIGIN` and `CHRONO_CMAEG_PUBLIC_API_URL`. Gateway discovery
still advertises the old prefixed API base, as intended before phase 3.
NyxID discovery still declares the old issuer. The three new HTTPRoutes remain
accepted, resolved and reconciled at generation 1.

DNS and unauthenticated public routing acceptance are complete. Cloudflare's
configured SSL mode and full ruleset were not inspected through its management
API; successful probes do not attest every WAF/cache rule, authenticated traffic
or stream duration. No production settings were changed during this verification.


### Desktop incident, 2026-11-03 15:03 SGT

Authenticated probes reproduced a new-domain desktop refusal: the same upgrade
request with `Origin: https://cma.nyxid.ai` returned 403, while the retained
`https://bot.chrono-ai.fun` origin reached the Agent lookup (404 for the deliberate
nonexistent probe ID). The API still uses the old public origin for its desktop
WebSocket check. This is independent of Sandbox/runtime availability.

A temporary exact-Origin WebSocket route bridge was attempted and fully rolled
back before activation: GKE rejects method matching, and Google URL maps also
reject modifying a header used in a header match. API-server dry-run alone does
not prove either controller feature. The previous serving route remains intact;
no Pod, Deployment, Sandbox, PVC or runtime image was changed by this attempt.
The durable fix is an API image with explicit trusted browser-origin aliases and
`CHRONO_SERVER_CORS_ORIGIN` including both exact CMA origins. Validate successful
binary WebSocket relay from both origins and rejection of foreign, null and
duplicate origins before declaring recovery.

Public probes also found Cloudflare returning 301 from old-domain **API** and
`/auth/login` requests to the new hostname at 15:04 SGT. The CMA HTTPRoutes do not
contain that HTTPS redirect. The owner subsequently prioritized full new-domain
functionality; restoring old-client behavior is not a prerequisite for that
work. Retained Kubernetes routes continue to serve directly at the origin, but
public old-domain client compatibility cannot be claimed while Cloudflare sends
protocol traffic through a cross-host 301. The Cloudflare owner should preserve
direct API, callback, webhook and setup traffic; browser page redirects need a
separate reviewed allowlist after canonical login works.

### Compatibility delivery scope

Only API, Gateway and web control-plane images and exact domain configuration
keys change. Preserve every runtime image pin, feature flag, native Sandbox and
Claim, runtime Pod UID, home PVC and active Agent. Capture a fresh inventory
before each rollout and compare after readiness and drain. Never apply whole
checked-in ConfigMaps or Deployments over live production configuration.

Canonical login entry redirects before issuing the host-only transaction cookie;
callbacks themselves remain direct. An OAuth attempt begun immediately before
the canonical cutover can require a fresh login because its original callback
URI belongs to the previous configuration. Persisted Agent conversations and
Sandbox resources are unaffected by this browser transaction retry.


## Retained Agent authorization migration

The CMA public origin also namespaces persisted Agent access-tool capabilities.
Changing only `CHRONO_SERVER_PUBLIC_URL` invalidates those capabilities and blocks
conversation reconnection, even while every runtime Pod and PVC remains healthy.
Runtime identity checks, desktop WebSocket checks and browser login alone do not
prove message execution. Never rewrite capability hashes, encryption keys, Agent
IDs, creation provenance or native conversation IDs to change the domain.

Deploy the compatibility reader first with the original public origin and
`CHRONO_AGENT_ACCESS_RETAINED_ORIGINS=https://cma.nyxid.ai`. After every older API
worker has drained, switch the public origin to `https://cma.nyxid.ai` and retain
`https://bot.chrono-ai.fun`. Both phases verify both explicitly configured
authorities. Existing records preserve their exact capability and installed
tool endpoint; new runtime incarnations receive canonical authorization. The
reader derives the authority from the stored credential hash and rejects ambiguous
records. Owner, runtime, creation, conversation and writer-fence checks still apply.

A running Agent has no verified live tool-endpoint rewrite operation. Keep
`https://bot.chrono-ai.fun/api/v1/agent-tools` directly routed to CMA while retained
Agents use it. A Cloudflare cross-host 301 is insufficient for authenticated MCP
POSTs. This compatibility route does not require users to keep visiting the old
website. Route API and WebSocket traffic directly; redirect browser pages only.

Before announcing completion, prove old credentials authenticate through the new
service, an existing Agent completes a newly admitted message in its original
conversation, and runtime Pod UIDs, container IDs, images, restarts and PVCs remain
unchanged. Pin the event cursor before the test so a historical reply cannot pass
acceptance. Confirm no command is replayed after a possibly successful execution.
A command that definitively expired before dispatch is a failed request, not proof
of agent recovery. Rollback after canonical enrollment must keep both readers.


## Explicit migration of retained Agent tool endpoints

Compatibility readers preserve installed URLs until a durable idle maintenance
operation changes them. `CHRONO_AGENT_ACCESS_TRANSPORT_MIGRATION` enables that
operation for an explicit batch. The application default is disabled; the
production manifest selects the reviewed cohort below. It requires managed
document context. The JSON contains `revision`, `origin` and `agents`; the origin
must equal `CHRONO_SERVER_PUBLIC_URL`, revision must be at least 2 and greater
than the context revision already applied, and agents contains exact Agent IDs
or the sole entry `"*"`. A future domain change needs a larger revision.

Deploy this reader version on every API worker with migration admission disabled
before enabling the batch. It adds optional persisted transport and maintenance
intent fields; an older API binary cannot read records once they are populated.
Wait for all older workers to drain naturally. Rollback after migration begins
uses this compatible binary with new admission disabled, and retains both
signing origins. Do not roll back to a binary that lacks these readers.

Start with the operator-selected existing Agent. Record its native conversation,
runtime/Pod/PVC identity, profile, settings, pending work and transcript boundary.
The worker attempts at most two eligible migrations per reconciliation sweep.
Active turns, pending approvals, queued owner commands, lifecycle work and
active native goals defer maintenance. When API reattachment has no replayed goal
snapshot, explicit transport maintenance may prove absence by reading the exact
conversation's native SQLite thread/goal records through the existing fenced Pod
exec transport. Both databases are opened read-only; any goal row, unknown schema,
missing thread, symlink, unavailable probe or changed runtime identity defers.
The proof is checked again after admission and before native reload. This fallback
does not enable ordinary guidance refreshes or override an observed native goal.
Paused Agents are not awakened
for this operation; their normal recovery must reach a verified idle boundary.
Provider-managed conversations require the separately audited binding relay
below; the managed access capability is not interchangeable with a provider key.

The existing maintenance admission persists the exact service origin and fences
new execution. The adapter resolves the original capability, closes only the idle
native conversation subscription, and loads that same persisted conversation
with the canonical access endpoint. All other tool descriptors, selected model,
reasoning effort, permission, working directory and owner guidance are retained.
It acknowledges the installed transport only after reload and settings requests
succeed. A lost acknowledgement remains a pending operation and is replayed on
the same conversation before another message can execute. The process, Sandbox,
home and credential are not replaced.

Accept the canary only after observing a tool request reach the canonical host,
a successful reply in the original conversation, unchanged retained runtime/home
identity and preserved earlier history. Then expand the explicit ID list, or use
`["*"]` after reviewing inventory. Keep a per-Agent pending reason for busy,
paused or unsupported cases. Do not count a skipped or failed operation as migrated.

The transport change preserves the signing namespace used to issue the original
credential. Keeping that namespace in `CHRONO_AGENT_ACCESS_RETAINED_ORIGINS` is an
identity-validation requirement; it does not generate network traffic to the old
host after its installed transports have migrated. Removing a signing namespace
requires a separate audit of historical enrollment readers and credential
revocation, and is not necessary to stop use of the old domain.

Only retire the old direct API routes after all managed endpoint dependencies,
provider binding proxies, paused recovery paths, webhooks and retained external
clients have been accounted for. Migrating the managed Agents cannot rewrite
URLs stored in somebody else's SDK client. Browser navigation redirects remain
independent of this endpoint migration.


## Preserve the enabled production transport configuration

The production manifest records the reviewed revision-2 cohort of 27 Agent IDs
and the nine existing provider binding IDs enrolled in the canonical relay.
These are the same explicit selections applied during the migration; this
configuration does not broaden the cohort or wake paused resources. A stopped
or deleted Agent ID in the historical cohort does not recreate that Agent.

`CHRONO_PROVIDER_TOOL_RELAY` maps the enrolled bindings' original
`https://bot-gateway.chrono-ai.fun/mcp` authority to the fixed upstream
`https://cma.nyxid.ai/event-gateway/mcp`. Native tool requests use
`https://cma.nyxid.ai/api/v1/provider-tools`. The relay preserves the original
binding receipt and provider credential; the old endpoint is an identity match,
not the upstream used by the enabled relay. New conversations for those bindings
also enroll in the canonical relay.

Keep these settings through normal ConfigMap updates. Restoring the initial empty
relay setting after enrollment would remove authorization for the installed relay
capabilities. Keep a compatible API binary and the enabled relay for rollback;
disabling new managed migration admission does not require disabling the relay.
A first deployment into an environment that has not enrolled must still deploy
compatible readers, drain older workers, and then activate its reviewed cohort.
Do not apply the whole production ConfigMap merely to copy these two settings:
runtime image selection and other staged features have their own rollout gates.

Activation is not proof of completion. Count an existing Agent only after its
refresh is acknowledged and its current transport record identifies the canonical
origin. Check the original conversation/history and credentials before and after,
and verify discovery through the new host from the original runtime. Existing
old-domain routes and historical signing audiences remain for retained clients.
Their separate retirement is outside this migration's endpoint cutover.

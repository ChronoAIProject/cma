# CMA SDKs

CMA's integration documentation lives with the repository-only SDKs:

- [TypeScript SDK guide](../sdk/typescript/docs/README.md)
- [Rust SDK guide](../sdk/rust/docs/README.md)

Neither SDK is published to an external package registry. The TypeScript package
is private and consumed locally by `web/`; the Rust crate is `publish = false` and
consumed by path. Local package and scratch-consumer checks keep both exported
surfaces usable.

Both SDKs expose owner Bot management: TypeScript `client.bots` and Rust
`client.bots()`, with create, list, get, available, test, disable, enable, delete
and key rotation. Creation requires a retained idempotency key; create and
mutations return durable progress for polling. Bot tests are synthetic provider
checks and do not send channel messages. See the SDK reference guides for types.

Those files are versioned beside their SDKs and are the designated source for the
CMA Developer surface. The public wire contracts remain in the interface documents:
[REST](API_REST.md), [AG-UI](API_AGUI.md), [Responses](API_Responses.md), [PTY](API_PTY.md), [MCP](API_MCP.md),
and [A2A](API_A2A.md).

## Cross-origin browser deployments

A browser application served from a different origin than CMA needs an explicit
deployment grant. Add each application's exact origin to
`CHRONO_SERVER_CORS_ORIGIN`; multiple origins are comma-separated, for example
`https://chat.example.com,https://admin.example.com`. An origin is the scheme,
host, and optional port, without a path. The service keeps credentialed CORS
enabled and deliberately provides no wildcard-origin mode.

For a listed origin, preflights admit the `Authorization` request header, the
`Last-Event-ID` cursor used when event streams reconnect, `Prefer` for optional
replay fields, `X-Cma-Browser-Client` for browser-control ownership,
`X-CMA-Document-Share-Token` for independently authorized document deliverables,
and the supported HTTP methods, including `PUT`. Default snapshot reads omit `Prefer`
when no optional fields are requested. Browsers may cache that verdict for one hour. If an
origin is absent, the preflight `OPTIONS` response omits
`Access-Control-Allow-Origin`. The browser then reports a CORS or network failure
and does not send the bearer-bearing API request, so the SDK receives no HTTP
problem document. The failed `OPTIONS` request in the browser network panel is
the observable distinction from an API authentication or authorization refusal.

That grant covers AG-UI as well as REST. A cross-origin conversation frontend
must consume `POST /api/v1/agui/run` and `GET /api/v1/agui/events` with `fetch`
and an `Authorization: Bearer` header; native `EventSource` cannot set that
header. The TypeScript SDK's `agui.run` and `agui.replay` use `fetch` and a streaming
body reader for this reason. Native `EventSource` is limited to the same-origin
cookie path. There is no terminal plane or terminal-upgrade ticket in this
release. See [AG-UI Authentication](API_AGUI.md#authentication) and
the SDK's [TypeScript Authentication guide](../sdk/typescript/docs/authentication.md#browser-sessions).

## Independent CMA Event Gateway clients

Both SDKs also expose the separate gateway protocol: TypeScript `Cmaeg` and
`cmaeg`, and Rust `cma_sdk::cmaeg::Cmaeg`. Configure the gateway origin separately
from CMA. Management uses creator credentials; tools use the channel's agent key;
rekey uses the dedicated CMA service credential. Custom events support either
source-key bearer authentication or Standard Webhooks HMAC signing. See the
[protocol](CMAEG_Protocol.md) and each SDK reference for operation names.

The clients cover channel creation/read/recovery/update/delete/test/rekey,
structured custom events and bounded SSE observation, replies, message updates,
typing and persisted event context. Neither client follows redirects, retries a
mutation, reconnects a custom stream or parses an opaque reference. A lost create
can be reconciled through its reserved `record_id`; a creator idempotency key is
replica-local. Interrupted custom streams remain uncertain and do not authorize
resubmission.

Both SDK schema suites exercise all 94 snapshot types and detect a removed
property. The compiled-gateway fixture uses real gateway HTTP handlers with fake
NyxID, record persistence and provider endpoints. It proves both clients can
create/recover/update/test a channel, sign an event, retrieve 32 KiB context,
receive a confirmed explicit reply on a custom stream and delete the channel.
This is local protocol interoperability evidence, not real channel delivery.

Run `npm ci && npm run build` in `sdk/typescript/`, then run from `api/`:

```sh
cargo +1.96.1 test --locked -p cmaeg --test sdk_clients -- --ignored --nocapture
```

The harness starts the compiled gateway and run-owned loopback fakes and invokes
`sdk/typescript/scripts/cmaeg-fixture.mjs` plus the ignored Rust `cmaeg_fixture`
test. Each client runner requires `CMAEG_FIXTURE_ROOT` identifying the fixture's
loopback control server and starts no service itself. `CMAEG_SDK_TARGET_DIR` can
select an existing Rust SDK build cache. Synthetic credentials remain in process
memory and subprocess environment, and no real NyxID or messaging call is made.

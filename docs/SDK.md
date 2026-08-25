# CMA SDKs

CMA's integration documentation lives with the repository-only SDKs:

- [TypeScript SDK guide](../sdk/typescript/docs/README.md)
- [Rust SDK guide](../sdk/rust/docs/README.md)

Neither SDK is published to an external package registry. The TypeScript package
is private and consumed locally by `web/`; the Rust crate is `publish = false` and
consumed by path. Local package and scratch-consumer checks keep both exported
surfaces usable.

Those files are versioned beside their SDKs and are the designated source for the
CMA Developer surface. The public wire contracts remain in the interface documents:
[REST](API_REST.md), [AG-UI](API_AGUI.md), [PTY](API_PTY.md), [MCP](API_MCP.md),
and [A2A](API_A2A.md).

## Cross-origin browser deployments

A browser application served from a different origin than CMA needs an explicit
deployment grant. Add each application's exact origin to
`CHRONO_SERVER_CORS_ORIGIN`; multiple origins are comma-separated, for example
`https://chat.example.com,https://admin.example.com`. An origin is the scheme,
host, and optional port, without a path. The service keeps credentialed CORS
enabled and deliberately provides no wildcard-origin mode.

For a listed origin, preflights admit the `Authorization` request header, the
`Last-Event-ID` cursor used when event streams reconnect, and the supported HTTP
methods, including `PUT`. Browsers may cache that verdict for one hour. If an
origin is absent, the preflight `OPTIONS` response omits
`Access-Control-Allow-Origin`. The browser then reports a CORS or network failure
and does not send the bearer-bearing API request, so the SDK receives no HTTP
problem document. The failed `OPTIONS` request in the browser network panel is
the observable distinction from an API authentication or authorization refusal.

That grant covers AG-UI as well as REST. A cross-origin conversation frontend
must consume `POST /api/v1/agui/run` and `GET /api/v1/agui/events` with `fetch`
and an `Authorization: Bearer` header; native `EventSource` cannot set that
header. The TypeScript SDK's `createAguiTransport` uses `fetch` and a streaming
body reader for this reason. Native `EventSource` is limited to the same-origin
cookie path, and a terminal-upgrade ticket is not an alternative credential for
either AG-UI stream. See [AG-UI Authentication](API_AGUI.md#authentication) and
the SDK's [TypeScript Authentication guide](../sdk/typescript/docs/authentication.md#browser-sessions).

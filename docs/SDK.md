# The chrono-bot client SDK

A TypeScript client for chrono-bot's public interfaces, in `web/src/sdk/`.

## Why it exists

chrono-bot is a backend service whose clients are other products' frontends and
backends (`CLAUDE.md` §1). `web/` is the reference client, not the only one, and
`acp-kernel`'s stated exit criterion is that a product frontend can build a chat
interface on the public interfaces alone. Nobody had tested that claim.

Everything a client must know to drive a session lived inside the reference
client, in a form only the reference client could use. The specific obstacle was
not organisational but concrete: that code reaches the service exactly one way,
a same-origin path authenticated by an http-only cookie. That is right for the
dashboard — a proxy in front of the bundle forwards `/api/` onward, so no host
is compiled in and no token is ever handled in the browser — and it is unusable
for anybody else. A product frontend on its own domain, a backend calling from
Node, a test driving the real service: none are same-origin and none have the
cookie. So a second client could not reuse the code; it would have had to
rediscover the protocol.

It would also have had to rediscover it _correctly_, which is the sharper
reason. Two defects found on 2026-08-19 were both the client and the server
disagreeing about the wire while each was internally consistent:

- `STATE_DELTA` carries its RFC 6902 patches under `delta`; the client read
  `patch`, so every patch was silently dropped — taking usage, permission mode,
  the plan, and the markers that say a stretch of conversation was lost.
- Usage lands at `/usage/inputTokens`, `/usage/outputTokens` and
  `/usage/contextUsed`; the client read `/usage/tokens` and `/usage/cost`, so no
  session ever reported what it had spent.

Neither failed loudly, and the fixtures encoded the same wrong shape as the
code, so a green suite agreed with both. They are exactly the mistakes a second
client would make again independently. One module owning the wire, citing the
document that fixes each name, is how that stops recurring.

## What is in it

| Module        | What it owns                                                |
| ------------- | ----------------------------------------------------------- |
| `config.ts`   | Where a deployment is, and how to be allowed in             |
| `errors.ts`   | `ApiError`, and `CapabilityRefused` for a permanent refusal |
| `http.ts`     | One request, and how a failure is reported                  |
| `types.ts`    | The session model, as the control plane reports it          |
| `sessions.ts` | The REST operations on a session                            |
| `wire.ts`     | The wire's own field names and JSON-Pointer paths           |
| `client.ts`   | `createClient` — one configured client                      |
| `index.ts`    | The entry point, listing the whole surface                  |

## Using it

```ts
import { createClient } from "./sdk";

const bot = createClient({
  baseUrl: "https://bot-api.example",
  credential: () => tokenStore.current(), // read per request, so it may expire
  cookies: false, // cross-origin: do not send the browser's own
});

const session = await bot.createSession({ cols: 120, rows: 32 });
if (!session.capabilities.conversation) {
  throw new Error("this deployment did not grant this session a conversation");
}
```

Every default is what the dashboard needs — same-origin, cookies on, no bearer
token — which is why `web/src/api/client.ts` passes no configuration at all.
That is deliberate: the reference client is held to the same surface a third
party gets, so anything it needs and the SDK lacks fails the build rather than
being discovered by whoever integrates next.

## What it deliberately does not do

It renders nothing and depends on no framework. A client's own surface is its
own; this carries what the service said and what it means, and stops there.

It invents no operation. Where the service withholds something, this reports
what the service said. An SDK that papered over a refusal would make `CLAUDE.md`
§3's central distinction — "this session can never do that" against "that just
failed" — impossible for a consumer to act on.

## What is not in it yet

Stated rather than left to be discovered, per `CLAUDE.md` §9.

- **The AG-UI stream and the conversation fold** are still in `web/src/agui/`
  and `web/src/chat/conversation.ts`. Both are already framework-free and
  protocol-only, and the entry point re-exports them, so the remaining work is a
  move rather than a rewrite. Until it happens, `web/src/sdk/` is not copyable on
  its own.
- **`followSession`** — the provisioning progress stream — stays in the
  dashboard. `EventSource` cannot carry an `Authorization` header, so it works
  only for a caller whose credential is a cookie. Moving it as it stands would
  put an interface in the SDK that silently fails for every cross-origin
  consumer, which is worse than its absence.
- **The other three interfaces.** PTY, MCP and A2A (`CLAUDE.md` §3, priorities
  3 to 5) have no client here. The SDK covers AG-UI and REST, which are the two
  a frontend needs.
- **It is not published.** There is no package boundary yet, so a consumer
  outside this repository copies the directory rather than installing it.

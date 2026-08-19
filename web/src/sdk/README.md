# chrono-bot client SDK

A TypeScript client for chrono-bot's public interfaces. It is the one place
that knows the wire.

## Why it exists

chrono-bot is a backend service whose clients are other products' frontends
(`CLAUDE.md` §1). `web/` is the reference client, not the only one. Everything
a client must know to drive a session — the REST operations, the AG-UI event
stream, and how to fold that stream into something renderable — lived inside
the reference client, so a second client would have had to rediscover it.

It also had to be rediscovered _correctly_, which is the sharper reason. Two
defects found on 2026-08-19 were both the client and the server disagreeing
about the wire while each was internally consistent:

- `STATE_DELTA` carries its RFC 6902 patches under `delta`; the client read
  `patch`, so every one was silently dropped.
- Usage lands at `/usage/inputTokens`, `/usage/outputTokens` and
  `/usage/contextUsed`; the client read `/usage/tokens` and `/usage/cost`, so
  no session ever showed what it had spent.

Neither failed loudly. Both are the kind of mistake a second client would make
again independently. One module that owns the wire, with the documents it
implements cited beside each decision, is how that stops recurring.

## What is in here

| Module            | What it owns                                              |
| ----------------- | --------------------------------------------------------- |
| `types.ts`        | The wire's own vocabulary: sessions, capabilities, events |
| `events.ts`       | Reading an AG-UI frame into a typed event                 |
| `stream.ts`       | The durable stream: cursor, reconnect, deduplication      |
| `conversation.ts` | Folding a stream into a conversation a client can draw    |
| `sessions.ts`     | The REST operations on a session                          |
| `client.ts`       | The one entry point a consumer constructs                 |

## What it deliberately does not do

It renders nothing and depends on no framework. A client's own surface is its
own; this carries what the service said and what it means, and stops there.

It invents no operation. Where the service withholds something, this reports
what the service said — `CLAUDE.md` §1's rule for the reference client is a
rule for every client, and an SDK that papered over a refusal would make it
impossible to obey.

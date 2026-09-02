# CMA TypeScript SDK

`@chronoai/cma-sdk` is the typed browser and Node client for CMA. It is a
repository-only package, deliberately marked private, and is not published to npm.
CMA's web application consumes it through the local `file:../sdk/typescript`
dependency. Build and inspect the same local archive used by CI with:

```sh
cd sdk/typescript
npm ci
npm run build
npm pack
```

The archive is for local scratch-consumer validation, not registry publication.
The public client includes CMA's stateful Responses conversation resources and
single-turn folding SSE connection; its exact subset is documented in
[API_Responses.md](../../docs/API_Responses.md).

The complete, executable integration guide lives in this package:

- [Overview](docs/README.md)
- [Quickstart](docs/quickstart.md)
- [Authentication](docs/authentication.md)
- [Sessions](docs/sessions.md)
- [Streaming](docs/streaming.md)
- [Triggers](docs/triggers.md)
- [Operations](docs/operations.md)
- [Sponsorships](docs/sponsorships.md)
- [Errors](docs/errors.md)
- [Generated-reference entry points](docs/reference.md)

The files under `docs/` are the canonical documentation and the source consumed
by CMA's Developer surface. Every TypeScript fence there is extracted and compiled
against the package declarations in CI.

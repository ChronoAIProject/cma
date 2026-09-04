# CMA TypeScript SDK

This package currently exposes CMA's anonymous deployment discovery route and
the generic HTTP transport beneath it. Resource clients were removed with the
retired domain model in issue #801. The replacement public SDK is defined by
issue #819.

```sh
npm ci
npm run build
npm test
npm run docs:check
```

The published guides live in [`docs/`](docs/README.md).

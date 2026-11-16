# Document executable contribution

Issue #2149; exclusive owner Developer08. `index.mjs` exports eight cases and four
authentic setup providers. The cases preserve all 36 catalog steps. No source-only
or mocked test result is a document feature PASS.

| Row | Observable checks |
| --- | --- |
| browse | Actual Artifacts tabs, known-title search, 26-row owned pagination, shared/inaccessible controls, selected route identity |
| create | New document UI, real upstream acceptance followed by one dropped response, UI retry with identical key/body, one durable document and editable page |
| edit | Title and rich-text edits, acknowledged revision advance, peer-ingress reload, stale revision rejected without source replacement |
| conflict-draft | Two independent sessions, peer save then local 409, downloaded local Markdown, cancel discard, explicit discard, newer durable revision retained |
| comments-read | 26 real comments, actual More comments, author/source-revision attribution, read-only form absence and rejected writes |
| comment | Comment permission, UTF-8 byte limit, real lost-ack retry, same body/key, one comment with current source revision after reload |
| people-sharing | Both Can view and Can comment, recipient UI and direct denials, exact resource scope, owner revoke and both-ingress protected-read denial |
| link-sharing | Both offered link permissions, actual clipboard, isolated recipient session, token in fragment/header only, wrong-resource and edit denial, revoke on both ingresses |

The shared bridge calls `setups[name](ctx, primitives)` with the identities and
disposable database lease specified in `fixtures.json`. Setup uses only public
HTTP APIs through `ctx.request`. It registers cleanup before creating documents,
records each create intent before dispatch, and retains any acknowledged identity
even if subsequent request attribution fails. It never retries unknown setup
writes. Lost-ack browser tests forward the actual request, consume its real
successful response, and then abort delivery to the UI. They never fulfill a
route with a synthetic success.

There is no public document DELETE endpoint. The parent must reserve and track
the exact disposable database, including documents, comments and grants, and
withhold aggregate campaign PASS until database teardown is confirmed. The
provider's `release()` acknowledgment means cleanup responsibility was retained;
it is not a deletion receipt. If this parent capability is absent the cases are
BLOCKED. Browser contexts, route interceptors, clipboard contents and downloads
have their own reverse-order cleanup.

Run author and shared offline contracts after installing the harness's pinned
dependency with `npm ci --ignore-scripts --no-audit --no-fund --prefix
.github/scripts/multi-am-features`:

```sh
node --test .github/scripts/multi-am-features/executable/*.test.mjs \
  .github/scripts/multi-am-features/executable/cases/developer-08/contracts.test.mjs
```

These contracts cover exact registration, missing/unsafe setup, cleanup custody,
unknown write preservation and retry-key/body invariants. The sole parent must
still execute the real compiled browser on its selected image, before and after
its controlled restart. No second native, AM, Mongo or production caller is
started by this contribution.

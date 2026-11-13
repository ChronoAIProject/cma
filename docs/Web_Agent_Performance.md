# Browser performance and request pacing

The Agent uses its existing SSE tail for streamed output. Queue discovery is a
separate read: a durable event cursor is a hint that the queue may have changed,
not a reason to repeat discovery for every streamed event. Each queue observation
retains fresh authorization before and after the read. Queue edits, removal,
receipt recovery and input admission retain their exact journal identities.

Queue hints coalesce behind a two-second cooldown. A continuous stream cannot
postpone an already scheduled observation. Local receipts, turn transitions and
explicit mutation refreshes run after 100 ms unless a failed read requires a
longer retry delay. Transient failures
retain their exponential backoff, capped at ten seconds, even while new events
arrive. Errors remain visible with retained content.

A compatibility 404 from an older serving instance leaves discovery idle and
recoverable. Later stream hints, focus/visibility or explicit refresh can retry
without remounting the Agent. Ordinary hints retain the two-second cooldown;
there is no periodic retry solely because the optional feature is unavailable.
Previously retained rows and real error text remain until a successful refresh.

Retained action status starts with a one-second retry. An unchanged result backs
off through two, four, eight and ten seconds; observed progress resets the delay.
Terminal receipts stop observation. Hidden documents suspend these background
reads, and visibility/focus events coalesce into a fresh observation when the
cooldown permits. Live conversation delivery remains on its existing stream.

Composer width changes schedule one resize in the next animation frame. Writing
the textarea height inside a ResizeObserver delivery caused undelivered resize
notifications in WebKit. Typing still resizes synchronously, and unmount cancels
any pending width-driven resize.

## Reproduce the local waterfall

Build the web application with the repository's normal immutable build identity
inputs, then serve that compiled `web/dist` with `npm run preview`. Set
`CMA_UI_PERF_URL` to the loopback preview URL and run from `web/`:

```sh
npx playwright test --config e2e/ui-performance.config.ts
```

The configuration runs Chromium, Firefox and WebKit, with one worker and no
retries. `CMA_UI_PERF_OUTPUT` selects a local evidence directory. All API responses
come from the repository's recorded public fixtures or explicit synthetic public
boundary responses. No production account or credentials are involved. The
Agent fixture uses a real local HTTP event stream and the production SDK parser.

Each API/auth request has a fixed 100 ms synthetic boundary delay. `waterfall.json`
records method, path, phase, start/end, status and failure, without headers,
query values, request bodies or credentials. The cases cover Agent inventory,
opening an Agent with a previous transcript, send to first streamed token, one
minute of durable stream activity, cached return, Operations and Usage.

Shell/content marks indicate when Playwright observed the corresponding UI.
Interactive marks require typing into the composer or opening a filter menu.
These are operational readiness measurements, not Lighthouse TTI or production
latency claims. Compare like-for-like builds and fixtures; report sample counts,
browser and synthetic delay alongside any timings. A single timing difference
does not establish a statistically significant latency improvement.

## Boundaries that must stay intact

- Independent hierarchy, transcript, usage and first history-page reads already
  overlap. Recent transcript and shell publication must remain independent of
  delayed older history and usage. Cached return must not reread old history.
- Fresh identity checks surround projections. Concurrent callers may share only
  a request started after every caller asked for its check. A single in-flight
  request and one queued batch serialize transport identity reads; completed
  results and failures are never reused. Adjacent checks with no intervening IO
  may consume a one-use proof within 250ms when `IDENTITY_HANDOFF_ENABLED` is
  activated. That switch ships disabled. Preparation handoffs and mutations
  always require a fresh check, and local journal epoch guards remain enforced.
- The model catalog refreshes across turn transitions because available runtime
  capabilities can change. Its repeated reads have a correctness purpose.
- Sandbox availability, effective access and service-consent observations have
  their own freshness contracts. Queue pacing does not extend those deadlines.
- Usage report creation, pending-report retries and 429 backoff keep their
  existing server-scoped behavior. Real failures must remain visible.

Access-request observation uses two seconds for outstanding requests, 2/4/8/15
seconds for unchanged active turns and thirty seconds while idle. Durable
conversation cursors hint reads with a two-second minimum; focus, visibility and
explicit decisions refresh the observation. Failures retain visible requests
and retry at 2/4/8/10 seconds. The idle fallback is required because runtime
consent admission validates live lineage without requiring an active turn.
Hidden personal navigation trees do not fetch badges. A mounted account retains
only immutable public creation categories across its two trees; unavailable
categories retry every minute. Unchanged hierarchy reads back off at 15/30/60
seconds, with changes, explicit refresh and visibility restoring the base delay.

The local checks establish browser behavior. Release acceptance still requires
the release owner's checks against the exact deployed web artifact, including
asset initialization, the blank-page guard and retained assets. This frontend
slice does not close the backend capacity gate in #1543 or establish production
MongoDB capacity.

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

- The selected Agent route requests Open readiness alongside access
  classification. `POST /auth/connections/open` independently authenticates the
  caller and authorizes the current target and required services. Only the
  authorized personal view can consume the result, once, for the same client,
  account and Agent. Shared views never consume it. Navigation, access failure,
  account/client changes, blur and visibility changes cancel an unconsumed read.
  Its completed transfer expires within one second, or earlier when cached
  evidence expires. Retry-After and connection-read cooldowns still apply.
- Open alone may reuse successful service verification for at most 30 seconds,
  keyed by authenticated user and connection service. Each AM has at most 4,096
  entries; scopes above 64 services do not use the cache. A cache hit starts a
  fresh background requirements check, single-flight per user and bounded to
  four tasks per AM without a queue. Background work owns its completion so an
  early Open response cannot cancel OAuth refresh custody. A missing, inactive,
  unknown, malformed or failed check invalidates positive evidence. Generation
  guards prevent older results from restoring evidence after a newer refusal.
  Connection start/cancel also invalidates the user's entries. The cache holds
  no credentials and preserves the original verification timestamp. Responses
  are private/no-store and explicitly distinguish cached from fresh evidence;
  the browser subtracts response transit time from the remaining lifetime.
  Cold/expired cache entries and a first visit to another AM require a fresh
  check. This does not promise a fast cold Open or a production latency target.
- **Security invariant:** cached Open readiness never authorizes Send. Every
  Send, including an already-live V2 session and a prepared creation handoff,
  waits for `POST /auth/connections/check` to freshly verify the connection.
  A revoked/missing/unknown connection or a failed check releases no command;
  the fresh endpoint and action gate reject cached-marked responses. Thus a
  revoked connection cannot be used on the strength of an old Open result.
  Transport identity checks, server effect checks and durable command custody
  remain in place; repair never automatically replays a message with a new key.
  Send includes the fresh verification latency. Browser readiness is not an
  atomic guarantee against revocation after the fresh check: provider and
  server enforcement remain necessary at actual use. This change adds no new
  server credential authorization for Provided/Binding credential sources.
- Independent hierarchy, transcript, usage and first history-page reads already
  overlap. Recent transcript and shell publication must remain independent of
  delayed older history and usage. Cached return must not reread old history.
- Fresh identity checks surround projections. Concurrent callers may share only
  a request started after every caller asked for its check. A single in-flight
  request and one queued batch serialize background transport identity reads.
  Cancel submission and recovery use a separate gate shared across all Agent
  sessions, also bounded to one request and one queued batch. A slow background
  check cannot delay Cancel's fresh checks before observation, before submission
  or after its receipt. Ordinary Send, admission-conflict retries and input
  recovery use their own shared gate with the same bounds, including the fresh
  check after a receipt. At most three transport identity reads run concurrently;
  account comparison, journal epoch guards and invalidation apply to all gates.
  Completed results and failures are never reused. Adjacent checks with no intervening IO
  may consume a one-use proof within 250ms when `IDENTITY_HANDOFF_ENABLED` is
  activated. That switch is enabled for adjacent read-only continuations. Preparation handoffs and mutations
  always require a fresh check, and local journal epoch guards remain enforced.
- Responses v2 control readiness follows the authorized live head, independently
  of history length or transient history-page failures. Stop uses its recorded
  response binding; queue discovery keeps its own fresh authorization checks.
  Only live approvals are visible before history completes. Desktop takeover
  still confirms and waits for interruption of an active turn. Missing heads,
  account changes and access review fail closed; legacy AG-UI retains its full
  history gate. `LIVE_HEAD_CONTROLS_ENABLED` in `controlReadiness.ts` can be set
  to `false` to restore the previous history gate; production rollback uses the
  normal Web generation rollback. This changes no server admission checks.
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

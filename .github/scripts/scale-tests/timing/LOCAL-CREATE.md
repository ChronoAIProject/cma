# Local create timing

`local-create.mjs` runs real plain and desktop creates against an isolated local
fixture. Developer01 supplied the runner and the lifecycle/composer/cleanup
helpers copied from prod-verifier02; developer17 integrated it and corrected the
visible desktop observer. `lib/core.mjs` is the existing shared timing helper.
No response, WebSocket, runtime, or desktop frame is mocked during a measurement.

Execution requires an owner-provided local fixture with runtime pools and image,
ACP/model startup, synthetic provider and identity, custody, and a Web app on the
same loopback origin as the API. `/ready` alone is insufficient. Coordinate one
window with the environment owner before running; this script creates and
retires real local Agent/Sandbox resources. It is not the full scale-suite driver.

Copy `local-config.example.json` to a private location and fill the actual fixture
inputs. The browser storage state must be a private regular file for the synthetic
identity. Use a new output directory for each run. Keep candidate API, runtime and
Web source/image details in the `source` field. Then run:

```sh
CMA_LOCAL_CREATE_CONFIG=/absolute/private/config.json node timing/local-create.mjs
```

Timing starts before create submission and includes journal IO, 250ms lifecycle
polls, API operation success, Agent Ready and Sandbox active, navigation, and an
editable composer with Send enabled. The draft is cleared without sending it.
Desktop completion additionally requires the product ready marker, an opaque
center canvas pixel inside the viewport, and Chrome's visibility/occlusion check.
Hidden ancestors, transparent canvases, clipped centers and loading overlays
(including `pointer-events:none`) cannot complete the observer. Opaque black can.

This conservative observer reports an upper bound on visible completion, including
browser scheduling and visibility observation; it does not timestamp the server's
durable commit or the first remote pixel. Chrome must support `checkVisibility`
and IntersectionObserver visibility tracking. Unreadable canvases and unsupported
observers do not qualify. Server logs are needed to split nested startup phases.

`results.json` retains every attempted sample and its phase timings. The summary's
acceptance field is only a local absolute-target check, requiring at least ten
complete attempts and no cleanup/browser-close failure. It is not an A3 matched
no-regression verdict or production acceptance. Small diagnostic runs remain
NOT_PROVEN. Plain/desktop order alternates and a failure stops the next create.
Cleanup verifies the same identity and retires only resources returned by this
create. Private cleanup journals contain the IDs needed for recovery; uncertain
outcomes retain them. Do not copy those journals into public reports.

The focused tests start headless Chrome with in-memory pages, without an
application, identity, database or network fixture:

```sh
CMA_CREATE_PLAYWRIGHT_MODULE=/absolute/path/to/playwright-core/index.mjs \
CMA_CREATE_BROWSER_EXECUTABLE=/absolute/path/to/chrome \
node --test timing/local-create.test.mjs
```

These tests verify the observer and local input/summary behavior. They do not
provide create latency results. `npm run lint` also checks the timing sources.

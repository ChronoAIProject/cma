# Agent Manager and web rolling deployment

The November 2026 deployment inspection found Agent Manager configured with one
replica and `strategy: Recreate`. Kubernetes therefore removed the serving API
before starting its replacement. Process-local request budgets and visitor Launch
reservations were the documented reason. Web already used the default rolling
strategy, but neither its manifest nor the API policy provided sufficient endpoint
propagation and connection draining. The API also ended HTTP draining after 30
seconds despite accepting 660-second Gateway connections.

Operation budgets now use majority-acknowledged Mongo GCRA buckets in
`request_rate_limits`. Database time controls refill and Retry-After. A local guard
still refuses obvious excess before database I/O; pre-auth and health guards remain
per pod. Failure to check a shared operation budget returns 503. It never grants a
fresh local budget as a fallback. Millisecond refill rounding is conservative.

Visitor Launch allowances commit in the existing Launch admission transaction.
The current platform limit, owner and UTC day determine admission. A shared
`visitor_launch_days` row serializes concurrent replicas. Existing same-day visitor
Launches seed the count during migration and remain a lower bound. Replay returns
the existing Launch without consuming another unit. Failed transactions consume
nothing; HTTP cancellation cannot release an already committed admission.

API and web retain one steady replica, permit one surge replica, set zero
unavailable replicas and require 30 seconds of stable readiness. GKE's NEG readiness
gate must also be healthy. A 30-second preStop delay allows endpoint removal to
propagate while the old process still serves traffic. Both backend policies set
660-second connection draining. API HTTP draining has the same bound and keeps
background workers alive until HTTP finishes; workers then receive their existing
30-second cooperative drain. The 735-second pod grace includes propagation, HTTP,
workers and margin. Web requests nginx graceful quit after propagation and keeps
the hook running until its master exits. Existing versioned web asset retention
must be preserved during image promotion.

Inspect the gate's reason, not only its boolean. Production inspection also found
the legacy `cma-api` Service alias advertising an unused standalone NEG. GKE marked
the shared Pod gate true with `LoadBalancerNegWithoutHealthCheck` for that alias
while the public API was unavailable. Verify that no Gateway or other backend
references the alias NEG, remove its stale `cloud.google.com/neg` annotation, and
wait for its endpoints to detach. Preserve the alias Service and DNS for existing
ChannelPointers. The canonical `cma-agent-manager` NEG must remain attached and
healthy; verify its actual backend health before switching traffic. A readiness
gate satisfied by an unhealthchecked alias is insufficient evidence.

Event Gateway retains its existing zero-unavailable rolling strategy and durable
turn drain, raised to 660 seconds to cover the full request window, with the same
30-second readiness/endpoint margin, 660-second backend
connection drain and 735-second pod grace. Image Manager declares those rollout
and drain margins for activation; this change preserves its zero replica count
and does not activate an unpublished image.

These settings do not migrate or rotate Sandbox runtimes. Existing Sandbox,
Agent, home and conversation IDs remain independently durable. All other process
authorities still need their existing leases and writer fences during overlap.

## Retained Web asset inventory contract

`infra/web/deployment.yaml` must preserve the complete, ordered live asset
initializer list. Recreating a Pod from an incomplete list leaves older browser
tabs without their lazy chunks, even when the current page and readiness pass.
The reviewed production snapshot is recorded in
`.github/fixtures/web-retained-assets.json`: Deployment identity and generation,
capture time, count, full-list SHA-256 and the documented rollback image digest.
The fingerprint covers every initializer field, including commands, mounts,
resources and Kubernetes defaults. Mapping key order is ignored; list order is
significant. The Manifests CI job checks this baseline and runs mutation tests.

Before a parity update, use the approved bounded read-only Deployment path to
capture the complete `spec.template.spec.initContainers` array. Retain only the
static asset initializer specification and resource metadata, never a raw
Deployment or credential-bearing env. The private JSON projection contains
`read_only: true`, `deployment: {name: "cma-web"}`, and `initContainers`. Compare
the manifest with that fresh evidence as well as the reviewed baseline:

```sh
ruby .github/scripts/web-asset-retention-contract.rb "$web_initializer_projection"
ruby .github/scripts/web-asset-retention-tests.rb
```

When a verified release intentionally adds assets, update the full manifest
list and the baseline count/fingerprint from the fresh capture in the same
reviewed parity commit. Derive the fingerprint with
`Digest::SHA256.hexdigest(JSON.generate(WebAssetRetention.canonical(items)))`
from the contract module. Retain the separately documented rollback image;
changing or removing retained releases needs an explicit retention decision.
Record the new capture time and generation. Do not regenerate a baseline merely
to accept an unexplained difference. CI protects the reviewed snapshot against
repository edits; it cannot discover later out-of-band production drift, so the
fresh live comparison remains a release/parity gate.

## Verified Train E pins (2026-11-11)

The parity snapshot follows VERIFIED Train E: API generation 177 and Web
generation 209, from develop source
`304b44fd9b9d75d282399ccd6ea34e035eb632ca`, tree
`2f9ca29be48da347d2619e4383195e43e651963f`, CI run `42886990359`,
attempt 1 (25 successful jobs). The retained CI artifacts were promoted without
rebuilding; destination manifests, configs and layers were verified.

| Component | Running image digest                                                      | CI artifact   | Native archive SHA-256                                             |
| --------- | ------------------------------------------------------------------------- | ------------- | ------------------------------------------------------------------ |
| API       | `sha256:9dd19848a67403165777e63db377f9ff129a66052762f0e5efd3fc020d522f36` | `13798883729` | `74f9972ebda1befde6ae081c59a3af4da33116aea8724acb21a7b1c16e097bed` |
| Web       | `sha256:065109527c1fb775ef66241c80b0d23090833901e31c5ae04402fa73e2f89de4` | `13798275521` | `2c7a2e39397f7b6a88e9d71b115b56b783fe42635436969a128d260991643e4f` |

Fresh bounded Deployment reads confirmed both pins and their existing
Deployment/template annotation projections. Image-only rollouts leave historical
annotations in place: those are copied exactly, not rewritten as current-image
provenance. The source/artifact matrix above identifies the running images.

Web retains all 63 ordered initializers, including the incoming release and the
preceding rollback image
`sha256:f85b24cea3bb0ed96d9caa34db37d2d0fd43472d74ef9f98872d0f6ccff6952c`.
The earlier `f9d6bb051767...` assets remain in the inventory as well. The baseline
now independently requires the immediate predecessor. This records retained
assets; an actual rollback still needs the release owner's compatibility gates.
API requests remain CPU `4` / memory `256Mi`, limits CPU `8` / memory `8Gi`,
with `TOKIO_WORKER_THREADS` read from `limits.cpu` using divisor `1`.

API's full observation passed with all 14 previously healthy unchanged-runtime
Agents publishing within the deadline. Web's 51 observations span 1,859.862
seconds, including asset/build checks at 0/5/15/30 minutes and zero restarts.
Intercepted browser fixtures do not constitute authenticated owner acceptance.
Future trains require new artifact/readiness receipts and a fresh parity capture.

## First migration from the old deployment

Changing a Deployment does not retrofit a preStop hook or longer grace into its
existing pod. The first promotion must therefore explicitly drain the old API
endpoint while retaining its process long enough for existing connections. Do not
assume the new pod template protects the old pod. Prepare the replacement from a
verified immutable CI image, prove readiness and quota behavior, route new requests
to it, retain the old endpoint for the complete connection-drain window, then
finish the canonical Deployment rollout. Preserve current image/runtime pins and
compare the live resource versions before each mutation so concurrent releases
are not overwritten.

Include both `cma-agent-manager` and the `cma-api` compatibility Service when
moving new requests onto the replacement. Preserve their original selectors for
restoration, and retain the old process until both Service endpoint sets and the
canonical NEG have removed it and the full drain interval has elapsed. Copy all
existing web asset init containers, retain the outgoing image's assets, and add
the incoming image's assets to the shared asset volume before routing web traffic.

Stage the temporary web Deployment with the outgoing web container image and the
incoming asset initializer. It serves the old page while both asset versions are
available. Verify the initializer's immutable image and every incoming asset hash
in the served volume. Route traffic to this staged pod and drain the original web
endpoints before promoting the canonical Deployment to the incoming web image.
Both remaining backends then serve the new page's assets during overlap. Retaining
old assets only in new pods does not protect a new page whose asset request reaches
an old pod. This staging order also applies to subsequent web upgrades.

The old binary cannot participate in shared request budgets. Its remaining
in-flight Launches can race the initial shared-quota observation. Route new
admissions exclusively to the upgraded API before ending migration, monitor this
bounded overlap, and retain the durable Launch count as the migration floor.
Subsequent rollouts of compatible binaries use the normal shared authorities.

Legacy workers also lack usage writes. Usage source watermarks detect their
unmetered transitions during this overlap, exclude uncertain intervals and retain
an explicit partial baseline before continuing. Keep billing disabled throughout
the first migration and verify coverage before enabling provider delivery.

Verify public web, API readiness, authenticated reads and active conversation
replay continuously during the promotion. Compare retained Sandbox runtime/home
identities and Agent IDs before and after, and check worker recovery and usage
interval continuity. A manifest validation or a Ready pod alone is not evidence of
client continuity. These notes describe the required rollout; they are not a
production deployment record.

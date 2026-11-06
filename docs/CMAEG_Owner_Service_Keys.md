# Event Gateway personal owner service keys

The optional owner-key lane accepts a freshly issued personal NyxID key as the
Bearer credential for existing Event Gateway management routes. It does not
require a prior CMA browser login. NyxID must verify the key for the configured
Event Gateway service UUID and return the current owner permissions. The route
still enforces its method permission and resource ownership. Browser/OAuth
sessions and channel worker authentication keep their existing contracts.

This feature is disabled by default. It requires Gateway-owned Mongo storage on
a replica set, durable worker credential tracking, the Manager classification
endpoint, and an explicit operator cutover. It is unavailable with the legacy
remote record store. Do not enable it just because a current channel scan is
empty: deleted, replaced and failed-admission credentials are not recoverable
from that scan.

## Authentication and custody

Every owner request and live activity refresh performs fresh service-scoped
NyxID introspection and reads both worker custody authorities. Manager classifies
its current and retired binding credentials through
`POST /api/v1/cmaeg/worker-credentials/classify`; Gateway uses its permanent
`eg_worker_credential_uses` collection. Classification errors, malformed replies
and unavailable stores refuse access rather than treating a key as unknown.
Owner authentication has no successful-key or negative-classification cache.

The introspection request sends `token_type_hint=api_key` and `service_id` to
`/oauth/introspect`, using the existing confidential Event Gateway client. The
response must have the exact issuer and service audience, `api_key` type, active
state, proxy scope, personal subject, explicit effective permissions, parent-key
issue time and key ID. A supplied expiry must be valid and current; omission
means that NyxID verified a nonexpiring key. Resource-restricted access tokens use
the existing OAuth lane instead. Keys cannot select `X-CMAEG-Owner` or acquire
organization authority. Refused key-shaped credentials never fall through to the
browser/legacy token verifier.

A credential permanently loses owner-key eligibility when it is handed to any
configured worker provider or admitted in a sealed Channel. Gateway registers
its SHA-256 digest before provider transport, including bind, preparation,
conversation, response, replay, context and media calls, and before Channel
writes or supported offline import. Default, additional and pending consumer
credentials are included. Failed provider calls, record CAS failures, rotation
and Channel deletion never remove the fact. Only hashes are stored, not raw keys.
A bounded cache can avoid repeat writes after a durable positive registration;
owner requests always read the authoritative store.

Registration uses majority+journal acknowledgement. Gateway exclusion reads use
primary linearizable unique `_id` lookups and bounded deadlines. The application exposes only history insertion and reading, with no history
update/removal API or TTL. The optional isolated runtime role enforces those
limits at the database as well. Existing shared-schema deployments retain their
trusted backend credential, whose inherited database-wide permissions are broader;
they do not claim database-level append-only enforcement or account isolation. This is separate from the Manager's existing binding custody and does not
grant Gateway access to Manager collections.

## Rollout

Use the released CMA binaries and the existing secure environment injection;
do not paste database credentials or owner keys into commands or logs.

1. Deploy the compatible Manager classification endpoint. It reuses existing
   renewable Gateway-to-Manager service authority with both
   `cma:provider:bindings` and `cma:gateway:records`. It returns only explicit
   `known_worker`/`unknown` classification for a canonical SHA-256 digest.
2. With owner keys disabled, install the additive schema and role grants. Generate
   the optional roles with `cmaeg --print-worker-credentials-runtime-role` and
   `cmaeg --print-worker-credentials-migration-role`; grant them in addition to
   the corresponding existing Gateway roles. Run
   `cmaeg --migrate-worker-credentials` under migration authority. This preserves
   the base, thread-fence, WhatsApp-form and media-continuation migration markers.
   Runtime startup refuses unexpected or expiring history indexes. Shared-schema
   deployments can retain their existing database credential and strict service
   collection ownership; its inherited readWrite/dbAdmin authority remains
   operator/backend trust, not a claim of database account isolation.
3. Set `CMAEG_WORKER_CREDENTIAL_TRACKING_ENABLED=true` on **every** Gateway writer,
   importer and audit tool; retain `CMAEG_OWNER_KEYS_ENABLED=false`. Deploy and
   drain all earlier writers, including operational tools. Confirm provider
   handoffs and sealed Channel admissions record credentials before dispatch.
   A rollout of one replica is not evidence that all writers are covered.
4. Under the combined existing Gateway migration and optional worker migration
   authority, with the same master key ring, run
   `cmaeg --attest-owner-key-cutover <exact-40-character-writer-commit> --all-writers-drained`.
   The flag is the operator's explicit attestation that old writers are drained;
   the command cannot discover or prove all external writer processes. It
   backfills current retained Channels, chooses a conservative cutoff from DB
   and local clocks, archives the attestation, and advances the durable active
   pointer with compare-and-swap. It also creates the permanent startup barrier
   that refuses an untracked new writer/importer. Backfill is not a reconstruction
   of missing historical custody. Save the returned nonsecret attestation receipt.
5. Verify the actual NyxID confidential client is allowed to introspect personal
   keys for the service UUID. Catalog visibility alone does not prove that
   registration. Set `CMAEG_OWNER_KEY_SERVICE_ID` to the exact UUID and
   `CMAEG_OWNER_KEY_CUTOVER_ID` to the returned attestation ID. Then set
   `CMAEG_OWNER_KEYS_ENABLED=true`, retaining tracking on all writers.
6. Use a new owner key whose NyxID parent-key `iat` is strictly after the returned
   `not_before` timestamp. Verify scoped management and activity access, denied
   cross-owner reads, method permissions, revocation, and worker demotion. The
   same pre-cutover key reported in issue #1225 remains refused. Issuing a new
   child from an old parent does not evade this boundary.

The four settings are declared in `infra/cmaeg/configmap.yaml`; both enable flags
are false and both identifiers are empty by default. Neither migration nor
cutover runs automatically at server startup. A failed or uncertain attestation
must be inspected in the durable archive/active pointer before retry. A later
attestation has a new ID and strictly later cutoff; it cannot widen eligibility
to older keys.

## Rollback and limitations

Disable owner-key acceptance before restoring any untracked writer. Keep the
history, cutover archive, active pointer and migration barrier. New tracking-aware
binaries refuse startup/import with tracking disabled after the barrier exists.
An older binary cannot understand that barrier: deployment control must prevent
it from running while owner acceptance is enabled. Re-enablement after any gap
requires all writers tracked and a new later cutover, or independently proven
complete gap reconciliation. Never delete the ledger or reset its cutoff to make
a key eligible. Previously accepted owner keys may need replacement after a new
cutover.

This is an existing HTTP Bearer authentication lane. It does not change NyxID
code, add a signed-header protocol, or make an `x-api-key`-only MCP transport
forward a Bearer token. NyxID integration configuration and client transport must
actually deliver the supported credential. Worker keys retain their existing
channel tools; they do not gain owner management. No existing worker credentials,
Channels, Sandboxes or running Agent work need rotation or restart merely to
deploy tracking with owner acceptance disabled.

Unit/socket tests cover strict identity evidence, every-request exclusion,
provider-before-handoff ordering, sealed consumer coverage, owner permissions,
cross-owner refusal and live-stream revocation. The ignored Mongo integration
test runs in CI against the existing authenticated disposable replica set via
`CMA_IM_TEST_MONGODB_URI`, selected exactly by
`.github/scripts/image-manager-mongo-contract.sh` (and its local Python fallback).
The shell runner retains a local source-build mode; CI uses its extracted-archive
mode described in [Backend CI archive](Backend_CI_Archive.md).
It checks separate-client visibility, exact runtime grants, retained markers and
the startup barrier. Manager current/retired classification tests are selected by
the existing `test(store::bindings::)` group in `mongo-contract-tests.sh`. These are acceptance tests,
not a claim that an environment has been activated or production-verified.

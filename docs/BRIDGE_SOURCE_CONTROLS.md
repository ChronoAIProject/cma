# Bridge source-control consent

`GET /api/v1/bridge-invoke-grants/{id}/source-controls` returns
`{ "policy": null }` before any consent is recorded, or a safe current policy.
`PUT` on the same path requires `Idempotency-Key` and returns exactly HTTP 200 with
`{ "id", "request", "result" }`: an immutable receipt and its original policy
revision. Policy IDs use `brscp_` plus 64 lowercase hexadecimal characters;
receipt IDs use `brscc_` plus 64 lowercase hexadecimal characters. Invoke grant IDs
use the existing `brinv_` plus 64 lowercase hexadecimal characters.

```json
{
  "expected_bridge_revision": 2,
  "expected_grant_revision": 1,
  "expected_policy_revision": 0,
  "action": "allow_sender_cancel_and_steer"
}
```

A policy exposes `id`, `grant_id`, `owner`, original `pin`, exact creation `target`,
`revision`, `enabled`, `scope`, `approval_mode`, `updated_at` and actual human/API
`updated_by`. Responses omit resource audit details, credentials, custody,
connector observations, request digests and runtime identities.

Consent is recorded separately for each invoke grant. A missing policy is required
null, whereas a revoked policy keeps its identity and revision. Only an explicit
request records or revokes consent; reads never do so. Current Bridge management
and exact target management/interaction authority remain required.

The request contains exactly four fields: `expected_bridge_revision`,
`expected_grant_revision`, `expected_policy_revision`, and `action`. Bridge revisions
are positive signed 64-bit values (safe integers in TypeScript); grant revisions are 1
or 2. Absent policy uses revision 0. Allow accepts policy revisions 0 through 254;
revoke accepts0 through 255. Policy result revisions are 1 through 256, with 256
reserved for revocation. Actions are `allow_sender_cancel_and_steer` and
`revoke_sender_controls`. Scope is always `source_sender`; approval mode is always
`dashboard_only`. There is no approval action in this consent API.

Keys contain 1 through 200 visible ASCII characters without spaces. Retain the
exact request and original key after cancellation, response loss, 409 or 503.
Recovery rechecks current authority and returns the original receipt before
fresh revision checks, including after policy or invoke-grant revocation.
Recovering an older enabled receipt does not re-enable the current policy.
Read the policy separately when current state is needed. New enablement of a
revoked invoke grant is refused; explicit policy revocation remains available.
Neither SDK retries mutations automatically.

An enabled policy records consent only. It does not establish current connector,
source-sender, exact-turn, retained-runtime, custody or admission authority, and
does not enable cancel/steer execution, source-reference tools, channel approvals,
reply producers or automatic notices. No resource, invoke grant or conversation
is rewritten by recording this policy.

Malformed JSON, unknown fields/actions and invalid bounds receive typed client
errors. Foreign or missing resources are concealed with 404. Stale revisions,
changed same-key requests and new consent on revoked grants receive 409. An
unconfigured or unavailable service receives 503 with Retry-After. Every response
uses `Cache-Control: private, no-store` and `X-Content-Type-Options: nosniff`.

The shared public corpus records real REST handler behavior, including required
null absence, accepted/revoked/current policy, historical receipt recovery,
foreign and anonymous callers, malformed bodies, explicit key failures, numeric
bounds, unconfigured services, and revoked invoke-grant recovery. SDK transport
checks preserve the exact request and key through unknown outcomes.

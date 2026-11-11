# Dedicated production acceptance

Part of #1549. The standard-library runner in `.github/scripts/acceptance/`
checks a declared production candidate using two dedicated ordinary accounts.
It does not deploy, modify a Trigger, grant roles, change shared settings, add
funding, or borrow another tenant's resources. Running it requires explicit
owner authorization for test-account writes and ordinary CMA OAuth consent.

## Private state and evidence

Credentials, cookies, returned OAuth requests, resource identifiers, fixture
contents and durable operation keys belong only in `~/.config/cma-accept/`.
The directory must be 0700, its JSON files 0600, and it must be outside all
repositories. Never pass tokens in arguments, print private state, or attach it
to an issue. Account subjects may be placed in the separately requested 0600
private manifest; credentials may not. Aggregate evidence is written to a new
0600 output file with response hashes, sizes, statuses and fixed case names.

`nyxid_accounts.py --manifest /absolute/private/manifest.json` uses NyxID's
documented ordinary-user admin creation path with the operator's existing local
CLI session. It does not create or assign roles. An unresolved creation is
retained for exact-account recovery instead of blindly creating another user.
Accounts must have the ordinary CMA capabilities agreed with the owner before
probes begin. Admin-created accounts do not prove public signup or default-role
enrollment. The runner never adds credit or changes payment instruments.

Prepare `acceptance-trigger.json` in the secret directory with `public_id` and
its current `public_descriptor`, using the owner's approved published Trigger.
For the November 11 campaign this is `nightly-smoke-readonly`, discovered from
GitHub environment `staging-e2e-readonly` / `CMA_E2E_PUBLIC_ID`. Do not copy its
resource identifier into source. The runner pins the complete public descriptor
and refuses drift or software-trust disclosures. Its reuse policy is fixed;
unsupported J1/J2 variants stay blocked.

## Execution

Run phases serially from the repository root. Select a fresh evidence filename
for every invocation; existing evidence cannot be overwritten.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s .github/scripts/acceptance -p 'test_*.py'
PYTHONDONTWRITEBYTECODE=1 python3 .github/scripts/acceptance/runner.py \
  baseline --output /absolute/evidence/baseline.json
```

The phases are:

| Phase | Behavior |
| --- | --- |
| `baseline` | Ordinary identity and capabilities, admin refusal, inventory, anonymous/invalid authentication |
| `documents` | Two tiny owned documents; allowed/foreign reads, writes and lists; grant, revoke, stale grant receipt replay |
| `updates` | Owner edits and stale-revision refusal, with exact source comparison in memory |
| `sessions` | Revoke/replay one test session, then restore an ordinary session for remaining work |
| `usage` | Bounded owner report query, metadata/rows/series controls, foreign report concealment |
| `domains` | Canonical/retained discovery, authenticated REST and exact CORS, with redirects refused |
| `consent` | Only owner-approved normal CMA sign-in scopes, signed returned consent request, exact callback and client binding |
| `journey` | Existing Trigger Launch, owner/foreign Launch reads/lists, bounded preparation, resource proofs when available, cancellation/deletion cleanup |
| `resources` | Owner/foreign Sandbox/Agent read, list, write-refusal and history-export probes, or explicit missing-fixture blocks |
| `finish` | Revoke only each test account's CMA consent/cookie and all its own NyxID sessions; verify stale credentials are denied |

The ordinary consent scopes are exactly `openid profile email roles proxy
offline_access`. The runner grants no additional connected-service or node
access. It does not establish missing model/service connections. Consent
evidence may contain the account hash, CMA client and scope list only. CMA's
server receives its own OAuth refresh token through the supported callback;
local NyxID session credentials remain in the private store.

Execute `finish` at the end of the acceptance window. The final known state is
signed out, with temporary CMA consent revoked. A later campaign requires a
deliberate new ordinary sign-in; do not run another login after final cleanup
and continue claiming that state. Synthetic documents have no supported delete
API and remain private with sharing revoked. Report this retained footprint.

## Bounds and restart behavior

The runner holds one file lock, uses one request at a time, spaces requests by
at least 1.05 seconds, limits each phase to 500 requests, and uses ten-second
socket timeouts. Request bodies are at most 64 KiB and responses at most 1 MiB.
No request may start during 02:00–02:59 SGT. Normal API traffic never follows a
redirect; OAuth explicitly validates its exact endpoints and callback instead.

Original mutation bodies and keys are saved before dispatch. Uncertain calls
reuse the original key, completed writes are returned as **retained write
receipts**, and a changed body is refused. The one exception is the documented
credential-setup refusal: after successful ordinary consent, its refused receipt
is retained and a new attempt/key is counted. There is no automatic fresh-key
retry of provisioning or Agent input.

The approved visitor cap is at most one live Sandbox and five Launch attempts
per SGT day per account. The runner reserves attempts durably and ordinarily
creates one Launch per account, with the single explicit post-consent retry.
It refuses unexpected existing Sandboxes. Launch polling and deletion polling
are each bounded to 24 observations spaced five seconds apart. Cleanup executes
even after a probe failure. A pending deletion is a blocker, not a successful
teardown. Only the resource hash enters evidence.

## Interpretation

Preserve original artifacts for harness mistakes and publish the exact source
contract, corrected expectation/query and successful rerun. Do not classify a
different denial code as an isolation defect without checking the deployed
source. Unexpected protected success or changed owner state is a security bug
and stops dependent work. Domain checks continue independent observations so
one failing edge does not conceal another; their process exit remains nonzero
if any case fails.

The runner is an application smoke suite. Its resource branch requires live
resources from the approved Launch and runs before teardown. It does not prove
every route, all organizations/delegations, the provider-I/O revocation race,
full receipt/byte capacity, production overload behavior, or network/controller/
volume isolation. It cannot verify fixes absent from the deployed source or
replace matched telemetry windows for database contention and failure pacing.
API usage checks do not qualify browser continuity or shared spend policies.
Issue closure requires the complete issue acceptance criteria and candidate-bound
evidence, not the aggregate pass count alone.

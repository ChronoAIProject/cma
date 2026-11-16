# M-FUNC-01: fresh NyxID identity, J0–J7

This is the developer-14 journey contract for #1536, using PR #2200's C275
`executeCase` runner. It is on `tl/cma-13b/journeys-j0-j7`. The contract revision
addresses SA-04's JID-01–06 findings and is submitted for source re-review.
**No local runtime PASS or M-FUNC-01 closure is claimed.** Offline tests use
explicitly synthetic unit data and cannot substitute for the real journey.

## Parent invocation and release admission

The existing sole C275 parent imports `invokeJourney` from `index.mjs`:

```js
const result = await invokeJourney({
  binding,        // disposable kind context/namespace and exact local origins
  release,        // independently frozen 13b source/image + compiled web artifact
  trust,          // approved nonproduction NyxID capture key, issuer, field mapping
  lifecycleTrust,// approved independent lifecycle-capture public key
  environment,    // existing C275 transport/adapter/fixtures/record + finite budgets
});
```

`environment` follows `executable/runner.mjs`: `phase`, `iteration`, `am`,
`source`, `image`, `campaignId`, `transport`, `adapter`, `fixtures`, `record`,
`caseBudgetMs`, `cleanupBudgetMs`. The parent supplies a finite journey budget
(for example 1,200,000 ms) and cleanup budget (for example 180,000 ms).
The result from `executeCase`, including cleanup, is the verdict. The return
value of a journey step is never a PASS oracle.

`binding` contains `scope: LOCAL_DISPOSABLE_TWO_AM`, `database_disposable: true`,
`coordination_enabled: true`, `production_acceptance: false`,
`production_writes: false`, `kind_context`, `namespace`, `api_source`,
`api_image`, `am_origins: {a,b}`, and `web_origin`. All three origins are distinct
where required, loopback HTTP origins without credentials, path, query or fragment.
`release` supplies `source`, `image`, `kind_context`, `namespace`,
`artifact_sha256`, and `web: {source, manifest, build_info_sha256}`. Sources are
full 40-hex commits and images use full SHA-256 digests. The parent must verify
its release artifact bytes before passing these independently frozen values.

The entry point checks these inputs against `environment.source/image` and an
actual `adapter.topology()` observation **before J0**. Both live AM members must
have the exact image/source, Ready state, distinct Pod UID/process ID and the
bound loopback origins. Topology must identify the same kind context/namespace,
shared database and coordination. Existing C275 adapters must add the source
and kind-context fields from their live Pod/build/config captures; inventing
those fields from the expected values is forbidden. The admitted observation
is hashed into case evidence. Calling `journey.run(ctx)` directly, or setting
`ctx.runState` flags, cannot bypass parent admission. Invalid binding tests
exercise the real runner and verify zero registration, browser and CMA calls.

The suite starts no processes, fixture, second topology, rollout or HPA change.
The parent retains restart/HPA authority and may invoke the journey before and
after its own restart, using a separately fresh registration each time.

## Identity provenance and browser authentication

See [RECEIPT-CONTRACT.md](RECEIPT-CONTRACT.md) for the exact evidence interfaces.
The registration adapter performs real signup and challenge-scoped cleanup.
It does not certify its own success. A separate approved observer supplies a
signed, hash-bound capture of that signup, a subsequent independent authenticated
NyxID user read, and the browser callback/cookie binding. The parent pins the
observer's public key and nonproduction issuer independently of the registrar.
The suite verifies the signature, invocation nonce, creation time, request IDs,
subject and registration joins, then reads the immutable provider-response
artifacts and independently checks their hashes and mapped subject/creation
fields. Flags such as `real_registration`, `fresh`, `verified`, or `session.bound`
are never evidence. No NyxID signature format or registration endpoint is invented:
the signature is the independent **capture process's** signature.

J0 opens the actual browser session and makes a cookie-authenticated
`/auth/session` request, checks the returned subject against the verified signup,
and hashes the actual scoped HttpOnly cookie jar. It also hashes the served
`/build-info.json` against the frozen web artifact. J2 and J7 repeat those checks;
their redacted browser evidence includes subject, session-binding and signup
receipt digests. Bearer-only evidence, another subject, another cookie jar,
or another compiled web build blocks the journey. Session rotation requires a
reviewed successor capture contract; this version refuses an unexplained change.

## Ordered actions and required assertions

| Step | Action | Evidence required |
| --- | --- | --- |
| J0 | Real signup/authentication; empty Agent inventory | Signed independent provider capture, verified raw artifact hashes, actual matching cookie session, zero existing Agents |
| J1 | First public Agent create; lifecycle; Ready | One intent/key, lifecycle receipt, actual Sandbox owner/lineage, exact release, create request joined to the lifecycle worker's owner/epoch |
| J2 | Open the Agent through API and compiled web | Agent/transcript identity, enabled Message composer, session/build proof, actual serving/owner evidence |
| J3 | Send `Reply only OK.`; then send and interrupt a separate held turn | Exact new completed turn with assistant reply `OK`; actual provider stream and active transcript before interruption; exact second turn becomes interrupted before hold release |
| J4 | Changed settings and revisioned rename | Changed values, confirmed publication, every mutation request joined to actual non-owner ingress and owner epoch |
| J5 | Stop; then pause and resume Sandbox | Exact terminal `stopped` before pause; exact `paused`/`active`; each terminal read from the other AM relative to the actual mutation receipt; lifecycle owner proofs |
| J6 | Delete the stopped Agent | One delete intent; exact terminal absence or fully removed tombstone; retained request/owner evidence |
| J7 | Personal CMA usage and real `Your usage` view | Same registration/cookie/build proof and the actual usage UI; CMA allocation presentation only, never an external invoice/payment claim |

Every Agent mutation must have its own matching request ID, Agent digest, actual
ingress, owner instance and positive epoch. J3/J4 use the runner's live lease
proofs with automatic non-owner routing. J1 and terminal lifecycle transitions
require independent signed worker/effect captures from the parent: creation has
no prior Agent lease, and stop/delete may release one. A later GET is not a
mutation proof. The suite blocks if these captures are unavailable; it does not
fabricate a still-live lease, replay a mutation or exempt a step from attribution.
The lifecycle adapter must represent actual Sandbox transition ownership where
applicable; whether those records expose the required join is a runtime binding
question for the parent and SA-04, not an assumed implementation fact.

## Exact NyxID limitations and current blockers

Local kind cannot supply the following through `acceptance/identity.py`, seeded
owner tokens, or a fixture that returns a successful verdict:

1. **J0 real account registration:** an approved nonproduction NyxID signup
   endpoint/flow, new provider subject and creation record, independent user read,
   immutable capture storage and a separate trusted capture key are required.
2. **J0 real authentication/callback:** email, OTP, invitation, social login or
   external callback required by that NyxID deployment must actually complete.
   The parent must install the new subject's API authorization and real browser
   cookie session; substituting existing owner authorization is forbidden.
3. **J1–J7 reuse that same identity/session.** They require no further signup.
   They remain blocked until J0's real registration/session can be supplied.
4. **J7 external billing:** invoices, payments and subscriptions require a
   separate real NyxID billing integration/manual verification. CMA's allocation
   basis and its `not_integrated` billing status cannot establish those claims.

No approved nonproduction NyxID registration/capture service or lifecycle
capture binding has been supplied to this branch, and no live journey has been
launched. The current J7 assertions cover the usage summary and rendered view;
complete-report/API-to-visible-report matching remains follow-up journey work
under the lead's instruction to resolve this contract review first. These limits
remain explicit BLOCKED/NOT RUN in acceptance reporting.

## Cleanup and unknown outcomes

Signup cleanup is registered before signup begins and uses the challenge to
reconcile an unknown outcome. Agent cleanup is registered before create. Unknown
create outcomes require parent reconciliation; mutations are not replayed with
new keys. Cleanup waits for exact stopped state, reconciles uncertain deletion
without another delete, and retires this run's Sandbox after Agent deletion.
Identity cleanup deletes only this run's registration after resources are released;
otherwise it revokes the session, retains the account for reconciliation, and
reports cleanup failure. The parent must confirm final scoped workspace/database
cleanup before accepting a campaign. No production writes are permitted.

## Offline checks

```sh
node --test .github/scripts/multi-am-features/journeys/j0-j7/*.test.mjs \
  .github/scripts/multi-am-features/executable/runner.test.mjs \
  .github/scripts/multi-am-features/routing.test.mjs
```

49 checks pass in this revision: 41 journey contract/behavior checks and 8 shared
runner checks. They include an actual C275-runner J0 unit invocation, invalid
binding zero-call tests, forged/changed/stale capture refusal, provider artifact
hash verification, cookie/subject/build mismatch, complete mutation joins,
terminal stop sequencing and the two separate turn outcomes. Synthetic unit
success is never reported as local-kind, real-NyxID or full J0–J7 acceptance.

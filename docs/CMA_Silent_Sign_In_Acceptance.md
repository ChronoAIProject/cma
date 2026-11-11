# Silent sign-in acceptance

This work builds on PR #1387 to implement the browser identity mechanism for TC-002. Local doubles and
public-contract replays prove mechanics; they do not claim real NyxID, first
answer, owner isolation or cross-browser go-live qualification. The deployment
operator and QA owner execute this runbook against the candidate environment.
No production access or deployment is performed by the implementation task.

## Evidence envelope

Record candidate API/web/runtime digests, source SHA, environment, start/end SGT
time, browser/version, sample count, `CHRONO_SILENT_SIGN_IN_ENABLED`, relevant
identity configuration and raw result locations. Count first runs and failures
separately from retries. Do not record credentials, OAuth codes, cookies, PKCE
values or private prompt contents in screenshots, traces or logs.

## Activation and rollback

Use the [API rollout procedure](API_Overview.md#silent-sign-in-rollout). Deploy
all compatible API replicas with the flag false before enabling it in a separate
rollout. Pin a compatible API rollback image; disabling the hint retains protocol
compatibility and the logout fence. No step deletes or restarts user Sandboxes.

## TC-002 and failure matrix

Run each ordinary-home entry state ten consecutive times: new NyxID registration,
returning NyxID session without a CMA session, and returning valid CMA session.
For each, use the public UI to explicitly create one Agent and submit one prompt;
require a nonempty first answer. Reopen the original Agent and verify its ID and
history. Count zero incorrect signed-out landings, duplicate creations/inputs or
cross-owner resources. No manual database authorization is permitted.

Also exercise:

- No NyxID session: one silent refusal, ordinary signed-out page, working explicit
  sign-in/registration, no redirect loop on reload or SPA navigation.
- NyxID consent required: one fresh consent exchange, cancellation returns to the
  same page, acceptance authenticates, no resource creation before explicit action.
- Home, deep Agent link, `/sandboxes/{id}?tab=access`, and `/bots/new?setup=…`:
  original address/state restored; draft remains unsubmitted. A skipped page
  followed by internal navigation gets one eligible check. Focus, visibility and
  reconnection never navigate, including after an identity outage.
- Draft already present for owner A: expire CMA identity and recover as A; preserve
  its prompt and original creation/command identity. Recover as B instead; A's
  private draft must not appear. Request A's resources as B and require refusal.
- NyxID unavailable during discovery, code exchange and identity verification:
  no new session or authority, safe return for a bound automatic attempt. A
  session-inspection outage follows the existing bounded policy; API mutations
  still require live authorization. Restore the provider and verify recovery.
- Direct `/auth/logout`, Sign out, and logout from another tab during blocked
  token exchange: no callback can create usable session authority after logout.
  Replay the callback while the original is blocked, then log out; the replay
  must not remove the original's logout handle. Repeat across API replicas.
- Consent escalation followed by logout; delayed callback response after logout:
  keep suppression when the delayed attempt started before logout. Complete a
  newly started explicit sign-in and verify automatic sign-in is re-enabled.
  Open a fresh anonymous tab while suppressed; require a false hint and no redirect.
  Reorder two logout responses around an explicit sign-in and simulate start-replica
  clock skew; the durable preference proof must keep the later logout effective.
- Cross-site logout: revoke the session without setting a persistent opt-out.
  Same-origin/none Fetch Metadata and permitted Origin establish the preference.
- Plant a known pre-auth binding in the test browser, then sign in: the resulting
  session token must differ and the planted value must never authenticate. Exercise
  logout before/during/after transaction commit on independent API replicas, with
  binding and session cookies separately. Confirm active rows remain predecessor-readable.
- Roll an old-start/new-callback and new-start/old-callback pair with activation
  off. Both complete; old callback reservations expire through their original TTL.
- Slow consent: extend pending custody with the continuation; a completed valid
  continuation must not fail because the original reservation expired.
- At 4,095 / 4,096 / 4,097 bytes of the encoded complete login URL, verify boundaries
  on direct API and nginx fallback routes. Large percent-encoded Bot Father drafts
  skip silent entry and retain URL/storage; explicit sign-in uses a path-only target.
  Raw non-ASCII return targets are refused. The web Sign out action clears task and
  creation drafts before initiating navigation.
- Expired state, missing/wrong binding, repeated callback, revoked NyxID identity,
  foreign Origin/Fetch Metadata and open-redirect candidates: zero authentication
  or cross-owner access. Confirm identity/flow responses are no-store (signup-options alone retains its public cache) and every cookie
  is host-only, HttpOnly, SameSite=Lax and Secure on HTTPS.
- Activation false/true/false across complete fleet rollouts: no new automatic
  attempt while disabled, in-flight compatible callbacks still terminate, and
  existing user work continues uninterrupted.

Attach raw evidence to the coordinating review and the owning go-live gates
(#1536, #1542, #1549, #1556). Their cross-day identities, seven-day NyxID matrix,
first-run streaks and candidate-digest requirements remain owned by those gates.

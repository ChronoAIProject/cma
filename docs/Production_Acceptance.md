# Dedicated production acceptance

Part of #1549. The Python standard-library runner in
`.github/scripts/acceptance/` uses two independently pinned ordinary test
accounts. Production writes require owner authorization **and reviewer GO for
the exact source and approval manifest**. Provisioning is complete and is not
part of this suite. No admin token, environment credential or NyxID CLI profile
is read. No Trigger, role, shared setting, payment instrument or connection may
be changed by this runner.

## Private storage and evidence

Raw subjects, credentials, cookies, fixture bodies, resource identifiers and
original operation keys belong only in `~/.config/cma-accept/`. The directory
is current-user-owned 0700; files are regular, current-user-owned 0600, without
symlinks and outside every repository. `accounts.json` and `probe-state.json`
are operational secret state, never evidence attachments. The notes account
manifest contains subject fingerprints only.

`approved.json` is the reviewed, hash-only binding to both account subjects and
emails, the CMA OAuth client, the nightly Trigger and its immutable revision.
Every phase reads NyxID `/users/me` and CMA `/auth/session` for **both** bearers
before any mutation. It requires the exact subject fingerprints, explicit
ordinary NyxID role/privilege evidence and exact ordinary CMA capabilities.
Missing fields, different accounts or extra authority refuse the phase.
Admin-created ordinary accounts do not prove public-signup/default-role quotas.

Evidence destinations are preflighted and reserved before constructing the
probe or making a request: a new regular file in an owned 0700 directory, no
symlinked parent, no repository, no overwrite. Publication is atomic and 0600.
Evidence contains fixed cases/reasons, status, counts, hashes and an explicit
allowlist of protocol problem codes. Unknown response codes become
`unrecognized`; raw bodies, response text and headers are never emitted.

## Review and execution

The current security hold remains in force. After the lead relays reviewer GO,
a private `review-go.json` receipt must contain `verdict: GO`, `source_sha256`
from `review_gate.source_fingerprint()` and `manifest_sha256` from the canonical
approval manifest. Only that exact reviewed source can run non-baseline phases.
The runner never creates or refreshes this approval. Any source change requires
another review receipt.

Both current test sessions were revoked at the end of the pre-review window.
Before another run, ordinary sessions must be prepared through supported NyxID
sign-in and placed directly in the checked private store. There is no suite
login/provisioning shortcut. Never put tokens in shell arguments or diagnostics.
Every invocation, including cleanup, requires fresh approved control bearers
for both actors. Old target tokens remain in the private session journal for
reconciliation; an expired/revoked control bearer never disables the gate.

Run offline tests first. Use a fresh evidence destination for each invocation:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s .github/scripts/acceptance -p 'test_*.py'
PYTHONDONTWRITEBYTECODE=1 python3 .github/scripts/acceptance/runner.py \
  baseline --output /absolute/private-evidence/baseline.json
```

| Phase | Behavior |
| --- | --- |
| `baseline` | Identity/capability checks, admin denial, inventory, invalid/anonymous authentication; GET only |
| `documents` | Retained owned documents; allowed/foreign read, write and list; grant/revoke/stale replay |
| `updates` | One retained owner edit per account and stale-revision denial |
| `usage` | One bounded retained report per account, foreign concealment, expiration handling |
| `domains` | Discovery, REST and exact CORS on current and retained origins |
| `consent` | Supported ordinary CMA sign-in with only the approved scopes and client |
| `journey` | Pinned nonstarting draft, policy-checked explicit Start, bounded resource probes and teardown |
| `resources` | Owned/foreign Sandbox, Agent and history-export controls when fixtures exist |
| `finish`, `sessions`, `cleanup` | Resume resource/grant cleanup, verify report expiration, then revoke tracked sessions and CMA consent/cookies |
| `all` | Re-bind at each subphase; run #1472 identity/document/revoked-session/resource probes first, then consent/journey, usage and domains; always run cleanup |

The consent scope is exactly `openid profile email roles proxy offline_access`.
Only the CMA client receives consent, from its approved account's session,
through NyxID's returned signed request. Connected-service and node access are
never expanded. Callback cookies are journaled as soon as received. A wholly
lost callback reply stays unresolved: consent can be revoked, but an unknown
cookie cannot be claimed replay-tested or deleted. The runner refuses another
sign-in and requires supported reconciliation; it does not enumerate sessions,
guess cookies, or claim successful cleanup. This limitation is explicit in the
cleanup result. Ordinary received cookies have resumable exact-cookie logout.

## Durable bounds and recovery

A process lock covers preflight, operations and journals. Sequential requests
are at least 1.05 seconds apart, with ten-second socket timeouts, 64 KiB request
and 1 MiB response limits. The normal budget is 350 requests; cleanup may use
an additional 150. Normal probes stop from 01:30 SGT to reserve cleanup time.
**Every** request, including cleanup, refuses 02:00–02:59 SGT. No HTTP redirect
is followed automatically. OAuth validates each explicit destination.

Original bodies and keys are saved before dispatch. Lost replies and transient
server failures retain uncertainty and reuse the same key. Completed writes
are clearly marked retained receipts. Pending resource cleanup runs on errors
and interruption; `cleanup` resumes it independently. A submitted session
logout marker is uncertain: a live exact token is logged out again; provider
401 establishes absence. Only journaled test bearers are revoked through
self-only logout. There is no account-wide session inventory or deletion.
If resource cleanup still needs credentials, sessions stay journaled and the
result stays BLOCKED. Passive report expiration does not require a live session
once its cleanup observation has been recorded.

Four private journals survive campaign replacement. Missing, uncertain or
mismatched history refuses writes; the runner cannot initialize blank history:

- `launch-ledger.json`: exact actor/Trigger/day/key/intent, response and cleanup
  status. At most **five attempts per SGT day per account**, counting refusals
  and uncertainty. Pending attempts block a new key even across date rollover.
  Original-key recovery precedes inventory checks. Uncertain receipts older
  than 23 hours refuse replay because server operation retention is 24 hours.
- `retained-fixtures.json`: hashes only, capped at **one document, one source
  edit and one report per account**. Approved documents are visibly labeled
  acceptance fixtures where supported, remain owned by the test accounts,
  and have sharing revoked. There is no supported document deletion API.
  Reports record server logical expiry; later reads verify expiration without
  admitting replacements. Physical Mongo TTL is asynchronous, not certified
  by a public 404. Unknown retained-write replies also have a 23-hour fence.
- `grant-cleanup.json`: obligation persisted before grants and stale replays;
  cleanup rechecks reader denial, reads the owner's current revision if needed,
  and revokes with the original bounded cleanup operation.
- `session-journal.json`: exact token fingerprints with raw tokens private,
  callback obligations, consent cleanup and confirmed revocation state.

The launch unit is at most **one live Sandbox per account**, with sequential
teardown before the other visitor proceeds. The reviewed Trigger is
`nightly-smoke-readonly`; its public ID is sourced from
`staging-e2e-readonly` / `CMA_E2E_PUBLIC_ID`, stored privately, and pinned by
hash. The public descriptor and immutable revision bind its policy. No
unreviewed Trigger revision may start. New requests create drafts with
`start_if_ready: false`; the returned revision is checked before Start.

A lost accepted reply is replayed by its original key before checking inventory,
then its exact owner-bound resources are cancelled/deleted. Terminal Launch
state and empty inventory are checked. Before any new attempt, server Launch
history must agree with the independent ledger and active inventory must be
empty. `--new-journey` explicitly chooses another generation only after those
checks; it never resets daily accounting. Existing missing connections and
fixed reuse/no-input policies remain BLOCKED and are not modified.

The November 11 migration preserved four historical attempts (two per account),
two cancelled drafts, zero allocated Sandboxes, two retained documents, two
edits and two reports. Known tracked sessions were already revoked. Migration
was offline from private receipts and historical evidence; it is not a new
production verification or reviewer approval.

## Evidence interpretation

Pre-review acceptance artifacts remain HELD. They were sanitized in place and
must not be handed off for issue closure. New acceptance evidence requires GO,
a declared deployed candidate, complete cleanup state and the issue's actual
acceptance criteria. Report a demonstrated defect before fixing it. Correct
harness expectation mistakes against deployed source and retain their history.

The suite does not prove every route, organizations/delegations, the provider-I/O
revocation race, full receipt/byte capacity, overload behavior, controller/network/
volume isolation, browser continuity or shared spend policies. It cannot
qualify fixes absent from the deployed source or replace matched telemetry
windows for contention and failure pacing. A pass count alone closes no issue.

## Sprint priority and domain PR decision

Friday 2026-11-13 15:00 SGT applies to this owner's Project 9 sprint items:
#1472 and PR #1291. The full runner puts #1472's authentication, documents,
revocation and available history/resource probes first. Previously revoked
tracked sessions can be replayed without signing out the control sessions;
final current-session teardown stays last. Capacity, overload and post-I/O
race evidence still require their own candidate-bound contracts; the safe
smoke suite does not simulate an abuse flood or consume 1,024 receipts.

#1291 is already closed as superseded. #1292 merged its routing/origin
implementation; #1610 merged the current-develop regressions and production
verification handoff. Reopening the obsolete branch is unnecessary. Current
and retained domain/origin production qualification remains a separate evidence
requirement and is not inferred from that source disposition.

# #1472 closure update — 2026-11-11

This supplements the [initial gap plan](https://github.com/ChronoAIProject/cma/blob/tl/cma-abuse/integration/docs/1472_Gap_Plan.md).
The deadline remains Friday 2026-11-13 15:00 SGT; unknown/blocked cells remain open.

## Source delivery and freeze

* Global allocation admission: #1672, exact head
  `57795e92162c7151009f76dc30556c0ef53fe7ea`, review-ready. Enforcement remains
  default-off; physical warm/prepared/system/N+1 reservations still need the
  capacity owner's ceiling and proof. Source tests do not qualify physical load.
* W46 P0 input classification: #1676, exact head
  `9909c5a4ba925fe92b25990c32faf82857966094`, review-ready, Closes #1673.
  Exact registered input submissions use the existing Input budget only when
  enabled; binding provisioning retains Create. Sixteen focused tests and four
  real Mongo contracts pass. No live rate-policy change has been made.
* Document quotas: #1661 in implementation. The target is 100 documents and 1 GiB
  retained source bytes per owner, plus bounded reservation count. Reserve before
  upload, retain ambiguous effects, import complete storage/orphan inventory and
  refuse unknown baselines. See [quota migration](CMA_Document_Quotas.md).
* Lead's develop merge freeze applies until the exact “MERGE FREEZE LIFTED” relay.
  Review and own-branch work continue. Earlier MERGE authorizations are queued;
  neither elapsed time nor green CI lifts the freeze. Train W needs a fully green
  exact-head run. Exact-head security verdict plus lead MERGE remains required.

## Latest acceptance evidence

The reviewed harness at `d7cc301436c5de2ececbca7d4a35052d4ec51d07` is not a new
application candidate. The current release remains source
`304b44fd9b9d75d282399ccd6ea34e035eb632ca`, API gen178 / Web gen209 / Gateway gen71.
The full suite completed 17:16:54–17:21:52 SGT with 210 bounded requests:
111/135 distinct passes, eight failures and 16 blocked. Two passes are explicitly
historical retained-source receipts; no new source edits were performed.
#1472 has 42/50 distinct passes (including those two historical receipts), eight
blocked, and no new failure. Final cleanup passed all 12 controls. No Sandbox or
model work was started. Permanent document/source/report slots remain consumed.

The initial document-owner 503 at 16:44:15.911 SGT took 2,502 ms and matches the
authentication-unavailable response plus verification retry timeouts. It is not
established as a document store failure. Dependency/transport versus executor
delay remains unproved. The shares 429 is consistent with the two-response
expensive-read budget; exact limiter attribution remains unproved. Both positive
controls passed in the subsequent full run without erasing the first observations.
Fairness and actual outage/load qualification are still required.

Seven retained-host 301 failures belong to the Cloudflare edge owner under #1567.
Image Manager discovery remains a known retirement issue under #1494; do not start
that service to satisfy an obsolete healthy-response expectation. These findings
do not justify a CMA quota increase or authorization bypass.

Safe identity fallback is specifically GET `/auth/session`, with a non-sliding
verified-token window of at most 60 seconds. Document GET still requires live
authentication and fails closed during verification failure. The actual-outage
campaign must test that exact scope, expiry and denied/mutation behavior; it must
not assume cached document access is the intended policy.

## Remaining code/evidence boundaries

The global Sandbox allocation ceiling does not supply public-stream admission or
global decoded ingress accounting. Current stream routes retain per-stream/page
bounds; the audit has not found the required shared decoded-byte permit spanning
public observers and hosted ingress. This is an additional CMA code/evidence cell
under #1472, overlapping #1487's runtime/observer mapping. cma-abuse owns the CMA
implementation audit and any unowned application slice; runtime/capacity owners
must supply the measured mapping and physical reserve. Do not claim the adapter's
64-stream/32-request semaphores prove a global 128 MiB decoded-memory bound.

Campaign tiers remain: Tier 1 API-only work is authorized after the plan, using
the reviewed harness and no new Sandbox/model turns. Tier 2 first supplies minimal
Sandbox/turn/duration counts and a private estimate; no figures from that estimate
belong on GitHub. Tier 3 large load/long soak remains proposal-only until an exact
go. No duplicate production run starts while security owns an active window.
The full suite's completion is evidence, not a new coordinated campaign slot.

Still required: separately activated full-receipt revoke, final-candidate ordered
races and genuine credential-negative vectors, abuse/load/fairness, actual isolated
identity outage, cold/warm/resumed runtime/network probes, compatible quota/admission
activation and teardown receipts. Preserve one retained document/source edit per
account, one live Sandbox/account, five Launch attempts/account/SGT day and the
02:00–02:59 SGT quiet hour. Existing controls already passing do not justify reruns
without new acceptance value. #1472 remains Project 9 In Progress.

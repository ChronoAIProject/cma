# #1472 gap plan — Friday closure

Prepared 2026-11-11 16:45 SGT; refreshed 2026-11-12 for current source delivery. Deadline remains **Fri 2026-11-13 15:00 SGT**.
Issue: https://github.com/ChronoAIProject/cma/issues/1472
Baseline: `origin/develop` at `542485b8b` when work began. Work only in the assigned
`tl/cma-abuse/integration` worktree; the main checkout and security branch are untouched.

## Delivery judgment and evidence rules

The four original application fixes, global Sandbox admission and document
quota implementation are merged. Full closure still needs separate revocation
activation, quota/admission baseline and activation, complete ingress ownership
code, candidate-bound acceptance, a real isolated dependency failure and
runtime/network qualification. Code delivery is within this workstream; external qualification
requires named owners and relayed execution slots. No Friday closure is claimed
until every row below passes. The updated security plan preserves full Friday delivery and explicitly
supersedes its earlier partial-delivery forecast. This plan makes the missing
closure steps and external inputs explicit.
It does not import another gate's seven-day/24-hour calendar as a new #1472 DoD.

Read: the closure queue, sprint W46 Friday plan, security Friday plan and retained
fixture contract under the lead's notes directory; harness runner, safety and
review gate at `4714d16ff231f051766405d7377effc1991880ee`. The lead's current
instruction says the harness is reviewed and running; its earlier planning text
saying review is pending is historical. This workstream neither edits that branch
nor executes the harness. New phases still require their own exact-source review.

Evidence must name source SHA, actual component digests/generations, effective
flags/configuration, environment, UTC/SGT window, per-case sample count, first
failure, private raw-result reference and teardown receipt. Aggregate GitHub text
contains no identities, resource IDs, credentials, private payloads or session links.
Local contracts and production observations are separate evidence classes.

The security execution update at issue comment 6791429823 reports denied
invalid/anonymous and observed foreign-document requests, with no access restored
by stale grant replay. Owner-read 503/429 responses stopped bounded attempts;
revision and broader acceptance remain uncertified. These first failures must
be explained before repeating the affected phase; retries are not fresh proof.
The harness cleanup correction is tracked separately in #1662.

## Acceptance matrix

| Acceptance item | Done, with evidence | Missing / gap class | Exact next work and owner | Train or window |
| --- | --- | --- | --- | --- |
| Full-receipt owner revocation | #1598 merged `e5c03a15849a84433292956d92dc9758c6394524`; compatible reader, default-off reserve, grant/link exact retries and independent count/metadata tests. `document_artifacts/{revocation_tests,rollout_tests}.rs` and Mongo rollout contracts. | **Activation + evidence**, not another reserve implementation. Need 1,024-receipt direct and link revokes, ordinary-write refusal, stale receipt denial and compatible rollback. | cma-web-hotfix verifies reader OFF, pins rollback with cma-migration, then separately enables `CHRONO_DOCUMENT_REVOCATION_RESERVE_ENABLED`. cma-security owns reviewed fixture preparation and execution. cma-abuse maps candidate tests and records receipts. | W compatible reader first; Thu evening separate ON, after Thu 14:00 cutoff. Saturation tentative Thu 18:15–00:30; no start after 19:00. |
| History post-I/O fence and projection | #1606 merged `89b04a71fe3bd23b29dc9f9c7286e9942b4912f9`, deployed in Train E; HTTP test `thousand_revoke_export_interleavings_release_no_native_bytes` covers 1,000 ordered races. Owner-only raw export and fresh authority after I/O. | **Evidence**: final-candidate 1,000-race receipt, nonempty live owner control, shared/foreign/stale denial, zero protected bytes. Sequential smoke cannot prove an in-flight race. | cma-abuse supplies exact-source test mapping; cma-security runs only relayed harness; runtime/provider owner supplies legitimate ready history fixture and any approved real I/O barrier. | Candidate contracts Thu daytime; live controls Fri 09:00–11:30 after final deployment. |
| At least 1,000 identity negatives | #1637 merged `47b78758b23c3872d69a53b64911e00d7b94292a`, application isolation proof; harness has low-count invalid/stale controls. | **Harness code + evidence**, not 1,000 random strings renamed as scope/audience cases. Required classes: expired/revoked, wrong owner, insufficient scope and wrong audience. | cma-security adds reviewed default-disabled batching through lead relay. NyxID owner supplies supported credential forms and validity receipts. cma-abuse prepares matrix and expected typed outcomes. | Four 250-negative batches + controls, Thu/Fri nonoverlapping owner-approved slot. |
| Bounded invalid-credential fanout | #1608 merged `21811fc8b207d2e16cc0d7304eb8dde042de3f03`, Train E. Existing 64-slot, 2.5-second verification budget; deterministic 1,000-distinct-credential saturation, no waiting queue, permit recovery. | **Evidence**: real inflight/queue, healthy-user latency/errors and first-run bounded load on the exact candidate. | cma-abuse designs bounded profile; NyxID, monitoring and release owners supply headroom and metrics; cma-security owns reviewed execution extension. | Thu daytime profile review; isolated authorized load Fri morning, separate from outage/rollout. |
| Large GET, metadata and ingress memory bounds | #1612 merged `b6bcab742f89c98ec9b7b026d7ff1162e87e3c9d`, Train E; returned-frame, live-fragment, relay and structural/shared-effect slices landed in #1715 with 25/25 exact develop CI. Permission #1709 merged `61a0c9f0cd65bedcc8cdf36a807bb6dbe64ebcc7`; raw #1717 merged `ce72ee311c703da38653aa227bb81d0d3c1e7ae5` with 25/25 PR CI. | **Code + activation + evidence**: #1720 decoded ownership merged at `3aee1ff68730d53c5a4cfa91fea5c1cf463fa64a`; #1723 coordinator, #1725 embedded-error parser, #1728 sign-in ownership and #1730 typed diagnostics are review-ready; #1731 context-goal frame ownership passes 661 focused tests and local gates; review and merge are pending. Exact heads and qualification are recorded in the linked PRs and aggregate issue comments. Typed domain/prepared, codec/fragment, desktop, durable assembly and independent working storage remain open. Selected requested allocations do not prove total ingress or RSS. | cma-abuse completes the remaining #1702 boundaries and measured contracts. The response projection owner retains read working-buffer scope; runtime/capacity and monitoring owners supply useful-capacity mapping and final-candidate workload/RSS receipts. cma-security owns the relayed bounded live profile. | Keep raw/JSON controls OFF on Thursday 13:00 train. Remaining source Friday 06:00; candidate-bound qualification before Friday evidence freeze. |
| Global admission across owners/replicas | #1672 merged `6bf560d737ecf468e654fcaab55aec939a4642d8`: durable default-off Mongo admission before allocation, cross-owner concurrency, replay and failure contracts. Existing visitor limits remain independent. | **Activation + evidence**: physical capacity mapping, conservative admitted-state baseline, replica/rolling overlap and useful capacity are not certified by source tests. | GKE/controller owner supplies physical pool/prepared reserve and approved cap; release owner enables only after inventory/convergence. cma-security runs the reviewed bounded refusal/recovery profile. cma-abuse fixes any demonstrated application defect. | Source merged; retain OFF until owner baseline. Candidate-bound capacity/refusal receipt before Fri 12:00. |
| Document count and retained source quota | #1677 delivered through #1715 merge `1d425cffc6e48bf33cc26635b6e1a58f92aec79f`; default-off persistent count/source reservations, write fencing, retry and uncertain-effect handling. Batch exact develop CI 43080307598 passed 25/25. | **Baseline + activation + evidence**: complete source/orphan inventory and compatible writer convergence are required. Source delivery does not establish live quota enforcement. | Document/store owner supplies an accounted baseline and orphan reconciliation; cma-migration/release owners execute the fenced install and ready readback. cma-security runs supported boundary/replay controls through the reviewed harness. | Source merged for Thursday train, OFF. Separate baseline/activation window required before Friday evidence freeze. |
| Safe identity-outage policy | Existing process-local non-sliding <=60-second verified-token fallback, GET `/auth/session`-only restriction, invalidation/logout fences; mutations/creation fail closed. `auth/identity_fallback_tests.rs` and bearer/cookie tests cover mechanism. | **Evidence**, plus fix only if a concrete failure is found: actual isolated provider outage, session-inspection cache expiry, document GET fail-closed behavior, denied-context controls, typed write refusal and recovery. | NyxID owner supplies account-scoped real verification failure <=75 seconds and removal <=10 seconds. cma-security runs reviewed <=30-request phase after relay; cma-abuse records policy/results. No shared endpoint/DNS fault. | Thu or Fri owner-booked slot, separate from load and rollout, before Fri 11:30. |
| Remote runtime/network isolation, cold/warm/resumed | #1637 supplies application isolation contracts. Deployment manifests and runtime authority exist; application denial is not network isolation. | **External runtime/controller implementation if probe finds a gap + evidence**. No demonstrated 2-direction x 3-state network/file/metadata/K8s/credential matrix. | GKE/network and sandbox-controller owners supply exact deployed policy/runtime receipts and execute scoped probes; runtime/image owner supplies positive dependency controls and lifecycle/volume cleanup. cma-abuse owns CMA-side fixes revealed by those results. No speculative shared-network policy change. | Owner nomination/window by Thu 09:00; cold/warm/resumed campaign Thu afternoon or Fri 09:00–11:30 after runtime HOLD resolved. |

Train E evidence: issue #1472 comment 6782263576 and closure sweep 6788334403;
source `304b44fd9b9d75d282399ccd6ea34e035eb632ca`, fully green push CI
42886990359, API generations 177/178, Web 209. These receipts establish deployed
slices, not hostile/load/outage qualification of the future final candidate.

## Original core PR sequence and gates (source delivered)

1. #1660, `tl/cma-abuse/global-admission` -> develop: global allocation ceiling with
   default-off config, infra parity, typed capacity refusal, success/replay and
   cross-owner failure/race coverage. Include this durable planning document.
2. #1661, `tl/cma-abuse/document-quotas` -> develop: owner document/source reservations,
   strict default-off config, infra parity, legacy-baseline refusal and storage
   timeout/ambiguous-effect/concurrency coverage. Split further only if the
   count and byte boundaries can remain independently sound and reviewable.
3. Concrete defects found by the required contracts get their own issue-linked
   `tl/cma-abuse/<topic>` PR before the next cutoff. Revoke activation uses the
   already-merged switch; no duplicate flag or fake activation PR is needed.

Each slice has an issue before edits, assigned to the configured GitHub account
with a security topic label. Bodies say `Closes #<slice>` and `Part of #1472`;
never auto-close the umbrella on a narrow slice. Every changed/new source and test
file stays below 500 lines. Gates: cargo fmt, clippy, targeted allowed/denied tests,
real disposable-Mongo contracts for shared atomicity, file-size check and gitleaks.
Web gates apply only if web changes. Start heavy work only with >=32 decimal GB free, pause below 30 GB and retry
after ten minutes. Use an existing shared heavy lock with `lockf -k`, keep one
target directory and at most two Cargo jobs, and remove only owned temporary artifacts.
Fetch and merge origin/develop before push; never rebase or force-push. No push
to an open PR without lead instruction. Exact-head reviewer-sec verdict plus lead
MERGE relay permits admin merge when no job has failed. No protection changes.
Production trains still require the release owner's qualified artifact receipt.
The Thursday performance candidate freezes at 08:30 SGT for the 13:00 train;
unselected resource policies stay OFF. Remaining aggregate source targets Friday
06:00. An explicit freeze lift is required before any queued merge.

## Campaign tiers and execution authority

Owner decision relayed 2026-11-11: spend only on work that adds acceptance value.

* **Tier 1 — authorized after this plan:** API-only authorization races/negatives,
  revoke, invalid credentials, large GET/metadata bounds and admission refusals.
  No new Sandboxes and no model turns. Run through the exact-reviewed harness,
  preserving its limits and owner-controlled activation prerequisites. Existing
  authority is sufficient; do not request another blanket execution approval.
  A new harness phase still needs its scoped source review via cma-security.
* **Tier 2 — estimate first:** any Sandbox start/resume/cold/warm lifecycle or
  model turn, including related load. Use the smallest sample that proves the
  criterion and retain visitor limits. Before execution, give the lead the
  Sandbox count, model-turn count, duration and a rough estimate in private local
  notes only. No cost figures or identities enter GitHub. The estimate is not
  permission to exceed fixture or visitor limits.
* **Tier 3 — propose only:** large load or a long soak. No execution unless the
  lead relays an explicit go for the exact bounded profile. Prior Tier 1 authority
  does not authorize a saturation burst that risks shared service capacity.

The inherited security plan's blanket execution hold is superseded for Tier 1
only by this later owner decision. Production configuration/fault injection stays
with its named owner; new fixture types and harness edits remain outside this
workstream. Do not rerun unchanged passing samples without new acceptance value.

Common: exactly two dedicated accounts; only the reviewed harness; one live
Sandbox and five Launch attempts/account/SGT day; retained one-document plus
one-source-edit/account contract; no replacements or ledger reset. No new
Trigger, elevated role or unrelated resource. Normal phase <=350 requests and
150 cleanup reserve, one in flight, >=1.05 seconds spacing, 10-second timeout,
64 KiB request/1 MiB response caps unless a separately reviewed extension has
explicit owner authorization. Stop new effects by 01:30; zero traffic 02:00–02:59.

* **Receipt saturation:** two existing labelled documents, one direct grant and
  one link context with no overlapping direct permission. Trusted count/byte
  oracle before filling; <=2,048 preparation writes cumulative, <=8/minute,
  <=300 writes/chunk plus <=40 controls, original keys retained. Require flag ON
  first. At exactly 1,024 receipts revoke, retry, stale replay, normal-write denial
  and owner success. Count and byte exhaustion remain separate proof cells.
  Abort on count uncertainty/overshoot, early exhaustion, unexpected 429, health
  regression or cleanup uncertainty. Remove grants/links/comments; retain only
  the two authorized documents and their private hash ledger. Never trim receipts.
* **Race/negative:** isolated final-source 1,000 ordered export races; four live
  250-negative batches, each <=25 owner controls and <=25 preflight checks,
  <=1,200 normal requests total, <=0.5 negatives/sec. Report unique credentials,
  vectors and repetitions separately. Abort on any private success/byte, failed
  owner control, unexpected 429/503, fixture drift or candidate mismatch. No
  retries-as-new-samples. Teardown exact tracked sessions/grants; keep journals.
* **Abuse/load:** first isolated 64-held + 1,000-refused verification contract and
  expensive-read cancel/size contracts. Live preliminary sample <=100 invalid
  plus 100 valid small GETs, <=230 normal requests, sequential. Higher load is a
  separate owner-authorized harness extension with agreed concurrency/rate/duration
  and live telemetry; no implied permission to fill shared 64 slots. Target
  inflight<=64/no queue/p99<=3s, healthy errors<0.1%, zero quota overshoot. Abort on
  any control error, readiness/restart/OOM change, owner-set headroom breach, or
  latency>2x baseline for two one-minute windows. Cancel/drop test reads, verify
  permit recovery and normal controls, then watch 30 stable minutes. Missing
  telemetry prevents start, rather than becoming a zero-impact claim.
* **Actual outage:** fault selector proven to match only test credentials; <=75s
  outage, <=10s remove mechanism, <=30 requests total and <=1/sec. Sample GET `/auth/session`
  near 0/30/59/61/75 seconds from last real verification; mutations fail
  closed, revoked/unknown/expired/scope-mismatched credentials stay denied.
  Stop on one unauthorized result, sliding cache, unrelated impact or fault
  removal delay. NyxID removes fault first; then reconcile original pending keys,
  verify recovery and restore tracked signed-out state. No shared process restart.
* **Runtime isolation:** exact two test-owned runtimes, both directions, cold,
  warm-adopted and resumed; five forbidden families gives >=30 negative cells.
  Probe only approved peer listener/files/volume, metadata, K8s/control API and
  credential paths; stop before consuming sensitive content. Require successful
  allowed DNS/model/identity/storage controls. A TCP connection plus HTTP401 is
  not network denial. Owner executes supported retirement and verifies claims,
  Pods and volumes; public404 alone is not teardown. Abort on any cross-boundary
  success, runtime/policy drift, unknown home ownership or health regression.

## Concrete asks to the lead / outside owners

| Needed by (SGT) | Ask / executor | Why it is required |
| --- | --- | --- |
| Thu 09:00 | Relay cma-web-hotfix + cma-migration's W reader convergence and compatible rollback digest; reserve Thu evening separate flag activation and 6-hour preparation/cleanup window. | Reserve writes cannot precede a verified compatible rollback floor; saturation cannot fit the smoke budget. |
| Thu 09:00 | Relay cma-security extension request for receipt-counted link/grant saturation, four negative batches and the <=30-request outage phase; reviewer-sec exact-source review. | 4714d16ff has no such campaign phases. This workstream may not edit that branch or bypass its guards. |
| Thu 09:00 | Name NyxID executor and provide private supported negative-credential receipts, isolated actual outage mechanism and removal SLA/window. | Malformed tokens and mocks cannot prove scope/audience or actual-outage acceptance. |
| Thu 09:00 | Name GKE/controller/runtime executor, resolve the existing legitimate Trigger prerequisites and runtime HOLD, provide conservative total allocation ceiling plus warm/prepared/N+1 headroom. | CMA cannot set a safe live global limit or manufacture cold/warm/resumed fixtures from per-owner quotas. |
| Thu 12:00 | Document/store owner supplies read-only legacy count/source/orphan baseline and exact two-fixture receipt/byte oracle; approve quota activation values. | Quotas must cover retained/uncertain source storage; revision alone is not receipt count. |
| Thu 12:00 | Release/monitoring + NyxID owners approve load profile and telemetry/headroom/abort thresholds, plus a nonoverlapping Fri morning slot. | Test accounts still consume shared API/provider/database resources. Production-impacting execution needs the owner's go relayed by the lead. |
| Before each merge/run | Relay reviewer exact-head verdict + MERGE; provide Tier 2 estimates before execution and exact Tier 3 go with candidate/window/bounds. | User's merge and tier policy; Tier 1 already has authorization after this plan, while Tier 3 needs explicit go. |

At Thu 12:00 and Fri 12:00 publish the unresolved closure cells and owners; no
silent scope reduction or invented owner commitment. Fri 14:00 finish cleanup
and evidence audit. Fri 15:00 report full closure only on complete receipts;
otherwise keep #1472 OPEN / Project 9 In Progress with the exact missing rows.

## Milestones and persistence

At gap-plan, PR-open, merge, deployment, verification or blocked milestone update
Project 9 Status and post one aggregate progress comment. Preserve the requested
local `notes/2026-11-11-1472-gap-plan.md`; the committed copy lives at
`docs/1472_Gap_Plan.md` because repository structure permits only docs/ for tracked
Markdown. GitHub is the collaboration source; local notes are the requested
handoff artifact, never a replacement for committed delivery.

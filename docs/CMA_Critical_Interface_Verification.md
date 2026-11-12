# Friday critical interface verification (#1574)

Audit: 2026-11-11 SGT, `develop@e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd`.
The original branch integrated `origin/develop@325c7f6a2` before push; its usage-UI
delta did not change the audited implementations or anchors. The November 13
synchronization preserves the historical audit baseline and acceptance gaps.
Part of [#1574](https://github.com/ChronoAIProject/cma/issues/1574).
The deadline is **Friday 2026-11-13 15:00 SGT**. Full qualification of all 311
entries cannot fit this slice. The deliverable is a reviewable critical ledger,
small deterministic test additions, and explicit acceptance gaps. No production
request, mutation, activation or journey execution was performed for this audit.

## Scope and counting

The [operation ledger](CMA_Critical_Interface_Operations.md) audits **57 exact
OP entries** from the original **311 entries (290 OP + 21 CONTRACT)**. It covers
the actions and proof reads for J0–J7 in #1508 TC-001/003–010, current inline
creation/discovery, and retained J6 trust compatibility. The three direct nightly
smoke tests and their indirect application reads are included below. Historical
J5 Profile publishing is excluded from current J5 acceptance; that script must
be replaced, not counted as an equivalent journey.

Nine supplementary auth/Image Manager operations are traced separately because
they are not individual OP entries in the original inventory. Three Web delivery
families are also separate: an arbitrary number of asset requests does not
increase the API-operation denominator. Optional console enrichment, the full
AG-UI/Responses/desktop protocol matrix (#1537), all legacy Profile authoring,
and the other 254 inventory entries remain outside this slice. A journey's
runtime file/home attestation is an acceptance condition, not an invented API.

| State                                                                | Original OP subset | Supplementary auth/image operations |
| -------------------------------------------------------------------- | -----------------: | ----------------------------------: |
| Implementation located                                               |                 57 |                                   9 |
| Deterministic test coverage located (boundary stated per reference)  |                 57 |                                   9 |
| Missing implementation identified within this subset                 |                  0 |                                   0 |
| Accepted in production on the audited develop candidate              |                  0 |                                   0 |
| Awaiting production acceptance                                       |                 57 |                                   9 |
| Historical route observations available, insufficient for acceptance |                  6 |                                   1 |

These rows are independent dimensions, not numbers to add together. In the
mutually exclusive acceptance classification, **66 implemented/test-covered,
awaiting acceptance; 0 VERIFIED; 0 missing handlers identified**. A test being
present is not a claim that its full suite was run here, that a deployment flag
is enabled, or that a third-party provider succeeded. The original issue's
44 partial / 9 missing / 14 disabled / 147 unverified / 97 implemented totals
are a historical baseline; this slice does not rewrite them or certify the
21 broad CONTRACT entries.

## Every operation in the GET-only nightly smoke

Authority: [workflow](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/.github/workflows/nightly-staging.yml#L61),
[three smoke tests](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/e2e/smoke.pw.ts#L7), and
[smoke preflight](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/e2e/preflight.ts#L7). Smoke preflight writes local
provenance but makes no API calls; authenticated preflight belongs to mutation
mode. The smoke reads are:

| Operation                                           | Ledger entry / implementation | What the test demonstrates                                                                                                                                                                                                                                                            |
| --------------------------------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /api/v1/conformance`                           | C01                           | Direct request asserts HTTP success and `service=cma`; it does not certify the advertised interfaces. Current response still says `rebuilding` with an empty interface list.                                                                                                          |
| `GET /api/v1/triggers/public/{public_id}`           | C02                           | Indirect public-page read, heading and enabled action; new N tests exercise the real SDK decoder.                                                                                                                                                                                     |
| `GET /api/v1/triggers/public/{public_id}/badge.svg` | C03                           | Direct request asserts 200, SVG MIME/body, no Set-Cookie. B covers fixed bytes, cache and unavailable cases.                                                                                                                                                                          |
| `GET /auth/session`                                 | A01 below                     | Indirect auth-provider read on landing, reload and anonymous console.                                                                                                                                                                                                                 |
| `GET /auth/signup-options`                          | A02 below                     | Indirect anonymous Trigger landing read. N distinguishes unavailable discovery from registration permission.                                                                                                                                                                          |
| `GET /t/{public_id}` (including reload)             | Web navigation family W01     | [Public route](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/App.tsx#L107), [SPA delivery](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/nginx.conf#L197); heading/action are browser assertions, not completed Launch evidence.                                                                                                                                |
| `GET /`                                             | W02                           | [Anonymous landing](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/App.tsx#L157); N confirms only the passive session read.                                                                                                                                                                                               |
| `GET /assets/{hashed_asset}`                        | W03                           | [nginx current/retained asset delivery](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/nginx.conf#L19), [entry imports/icons](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/index.html#L1), [fonts](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/styles/tokens.css#L25). JS/CSS/font/brand/icon dependencies are a single delivery family; image-smoke fixtures are separate from provider acceptance. |

The [browser guard](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/e2e/security.ts#L88) also permits HEAD/OPTIONS;
the suite explicitly invokes GET. `page.request`/`request` calls do not inherit
browser route interception, so their explicit GET methods matter. The local
[guard mechanics test](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/e2e/mechanics.check.ts#L8) proves browser POST/DELETE
refusal; it is not a server-side guarantee of side-effect-free arbitrary GETs.
No login/callback/open-entry URL is visited by the passive smoke.

**Fixture requirement:** `always_review` must not be false. A published
no-review Trigger calls `POST .../entry-intent` from
[PublicJourney.tsx:182](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/public/PublicJourney.tsx#L182). The guard
blocks it and the smoke's `blocked=[]` assertion fails. The new N test records
this exact attempted POST and proves its refusal never admits a Launch. Do not
allowlist that POST or count a failed smoke as a pass. Signed-in retained intent
is also outside the fresh anonymous smoke fixture.

## Supplemental operations (not additional original OP entries)

All nine are implemented and have the scoped coverage below; all lack current
candidate production acceptance. A01 has historical E1 route evidence only.

| ID  | Operation / journey                                     | Implementation                                                                                         | Deterministic test reference                                                                                                                                                                                            | Missing |
| --- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------- |
| A01 | `GET /auth/session`, smoke/J0–J7                        | [session_routes.rs:16](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/auth/session_routes.rs#L16)                               | [session permission/revocation test:45](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/auth/cookie_authorization_tests/session_routes_tests.rs#L45), adjacent failure/anonymous cases; N                                                         | R1/R8   |
| A02 | `GET /auth/signup-options`, smoke/J0                    | [signup_options.rs:70](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/auth/signup_options.rs#L70)                               | [projection/cache test:173](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/auth/signup_options.rs#L173), failure/closed fallback:240; N                                                                                                          | R1/R8   |
| A03 | `GET /auth/login`, J0/J4 registration/sign-in gesture   | [flow.rs:97](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/auth/flow.rs#L97)                                                   | [complete authorize-attempt test:25](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/auth/flow_registration_tests.rs#L25), invalid-screen refusal:154                                                                                             | R1/R4   |
| A04 | `GET /auth/callback`, J0/J4                             | [flow.rs:207](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/auth/flow.rs#L207)                                                 | [unseen identity/no-bootstrap test:239](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/auth/flow_registration_tests.rs#L239)                                                                                                                     | R1/R4   |
| A05 | `POST /auth/connections/check`, current J4/J6           | [browser.rs:341](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/connections/browser.rs#L341)                                    | [authorized check overlap:295](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/connections/browser_tests.rs#L295), omission/foreign-intent refusal:224                                                                                            | R4/R6   |
| A06 | `POST /auth/connections/start`, current J4/J6           | [browser.rs:349](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/connections/browser.rs#L349)                                    | [owned required-service test:270](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/connections/browser_tests.rs#L270); [late response fenced:251](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/connections/ConnectionWizard.test.tsx#L251)                                           | R4/R6   |
| A07 | `POST /auth/connections/cancel`, current J4/J6          | [browser.rs:357](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/connections/browser.rs#L357)                                    | [strict HTTP intent shape:6](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/connections/client.test.ts#L6); [modal cancellation:107](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/connections/ConnectionWizard.test.tsx#L107); this is consumer coverage, not a full HTTP owner-denial matrix | R4/R6   |
| A08 | `GET /api/image-manager/v1/images?scope=all`, J5/J6     | [http.rs:175](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma-image-manager/src/http.rs#L175), [Web proxy:96](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/nginx.conf#L96) | [all pages/scope/cancellation:11](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/create/imageCatalog.test.ts#L11), partial/cursor failures:56                                                                                                               | R5/R6   |
| A09 | `GET /api/image-manager/v1/images/{id}/versions`, J5/J6 | [http.rs:348](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma-image-manager/src/http.rs#L348)                                        | [successful immutable version across pages:27](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/create/imageCatalog.test.ts#L27); [selection failure:90](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/trigger-publisher/InlineTargetStep.test.tsx#L90)                                          | R5/R6   |

The current [ReviewCard](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/public/ReviewCard.tsx#L22) uses
ConnectionWizard/A05–A07. C14–C15 remain the public/retained service-handoff
contract. Neither proves actual OAuth, API-key or device-code provider acceptance.

## J0–J7 assertion ledger

Each journey has implemented building blocks and deterministic coverage, but
**all eight acceptance journeys remain unverified**. The operations table alone
must not turn independent handler passes into a successful end-to-end journey.

| Journey                                                  | Implementation and deterministic evidence                                                                                                                                                                                                                                                                                                                                                            | Missing production proof / automation                                                                                                                                                                                                                                                                                   |
| -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| J0 new registration to one complete answer               | A01–A04, C02/C04–C13/C30/C34; [flow_registration_tests.rs:239](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/auth/flow_registration_tests.rs#L239) proves callback does not bootstrap; [PublicJourney.test.tsx:123](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/public/PublicJourney.test.tsx#L123) proves one explicit intent; [worker guardrail:246](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/trigger/worker_tests/guardrail_tests.rs#L246) proves first message supplied once | R1: new identities/default grants, unchanged attribution, one nonempty completed answer, no extra start/input; 30 across two days.                                                                                                                                                                                      |
| J1 always-new ownership                                  | C06–C13; [launch_resolution_service.rs:55](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/layer2-core/src/trigger_domain/launch_resolution_service.rs#L55), [policy/context matrix:299](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/layer2-core/src/trigger_domain/launch_service_tests.rs#L299), [foreign-owner launch read:347](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/layer1-public/src/rest/trigger_routes_tests/launch_routes.rs#L347)                                                  | R2: two real visitors, separate owned resources and complete first replies, ten repeats; current browser J1 uses one visitor and is insufficient alone.                                                                                                                                                                 |
| J2 reused Sandbox, distinct Workspace                    | Same resolution matrix; C34/C38/C44–C46 read hierarchy/history                                                                                                                                                                                                                                                                                                                                       | R2: ten repeats plus old/new file hashes and transcript preservation. Browser IDs alone do not prove filesystem isolation.                                                                                                                                                                                              |
| J3 continue/fresh/resume and uncertain response          | [repeat branches:736](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/layer2-core/src/trigger_domain/launch_service_tests.rs#L736), [PublicJourney retry/choice tests](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/public/PublicJourney.test.tsx#L187), [admission coalescing:59](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/public/intent.test.ts#L59)                                                                                                                                                 | R3: every branch ten times, retained/new IDs and same-key lost-response replay with no duplicate Agent.                                                                                                                                                                                                                 |
| J4 connection-required recovery                          | A05–A07 and C10–C15; [callback inert until same-owner readiness:171](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/connections/ConnectionWizard.test.tsx#L171), popup-blocked:153; [pending relogin masks old connection:173](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/connections/browser_tests.rs#L173)                                                                                                                                                  | R4: real OAuth/API-key/device-code valid/expired/cancelled/lost-callback matrix, ten each and seven-day record; cannot fit entirely before Friday.                                                                                                                                                                      |
| J5 image/inline publication and embeds                   | C16–C27/A08–A09; TC/TU prove inline image authority; [InlineTargetStep tests](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/trigger-publisher/InlineTargetStep.test.tsx#L39), EM snippet goldens                                                                                                                                                                                                                                        | R5: replace [legacy publisher script](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/e2e/publisher.pw.ts#L24) with current image/inline two-owner journey; real allowed-origin click-through, immutable version, unique counts and safe public fields, ten runs. Legacy Profile fork is not current J5.                                                         |
| J6 two inline Agents, stop/delete, pause/revive/rotation | C28–C46/C49–C57/A05–A09; AC/AL/AS/AD/HP/RU; [lifecycle failure tests](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/create/lifecycleFailure.test.tsx#L1), [transport handoff tests](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/src/create/lifecycleTransport.test.tsx#L1); HC is retained trust compatibility only                                                                                                                                                          | R6: replace [legacy lifecycle entry](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/web/e2e/lifecycle.pw.ts#L5); prove same named Workspace disambiguation, distinct homes, dirty files/history/IDs through real rotation; ten runs and bounded terminal outcomes. Runtime port mocks do not prove home preservation.                                               |
| J7 retired admission and paused orchestration            | C02/C06/C08/C09/C21/C47/C48; [availability refusal:373](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/layer2-core/src/trigger_domain/launch_service_tests.rs#L373), [worker paused boundary:501](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/cma/src/trigger/worker_tests/recovery_tests.rs#L501), N retired intent                                                                                                                                                | R7: approved fault slot, stable event head across three reads, then exactly one completion after resume; ten runs. Already admitted drafts retain frozen revision authority ([test:1115](https://github.com/ChronoAIProject/cma/blob/e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd/api/crates/layer2-core/src/trigger_domain/launch_service_tests.rs#L1115)); retirement is not cancellation of admitted work. |

## November 13 scheduled-smoke supplement

This preserves the November 11 audit and its source-bound evidence. Existing
source/test links are pinned to `e9f30f595944c8ffe33b7a4a6c8208f3aad7d6fd`, so
later refactors cannot silently change what the historical line references mean.
The added passive HTTP test remains linked to this PR's source.

GitHub Actions metadata read on 2026-11-13 confirms two later **scheduled,
first-attempt** Nightly Staging successes:

| Run | Workflow source | GET-only smoke window (SGT) | Attempt/result |
| --- | --- | --- | --- |
| [42884394431](https://github.com/ChronoAIProject/cma/actions/runs/42884394431) | `15deaed86e6da9a07c6b85e725387b7eb9eb9139` | 2026-11-11 03:22:44–03:22:55 | 1 / success |
| [43086439261](https://github.com/ChronoAIProject/cma/actions/runs/43086439261) | `1d425cffc6e48bf33cc26635b6e1a58f92aec79f` | 2026-11-12 03:23:29–03:23:42 | 1 / success |

Both runs completed the Read-only staging smoke job and its GET-only smoke step.
Reviewed staging authority and Explicitly scoped staging journeys were skipped.
These results supersede E3's historical lack of a successful scheduled smoke.
Workflow source is not proof of the deployed API/Web digest, successful mutation
journeys, provider acceptance or certification of all 66 audited operations.
R0–R7 remain open; R8 still needs reconciliation with the accepted candidate and
its deployed identities. No production request was made for this supplement.
#1826 tracks this scoped source/test delivery; #1574 retains the parent gate.

## Evidence index and explicit missing list

Existing private evidence is referenced by path and aggregate result only. No
customer identifier, hash, credential, request body or message content is copied.
The original audit read these reports locally. The November 13 refresh verifies
GitHub run metadata and pinned source links; it does not recapture production or
independently reverify the private E1/E2 reports.

- **E1 — historical route activity, not journey acceptance:**
  `tech-lead/notes/2026-11-10-api-deploy-4/deploy-20261110-192430/report.md`,
  lines 1–35 and 149–165. The candidate `4bc09db3f` was rolled back; the original
  `4f03c35a8` was restored at 22:12 SGT. The candidate's 46-minute sample observed
  Agent/lifecycle/hierarchy reads and session reads. The 33 lifecycle, 75 Agent
  list, 190 Agent detail and 34 retained Sandbox revision responses had no 5xx;
  786 Sandbox detail responses included two 5xx and 629 Workspace detail reads
  included one. These do not prove the response payload, caller authority,
  downstream work or current develop. A historical status count is not a pass.
- **E2 — Web delivery only:**
  `tech-lead/notes/2026-11-10-cmaeg-web-release/execution-20261110T032338Z/report.md`,
  line 203; Web generation 208, source `4f03c35a8`, VERIFIED 18:34:42 SGT.
  Actual-origin asset/build/cache/MIME checks and a fixture-backed channel-settings
  browser render passed. This is historical W03 delivery evidence, not J0–J7,
  real public Trigger content, or acceptance of this develop candidate.
- **E3 — historical nightly sample (superseded for scheduled-run status):**
  `tech-lead/notes/reviews/2026-11-10-pr1592-evidence/nightly-failure-class.txt`,
  run `42833983221`: required base URL absent. The original audit's five-run
  sample contained one failure and four cancellations
  (`42682567041`, `42514606639`, `42405769735`, `42278877818`). No successful nightly production
  smoke artifact was established by this audit. Preflight explicitly records
  `provenanceVerified=false`; a future configured-origin run still needs its
  candidate/API/Web identities reconciled before being called production proof.

| Gap | Explicit missing work                                                                                                                                                             | Owner routing / safe next step                                                                                                                     |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| R0  | Current-candidate evidence for **every C01–C57 and A01–A09**; no accepted run covers the set.                                                                                     | #1574 audit owner maintains ledger; release executor supplies train identities, #1508/#1536 journey owner supplies approved runs.                  |
| R1  | J0 genuine new-account/default-grant and unique complete-answer acceptance; 30 samples/two days.                                                                                  | #1508/#1536 runner owner, identity prerequisites via lead. Do not substitute existing sessions.                                                    |
| R2  | J1 second visitor/ownership and J2 preserved files/history, ten per path.                                                                                                         | #1508/#1536 runner owner; bounded fixture ownership and runtime attestation.                                                                       |
| R3  | J3 real continue/fresh/resume plus lost-response repeat with original key, ten per branch.                                                                                        | #1508 journey owner; authorized fault fixture.                                                                                                     |
| R4  | J4 real provider outcomes and seven-day matrix; current consumer cancel coverage is not full HTTP owner-denial coverage.                                                          | #1508 connection/journey owners. Seven days cannot be completed by Friday from this audit start; keep gate open and retain first verified samples. |
| R5  | Current image/inline J5 script, two independent publishers, real embed/origin/attribution/stats acceptance.                                                                       | #1508/#1536 Web journey owner; retire legacy script from current acceptance totals.                                                                |
| R6  | Current inline J6 script and runtime home/file/history continuity with two Agents; all state transitions and rotation.                                                            | #1508 journey owner with release executor's approved observation slot. Preserve legacy trust as a separately labelled compatibility branch.        |
| R7  | J7 approved pause/resume fault execution and retired/new-admission proof, ten repetitions.                                                                                        | #1508 journey owner and release executor; no operator write from this audit.                                                                       |
| R8  | Reconcile the successful scheduled GET-only smoke with review-enabled fixture and exact deployed candidate identities; workflow metadata alone does not certify the deployed candidate. | Nightly-smoke owner / lead-provided nonsecret target. No credentials/hooks, no POST exemption.                                                     |

The references are routing proposals to the already owned gates, not new
assignments or authorization to contact customer/provider owners. The remaining
254 inventory entries and all cross-protocol, performance, streaming-load,
multi-replica, live provider and seven-day soak claims remain explicitly unaudited
or unaccepted here. Production approval and a green release train remain required.

## Added deterministic coverage and local validation

[PassiveHttpContract.test.tsx](../web/src/public/PassiveHttpContract.test.tsx)
adds eight cases through the actual App, NyxID adapter and TypeScript SDK. Only
HTTP is replaced with fixed in-memory responses; unexpected requests are
asserted even if the consumer catches their errors. It covers anonymous and
signed-in reviewed landing remount/focus, passive home, retired saved intent,
malformed preview, signup/identity failures, and blocked no-review entry probing.
It does not supply provider tokens, use a network, or exercise a production page.

Focused Vitest, Web typecheck, ESLint and formatting results are recorded in
the PR body and private audit report. Backend implementation is unchanged;
Rust references above document existing coverage for CI, not a fresh local
Rust execution. This PR must use **Part of #1574**, not close the gate.

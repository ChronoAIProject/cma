# CMA native Sandbox production verification — 2026-10-14

CMA Agent Manager, Event Gateway and Web are deployed, and the final cold
Gateway user flow passed. Results below identify their tested version. Evidence
paths are operator-local artifacts, not repository files.

## Final deployed set

At **11:31:31 SGT**, Agent Manager, Web and Event Gateway each had one updated
Ready replica, current observed generation, the exact published imageID and
zero restarts. CMA runs in northeast `chrono-bot`; native Sandboxes run in
southeast `example-registry-01`. One Web image implements all three portals. Image Manager
activation remains deferred.

| Component | Candidate | Registry manifest digest |
| --- | --- | --- |
| Agent Manager | `cd37968d` | `sha256:b641e4e1795acac4e0586ac4db01ba43d55b3e2c7d3d1bea5b0e54b2b5f99eef` |
| Default runtime | `37d6f95c` | `sha256:1c44be8aaa7b88c0190d17ca44f4f6e0f9a8aec880038c5d4da9cb2bc8c68e8b` |
| Web | `ac837aa7` | `sha256:477215d347f052c3315a8dafc3be31e89a6c4516772f65bc21e8aee83b884a0f` |
| Event Gateway | `cabfe5c3` | `sha256:0c3068130eaf618ce88dbe47bd7d111d365a5bba816c49f78b4000e8550ef1e7` |

Application images use
`us-central1-docker.pkg.dev/example-project-01/example-registry-02/` repositories
`cma-api`, `cma-web` and `cma-cmaeg`. Runtime uses
`asia-southeast1-docker.pkg.dev/example-project-02/example-registry-01/cma-runtime`.
The following are actual CI image source revisions, not inferred PR merge SHAs.

| Component | Actual image source revision | Source tree | CI |
| --- | --- | --- | --- |
| AM | `b8f6ae10d048cd95593ef9efb88ff88195d82216` | `73a43035bda9d9f07e38ac378ab9eb44dfc2e17f` | 38502541842 attempt 1; full 15/15 passed |
| Runtime | `1d2473a3e7f8e5961f75b63725ebc0742f973fe7` | `b84818d8dbbe22ee84ebd575aec0cb94c650e7fa` | 38482643832; full 15/15 passed |
| Web | `d498b1b5e5541485654b30d9b5c97f80ae3e0962` | `f99e9ae871af0b291d657c71f1ddc8db637d759d` | 38504776234 attempt 1; four required jobs passed |
| Gateway | `21f3401204becb767e462bf5fb4258d996dc923d` | `aae602e75dc85ff5f8cb7aabb14c8504aef619ea` | 38496119196; full 15/15 passed |

AM publication required Backend, Images(api), Repository structure, Secret scanning
and Manifests. Its full run was in progress at publication, then completed 15/15
at the 11:32:56 SGT observation. Web required Images(web), Frontend, Repository
structure and Secret scanning; its full run was in progress at publication.
Scoped publication and full-run observations are separate evidence. At the
11:47:00 SGT check, Web had 13 successful jobs of 14 created, with Backend still
running; no full Web CI success is claimed. Current snapshot:
`/tmp/cma-scope-text-01/full-ci-current-observation.json`.

| Release evidence | Receipt |
| --- | --- |
| AM exact publication and CI observations | `/tmp/cma-native-conflict-01/publication-report.json`; `full-ci-at-publication.json`; `full-ci-final.json` in that directory |
| Web exact publication and publication-time CI | `/tmp/cma-scope-text-01/publication-report.json`; `full-ci-at-publication.json` in that directory |
| Gateway exact publication | `/tmp/cma-test-timeout-01/publication-report.json` |
| Runtime publication, residue scan and canary | `/tmp/cma-release-final-37d6f95c-01/publication-report.json`; `/tmp/cma-release-final-37d6f95c-01/runtime-residue-scan.json`; `/tmp/cma-runtime-final-37d6f95c-01/result.json` |
| Final complete rollout/configuration | `/tmp/cma-rollout-native-conflict-01/plan.json`; `/tmp/cma-rollout-native-conflict-01/readiness.json` |
| Public and owner readback | `/tmp/cma-public-endpoints-final.json` |
| Synchronized repository manifests | `/tmp/cma-final-repo-sync-01/resolved-receipts.json` |

AM generation 13 uses Pod UID `b65b0274-3d86-40e3-8c0c-5e7fba98a910`.
Web generation 89 retained UID `1ea4ec73-3628-4dd7-800a-6ed6eb9dc430`;
Gateway generation 6 retained UID `2de69466-5794-4ea5-9ff0-a5ba584ffddc`.
Deployment UIDs are unchanged. AM remains singleton/Recreate with native
agent-sandbox enabled and authenticated `kubernetes-exec` transport. Runtime 37d
and Gateway origin `http://example-host-01.example-namespace.svc` are preserved. No infra-owned
controller, CRD, RBAC, storage or network policy was changed.

At **11:31:59 SGT**, all seven public/owner checks returned HTTP 200: Web `/`, AM
`/ready`, Gateway `/readyz`, authenticated non-visitor owner session, valid owner
inventory, and the original protected Sandbox/Agent, both still paused.

## Measured acceptance

| Flow and tested version | Actual result | Evidence |
| --- | --- | --- |
| Three creation contexts — AM/runtime 37d | New Sandbox, existing Sandbox and existing Workspace each preserved a 79-byte UTF-8 title, empty workspace, `gpt-6/high/autonomous`, mandatory scopes and target identity. Each completed a real first turn; identical operation replay returned 202 with the same operation. Native imageID matched runtime 37d. | `/tmp/cma-final-owner-new-37d6f95c.json`; `/tmp/cma-final-owner-existing-sandbox-37d6f95c.json`; `/tmp/cma-final-owner-existing-workspace-37d6f95c.json` |
| Persistent home/history — AM/runtime 37d | Native canary passed assignment/replay and same-PVC suspend/resume. API exports were identical UTF-8 JSONL, 43,428 bytes and 20 records, across five requests. Retained Mongo identities/history prefixes and protected original history survived rollout. | `/tmp/cma-final-history-export-37d-zo5ye6_a/report.json`; `/tmp/cma-final-rollout-after-37d6f95c-zht4aiai/reviewed.json`; `/tmp/cma-secondary-model-resume.json` |
| Owner Details/settings/model/history — Web c63, AM 37d | Details exposed only model, reasoning, permission mode and key scope, with both mandatory scopes fixed. Three changes/restores returned 200, runtime acknowledgment, reload persistence and stored-event replay. Two actual turns used native-confirmed gpt-5.6-sol and gpt-6. Two menu downloads matched independent HTTP export: 98,511 bytes, 46 records. | `/tmp/cma-final-agent-ui-proof-FigMWZ/acceptance-reconciled.json`; `/tmp/cma-final-agent-ui-proof-FigMWZ/native-model-proof.json` |
| Final service-picker appearance — Web ac837 | At 11:20:13 SGT, 1442 and 390 px checks each measured 105 service rows/431 elements with zero overflow. Screenshots passed review. Viewport restored; Agent/Sandbox stayed paused at revision 85; zero revives, settings changes, prompts or unexpected mutations. | `/tmp/cma-final-details-style-proof-kHhQRd/report.json` |
| Owner-approved MCP continuation across AM replacement — AM 37d/Web 5ce, retained runtime 028aca | One owner approval returned 200; exact connections were ready; credential generation advanced 2 to 3. Original conversation automatically completed one acknowledgment at turn 56, with no new prompt and preserved prefix/reload. | `/tmp/cma-final-warm-approval-proof-xwfmHa/report.json`; `/tmp/cma-final-warm-approval-resume.json` |
| Earlier portal/runtime flows — versions in individual receipts | Browser creation and optional repository first prompt passed. Pause/resume retained home/conversation identities. Warm-pool adoption used its existing native resources; resize to zero preserved the claimed Agent/PVC, rejected stale revision with 409 and removed unclaimed resources. | `/tmp/cma-real-portal-owner-proof-j9MtV6/report.json`; `/tmp/cma-optional-repo-f034/status-redacted.json`; `/tmp/cma-native-live-accept-f034/`; `/tmp/cma-native-warm-live-8ur5fbbq/report.json` |
| Hosted callback — already-connected service | Registered return URL and official hosted completion passed; temporary binding removed and all 47 original bindings preserved. This was not a fresh missing-service OAuth journey. | `/tmp/cma-fixed-connection-callback-client-receipt.json`; `/tmp/cma-real-hosted-callback-proof-xG2iZH/report.json` |
| Earlier Gateway routing — versions in individual receipts | Independent OIDC login, target-preserving edit, selected-Agent management test and custom-event ingress completed actual model responses. Original Telegram channel preserved. An earlier explicit Busy response admitted zero turns; a caller retry completed one acknowledgment. | `/tmp/cma-real-gateway-ingress-Vk2ucc/report.json`; `/tmp/cma-gateway-inline-live-Gzcyq5/report.json` |
| Final cold inline-Agent Gateway — AM cd379/Web ac837/Gateway cabfe | Passed at 11:43:56 SGT: zero pre-test Agents; one actual UI click/POST returned HTTP 200 with the exact acknowledgment visible after 42.624 seconds. One ready Agent, one completed turn, one user/assistant pair and 19 contiguous events; reported acceptance latency 97 ms | `/tmp/cma-gateway-cold-live-czmxw4jd/acceptance-receipt.json` |

The c63 functional receipt reconciles an initial empty CDP body capture using
actual downloads and independent HTTP, without repeating settings or prompts.
It proves successful stored-event replay, not a fresh refused-settings flow.
The earlier caller retry and ready-Agent Gateway checks do not substitute for
the final cold management-test result.

## Cleanup and protected resources

| Fixture | Normal cleanup result | Receipt |
| --- | --- | --- |
| Retained warm permission fixture | DELETE 202; revocation event 71; exact Claim/Sandbox/Pod/PVC absent at 09:24:45 SGT. Disabled application pool target/ready stayed zero; application/native and infra python-a pool UIDs preserved. | `/tmp/cma-warm-retained-cleanup-report.json` |
| Failed 5ce cold fixture | Original DELETE 202; Sandbox/Agent retired; exact four native resources/channel absent and dedicated key revoked by 09:32:19 SGT. Lease/home races converged without extra mutations or forced deletes. | `/tmp/cma-gateway-cold-live-lm2syu_a/owner-retire-receipt.json` |
| AM 37d provisioning-failure fixture | Normal channel DELETE 204 retired the Sandbox; channel/key and four exact original native resources absent by 10:36:34 SGT. | `/tmp/cma-gateway-cold-live-14esj40l/failed-provision-native-exact-absence.json` |
| Primary paused acceptance fixture — AM 37d | DELETE and same-key replay returned 202; retired revision 98; credential event 78 completed. Original Claim/Sandbox/PVC absent at 11:21:16 SGT; Pod already absent while paused. One transient stop_agents failure converged normally; no forced delete. | `/tmp/cma-final-primary-paused-cleanup.json` |
| Final cold fixture | Passed by 11:45:14 SGT: channel DELETE 204 then 404; Agent deleted at 11:44:29 and Sandbox retired at 11:45:06; all four original native resource UIDs absent, dedicated key revoked/404, disposable row absent after reload | `/tmp/cma-gateway-cold-live-czmxw4jd/acceptance-receipt.json` |

The protected original Sandbox and Agent are distinct from these owned fixtures;
the final seven-check public readback confirmed both remained paused.

## Corrections and relevant historical failures

| Earlier evidence | Failure and final correction | Original receipt |
| --- | --- | --- |
| Runtime 2bc6 | Build-test installation residue caused a Sandbox identity mismatch. Final 37d isolates build state, honors persistent TMPDIR and passed a zero-residue scan across 15 logical layers plus native canary/application flows. | `/tmp/cma-release-final-37d6f95c-01/runtime-residue-scan.json`; original diagnosis retained in [deployment handoff](CMA_Deployment_Readiness.md) |
| Settings event compatibility before 5ce | Missing TypeScript update_settings parsing hid Agent pages/approval. Regressions reproduced it; final source includes the parser and c63 live stored-success replay passed. | `/tmp/cma-paused-page-error-evidence.json`; `/tmp/cma-settings-event-compat-receipt.json` |
| 5ce cold management test | HTTP 503 provider_unavailable, zero admitted input/turns. Gateway cabfe separates 90-second startup from a fresh 30-second acceptance/completion budget; Web/default SDK callers allow 125 seconds. Only explicit Busy retries the same body/key; ingress remains 660 seconds. | `/tmp/cma-gateway-cold-live-lm2syu_a/report.json` |
| AM 37d native provisioning | Kubernetes 409 on the first home-authority patch caused terminal validation; zero Workspace/Agent creations and zero test POSTs. Final AM retries only exact 409 Conflict after rereading/revalidating original identities; AlreadyExists/unknown 409/ownership/UID/spec refusals remain terminal. | `/tmp/cma-gateway-cold-live-14esj40l/provision-failure-receipt.json`; `/tmp/cma-native-home-conflict-receipt.json` |
| Web e189/af1 picker visuals | e189 loaded styles but overflowed the grid. af1 fixed the grid yet a real 278 px unbreakable label still overflowed a 257 px field. Final ac837 scopes min-width:0/overflow-wrap:anywhere and passed actual desktop/mobile production checks. | `/tmp/cma-final-details-style-proof-FI8Ptt/report.json`; `/tmp/cma-final-details-style-proof-5qXLnB/report.json` |

Focused verification also passed the 48 KiB history regression, 28 native
transport checks and strict Clippy; four pinned ACP suites covered seven
settings/model/reload/credential cases. Those ACP suites used an isolated Pod
based on the prior runtime, separately from final-image acceptance. Final AM
conflict tests covered fresh versions, identical UIDs, no duplicate allocation/
PVC writes and replacement-UID refusal. Final Web local checks reproduced the
actual catalogue failure and passed 429 elements at 1442/1280/390/320 px plus
extra unbroken text. Receipts: `/tmp/cma-full-pinned-acp-receipt.json`,
`/tmp/cma-native-home-conflict-receipt.json`,
`/tmp/cma-details-scope-text-receipt.json`. Component CI results are above.

## Coverage limits

Image Manager build/catalog activation, fresh missing-service OAuth, NyxID
frontend automatic navigation/binding-key delivery, external reply sinks,
broad process-crash/failover, full Console/media/STT, Trigger delivery and
cross-user sharing are not established here. Existing Agent UI uses AG-UI; no
production UI transport-flag switch is claimed. Runtime canary results use
operator identity and local status; real application flows are separately
identified. Exec connectivity does not prove direct Pod-network isolation.
A default runtime update does not migrate retained Sandboxes; the MCP approval
flow intentionally used retained runtime 028aca.

The four repository image/config manifests are synchronized to final receipts;
YAML, configuration key/documentation, native contract and route ownership
checks passed. Operational instructions and infra ownership boundaries are in
[CMA deployment readiness](CMA_Deployment_Readiness.md).

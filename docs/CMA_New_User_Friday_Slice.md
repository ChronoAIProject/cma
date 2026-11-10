# J0–J2 authenticated Friday slice

Implementation work: #1508. Gate: #1536 (M-FUNC-01). This entrypoint selects
three existing real-dependency journeys. It is partial evidence, not the full
new-user campaign, a fixture replacement, or a GO decision.

The lead assigned this bounded slice for Friday **2026-11-13 15:00 SGT**.
The gate's November 29 acceptance and November 30 decision remain separate.
The older #1508 campaign dates (November 14/21/27) are not silently rewritten:
product must confirm the wider campaign schedule, cohorts and execution owner.
The sprint owner permits production as staging; cma-web-hotfix owns production
execution. The coding agent never borrows the owner's browser or credentials.

## Run the existing account harness

From `web/`, after `npm ci` and the existing SDK dependency setup:

```sh
npx --no-install playwright test --config e2e/j0-j2.config.ts --list
npx --no-install playwright test --config e2e/j0-j2.config.ts
```

Listing discovers exactly J0, J1 and J2 and executes no journey. The execution
command is for the authorized release/test operator after inputs are provisioned.
It fails before scenario execution unless `CMA_E2E_MODE=mutation`, the exact
HTTPS `CMA_E2E_BASE_URL` matches `CMA_E2E_MUTATION_TARGET`, and all existing harness
prerequisites pass. No fallback account, silent skip, retries or repeated samples
are allowed. The full runner and scheduled read-only smoke are unchanged.

Required existing inputs:

- `CMA_E2E_PUBLIC_ID`, `CMA_E2E_FIXTURE_OWNER`, `CMA_E2E_FIXTURE_FILE` and a fresh
  external `CMA_E2E_OUTPUT_DIR`, plus the exact target/mutation mode above.
- The complete existing manifest from `e2e/environment.ts`: two dedicated test
  owner identities, a different real visitor identity for each registered case,
  known Trigger/placement fixtures, baseline services, immutable profile and
  API/web/runtime/CMA/NyxID provenance, connection/fault/embed fixtures, ordinary
  signup/connection and scoped operator hooks, and credential sentinels.
- All manifest/auth/hook/sentinel files outside the checkout with private 0600
  permissions. Test identities and resource ownership must match `/auth/session`
  in preflight; no admin account can stand in for a visitor. Do not paste values
  into GitHub, logs or command arguments. No fixture credentials ship in the repo.
- A real fresh signup path for J0: its context starts without storage state,
  and the ordinary UI hook must create a new identity, not install an existing
  cookie or manufacture grants. Baseline-only service consent, a working pinned
  managed model/runtime and a stable nonempty first message are prerequisites.
- Enough explicitly approved test capacity for two simultaneously live J1
  Sandboxes and two J2 Workspaces/Agents. Default visitor quota behavior is
  tested separately; the runner does not raise quotas or edit policies.

The slice intentionally reuses the full manifest and preflight contract. Thus
fixtures for later journeys must still be registered, even though no J3–J7 case
runs. This avoids introducing a second credential or cleanup authority model.
If the complete manifest is unavailable, record the missing inputs and do not
substitute an owner session or report local tests as authenticated acceptance.

## What the three cases establish

| Case | Required observations | Remaining full-acceptance work |
| --- | --- | --- |
| J0 / TC-001 | Anonymous landing has no commands; ordinary signup returns to the original Trigger/placement; fresh visitor identity/default capabilities; one Launch; actual Sandbox belongs to that identity; exactly one completed nonempty first answer and no extra start/input | 30 new identities across two days, independently bound candidate/dependency evidence, full new-account cleanup and cross-tenant campaign |
| J1 / TC-003 | Two launches by the isolated J1 visitor produce different Sandboxes; each actual Sandbox belongs to that visitor and each admitted Agent completes its first answer | Visitor A/B comparison, publisher/cross-visitor denial probes and ten uninterrupted first-pass samples |
| J2 / TC-004 | Same visitor reuses its Sandbox, gets different Workspaces and Agents, completes both first answers, and the original Agent transcript remains unchanged | Separate file markers/hashes and paths, ten uninterrupted first-pass samples |

These checks use the deployed public API and actual browser; they do not mock
NyxID or runtime/model responses. Local oracle/selection tests only verify test
mechanics. They must never be counted as live samples. The ordinary homepage
signup path (TC-002) is also outside this Trigger-focused slice.

The existing `cleanup.ndjson` ledger cleans only resources actually created by
the run and refuses ambiguous ownership/quiescence. Reused resources are not
owned by cleanup. Missing cleanup proof is a failed run requiring scoped operator
reconciliation. NyxID signup-account removal is the existing external operator
responsibility; no new account-administration operation is introduced.

## J3–J7 prerequisites and exclusions

| Journey | Required before execution | Required acceptance beyond this slice |
| --- | --- | --- |
| J3 / TC-005 | Isolated visitor; published ask/resume Triggers with current supported inline configuration; completed existing Agent; approved lost-response transport injection | Continue/resume retain Agent; fresh creates a distinct Agent in the intended Workspace; same-key retry creates no extra Agent; each branch ten consecutive first-pass samples |
| J4 / TC-006–007 | Dedicated unconnected visitors; real test OAuth/API-key/device-code services and ordinary connection hooks; allowed/blocked popup variants; cancelled/expired/lost-callback fixtures and credential sentinels | Original Launch/authorization receipt survives; correct owner; one start; no leaked credential or invented connection readiness; ten samples per variant and the required final seven-day matrix |
| J5 / TC-008 | Two dedicated publishers and visitor; current image plus inline Agent/Workspace authoring; pinned public test repository; approved real embed origins; publication/stats reads | Update the legacy profile-based `publisher.pw.ts` before claiming current J5; fixed image/revision, visitor ownership, independent publisher configuration, no side effects from passive navigation and no duplicate embed statistics; ten samples |
| J6 / TC-009 | Current image/inline creation, isolated workspaces and two Agents, dirty-work/file hashes; approved lifecycle/rotation hooks and before/after runtime evidence | Distinct Agent/home authority, preserved identity/history/files, bounded typed outcomes; historical profile trust is a separate compatibility branch; update legacy profile-based lifecycle paths before claiming current J6; ten samples |
| J7 / TC-010 | Isolated visitor; real retired and paused Trigger fixtures; release-owned scoped pause/release fault driver | Retired admits zero commands/resources; paused preserves original state/event head, release completes original Launch once; ten samples per variant; no coding-agent production fault injection |

Full M-FUNC-01 also needs files/documents/sharing, homepage onboarding, support
and deletion, the advertised-capability matrix, three-browser fault/retained-asset
coverage, and the required prerequisite issues. Do not advertise PTY/general MCP/
executable A2A or an unverified provider. A passed slice does not close #1508 or
#1536, satisfy the whole gate, or change its measured value.

## Evidence and handoff

The dedicated reporter writes `j0-j2-results.json`, never full-campaign
`results.json`. It requires exactly one first-pass result for J0/J1/J2; skipped,
retried, cancelled, filtered and incomplete execution cannot pass the slice.
It records `scope=j0_j2_partial`, `fullCampaignGatesAdvanced=false` and
`samplingTargetsSatisfied=false`. Case names/status/duration are curated;
raw assertion errors, DOM dumps, credentials and identity-provider URLs are not
published. Traces, HAR, video and automatic screenshots remain disabled.
Guarded screenshots and the cleanup ledger retain the existing rules.

Before/after independent deployment attestation is still required for real
acceptance. The existing manifest provenance is operator-declared and explicitly
unverified by the browser preflight. Record candidate API/web/runtime/CMAEG
digests, source/NyxID commits, environment, SGT interval, actual sample count,
configuration/switches, first-pass outcomes and private raw-evidence paths.
A changed candidate, interrupted run, failed cleanup or missing fixture is not
passing evidence. Never feed this partial report to the full-campaign audit or
scheduled-window count. The release owner reports actual results to the lead;
only the product gate owner can accept the full campaign or correct its schedule.

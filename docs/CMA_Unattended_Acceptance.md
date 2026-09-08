# Unattended Production Acceptance

Scope: the owner's 21 requirements dated 2026-09-08. Build from
`feat/trureturing-trigger-loop-20260907` and deploy directly; do not merge to
`develop` or `main`. A passing local test is not production acceptance.

Preserve existing owner identities, histories and work. Future Trigger checks
must use or amend **Truretruing Formalize and Contribute Trigger**, never create
another disposable Trigger. Never record credentials, private logs or tokens.
Monitor disk usage and inspect exact targets before removing stale artifacts.

## Acceptance Register

| # | Required behavior | Current evidence and remaining work |
|---|---|---|
| 1 | Live Sandbox creates Workspace with zero Agent Profiles | Pending public API, UI and production verification. |
| 2 | Live Sandbox creates Workspace with one Agent Profile | Pending public API, UI and production verification. |
| 3 | Live Sandbox creates Workspace with multiple Agent Profiles | Pending public API, UI and production verification. |
| 4 | Snapshots never interrupt creation, deletion or Agent interaction | Not satisfied: deployed whole-home capture holds writers. Requires a consistent point-in-time copy without a capture-duration writer lock, then concurrent production checks. |
| 5 | Storage physically retains only the latest snapshot | Completed publication prunes the previous generation. Runtime scratch archives and orphan uploads still require implementation/audit. |
| 6 | Terminated Sandbox snapshots are deleted | Pending complete storage audit, including runtime scratch and orphan objects. |
| 7 | All lifecycle screens show secret-free bounded live logs and sync time | Shared lifecycle progress exists. Audit all relevant screens and actual telemetry; verify bounded scrolling in production. |
| 8 | New Agent fields are understandable, profiles have anchored hover details, deleted choices are excluded | Deployed in web df6e58e3. Desktop/mobile production checks pass for field explanations, deleted-choice exclusion, anchored bounded scrollable pinned-profile preview, and Escape preserving the creation dialog. All 1,127 frontend tests pass. |
| 9 | No review/consent flash for no-review Triggers | Pending browser verification against the required Trigger, including fresh sign-in and resumed Launch. |
| 10 | Profiles and Marketplace use CMA-styled Ornn-inspired cards/details | Pending source comparison, implementation and responsive production verification. |
| 11 | Separate collapsible Live and Paused Sandbox groups | Deployed in web 40300b5e. Production desktop/mobile checks pass for both groups and persisted collapse on reload. Live transition cases additionally covered by local tests. |
| 12 | Only the named Trureturing Trigger is used for Trigger testing | Active constraint. Older disposable-Trigger harnesses must not be reused unchanged. |
| 13 | Persist Trigger/manual provenance and owner-defined quick messages | Pending schema, public API, configuration, UI and production verification. Required messages: Contribute to Github source; Introduce Truretruing. |
| 14 | No unavailable Sandbox/Workspace expansion controls or children | Sandbox gating retained; preparing/deleting/failed Workspaces now also hide children and expansion controls. Local tests pass; production verification pending. |
| 15 | Standard Sandbox states; deleted hidden; deletion dialog explains removal | Resource-specific labels implemented; snapshots no longer replace Live/Pausing. Explicit Needs attention remains for genuine failures. Production coverage pending. |
| 16 | IME confirmation Enter never sends; ordinary Enter sends | Deployed in web 40300b5e. Production desktop/mobile synthetic composition, keyCode 229, Shift+Enter and ordinary Enter checks pass; draft retained. Ordinary-send requests intercepted before reaching the Agent. Native OS IME selection still requires a real keyboard check, not claimed by browser-generated events. |
| 17 | Agent header and sidebar show consistent resource state | Pending shared resource-specific presentation and live production comparison. |
| 18 | Notifications animate on showing and dismissal | Pending shared notification audit, implementation and reduced-motion checks. |
| 19 | Real native terminal and chat share one conversation, all CLI features, matching theme and reliable sizing | Pending pinned-engine feasibility and implementation. An independent second conversation or imitation terminal is not acceptance. |
| 20 | gh through NyxID GitHub OAuth; identity per user; three-service Connect/Verify flow | Pending runtime/proxy implementation and connection-flow verification. Do not hardcode author identity in shared profiles. |
| 21 | Agent, Workspace and Sandbox deletion completes correctly | Earlier production deletion during another Sandbox restore passed after bounded lifecycle concurrency fix. New requirement needs full UI and same-Sandbox snapshot concurrency verification. |

## Baseline

The previously deployed API includes bounded concurrent lifecycle reconciliation;
see `CMA_Production_Verification.md` for image digests and recorded checks. That
improves unrelated deletion latency but does not remove snapshot writer fencing.
The last recorded README-trigger revival took 508,520 ms; performance work remains.

Owner histories remain in their existing Agents. Runtime changes must not silently
rotate or pause the owner's active Sandbox. Production acceptance requires actual
image identity and measured results for each behavior, with unknowns left explicit.

## Verification Log

- 2026-09-08: Resumed with branch clean at `19c9d9f`. Host disk: 57 GiB free.
  Docker reports 21.55 GB images, 57.58 GB volumes and 7.76 GB build cache.
  No cleanup or restart needed at this point.
- 2026-09-08: IME and navigation batch: 1,123/1,123 frontend tests pass;
  TypeScript, ESLint, Prettier and Vite production build pass. The optional CSS
  collision diagnostic still reports two existing cross-file landing selectors
  (`.brand-home` and `.landing__character`); neither was changed by this batch.
  No gate was disabled or allowlisted.
- 2026-09-08 14:20 UTC: Web 40300b5e deployed from this branch at
  `sha256:314b0768b9bb8a95f9d07904898d2112903c858fe8d29a24433ddbb74b4855ad`.
  Linux/amd64 build, fixed HIGH/CRITICAL vulnerability scan and remote digest
  validation pass. Rollout verified healthy with zero restarts; API and runtime
  configuration unchanged. Evidence: scratch `unattended-web-rollout-3LEuPM`.
- 2026-09-08: Production browser checks pass at 1440x900 and 390x900 for
  paused-child hiding, independent group persistence, composition Enter,
  keyCode 229, Shift+Enter, ordinary Enter request and draft preservation.
  No mutating browser request forwarded to production. Evidence:
  scratch `unattended-ui-batch1-RTtyD6/result.json` and screenshots; local
  counterpart `unattended-ui-batch1-qBa03d`. Initial browser runs exposed a
  harness readiness race with mobile navigation, corrected by awaiting the
  mounted Sandbox heading independently of drawer visibility.
- 2026-09-08 14:35 UTC: Web df6e58e3 deployed from this branch at
  `sha256:a23411a1864a1f793fcb89e9434422f27f90304ebcad2e6bbadfdc864061ef90`.
  Linux/amd64 build, vulnerability scan, remote digest validation, TypeScript,
  ESLint, Prettier, and 1,127 frontend tests pass. Rolling update verified healthy
  with zero restarts; API/runtime configuration unchanged. Rollout evidence:
  scratch `unattended-web-rollout-CypoVk`. Production creation-dialog evidence:
  `unattended-creation-ui-JwLw9p/result.json`; local counterpart
  `unattended-creation-ui-ha7Piw`. Both include 1440x900 and 390x900 screenshots,
  light/dark preview checks, and zero forwarded mutations.
- Post-rollout readback: owner Sandbox still Paused, latest snapshot epoch 27;
  owner Agent histories remain four turns and one turn respectively. SHA-256
  digests of the complete turn arrays:
  `285650b191391e7b01b2127bc94698b0a00c3239cb57596cea60757e12869c71` and
  `1e9e314f75d3045ce6b26e8d9be46b3f7ba580143c23c509afb5b553b11be303`.
  Only cma-api and cma-web pods remain in chrono-bot, both healthy.
  Disk monitoring ranged from 54 to 57 GiB free; no Docker restart or artifact
  deletion was needed. No branch was merged to develop or main.

## Next Verification Work

- Complete header/sidebar status synchronization, including newer durable
  Agent status versus periodically refreshed hierarchy. Shared vocabulary alone
  does not prove data freshness agreement. Current controller imports newer
  titles from navigation, not newer status; durable AG-UI status frames include
  `timestamp`, while ephemeral frames may omit it.
- Workspace profile validation accepts zero through eight distinct Agent Profile
  references. `agent_profile_provisioning.rs` reconciles each pinned profile only
  after Workspace Ready plus a resolved commit. Verify zero/one/multiple creation
  on a live Sandbox; do not assume validation alone proves provisioning.
- Continue the complete register above, especially consistent nonblocking
  snapshots, storage cleanup, Trigger quick messages/provenance, GitHub proxy
  authority and the genuine synchronized terminal. These remain unsatisfied.

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
| 8 | New Agent fields are understandable, profiles have anchored hover details, deleted choices are excluded | Pending implementation and production verification. |
| 9 | No review/consent flash for no-review Triggers | Pending browser verification against the required Trigger, including fresh sign-in and resumed Launch. |
| 10 | Profiles and Marketplace use CMA-styled Ornn-inspired cards/details | Pending source comparison, implementation and responsive production verification. |
| 11 | Separate collapsible Live and Paused Sandbox groups | Implemented with counts, keyboard-accessible group controls and owner-scoped persistence; local tests pass. Production verification pending. |
| 12 | Only the named Trureturing Trigger is used for Trigger testing | Active constraint. Older disposable-Trigger harnesses must not be reused unchanged. |
| 13 | Persist Trigger/manual provenance and owner-defined quick messages | Pending schema, public API, configuration, UI and production verification. Required messages: Contribute to Github source; Introduce Truretruing. |
| 14 | No unavailable Sandbox/Workspace expansion controls or children | Sandbox gating retained; preparing/deleting/failed Workspaces now also hide children and expansion controls. Local tests pass; production verification pending. |
| 15 | Standard Sandbox states; deleted hidden; deletion dialog explains removal | Resource-specific labels implemented; snapshots no longer replace Live/Pausing. Explicit Needs attention remains for genuine failures. Production coverage pending. |
| 16 | IME confirmation Enter never sends; ordinary Enter sends | Implemented; all 10 Composer tests pass, including composition lifecycle, native composing flag, keyCode 229, Shift+Enter and draft retention. Production verification pending. |
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

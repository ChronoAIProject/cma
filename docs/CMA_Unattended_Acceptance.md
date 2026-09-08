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
| 1 | Live Sandbox creates Workspace with zero Agent Profiles | Production UI and public API pass: empty-directory Workspace Ready with a resolved commit, zero Agents and zero Worktrees in 4,750 ms. Evidence: workspace-profile-counts-P4vsSj/creation-result.json. |
| 2 | Live Sandbox creates Workspace with one Agent Profile | Production mobile UI and public API pass: Workspace and one correctly pinned Agent/Worktree Ready in 24,396 ms, with no first prompt sent. Same creation evidence as #1. |
| 3 | Live Sandbox creates Workspace with multiple Agent Profiles | Production UI and public API pass: Workspace and two distinct correctly pinned Agent/Worktree pairs Ready in 15,009 ms, with no first prompts sent. Same creation evidence as #1. |
| 4 | Snapshots never interrupt creation, deletion or Agent interaction | Not satisfied: deployed whole-home capture holds writers. Requires a consistent point-in-time copy without a capture-duration writer lock, then concurrent production checks. |
| 5 | Storage physically retains only the latest snapshot | Completed publication prunes the previous generation. Runtime audit found two sealed local archives of 1,804,395,715 and 1,804,436,309 bytes. Runtime scratch and orphan cleanup remain unsatisfied; do not equate metadata pruning with physical retention acceptance. |
| 6 | Terminated Sandbox snapshots are deleted | Pending complete storage audit, including runtime scratch and orphan objects. |
| 7 | All lifecycle screens show secret-free bounded live logs and sync time | Shared lifecycle progress exists. Audit all relevant screens and actual telemetry; verify bounded scrolling in production. |
| 8 | New Agent fields are understandable, profiles have anchored hover details, deleted choices are excluded | Initial batch df6e58e3; production creation exposed mobile hover previews intercepting Create workspace. Corrected and deployed in fa3f6850: previews leave dialog header/footer reachable, selection dismisses pending/open preview. Real clicks and geometry pass at 1440x900, 390x900 and 390x600, both appearances; 1,131 frontend tests pass. |
| 9 | No review/consent flash for no-review Triggers | Pending browser verification against the required Trigger, including fresh sign-in and resumed Launch. |
| 10 | Profiles and Marketplace use CMA-styled Ornn-inspired cards/details | Pending source comparison, implementation and responsive production verification. |
| 11 | Separate collapsible Live and Paused Sandbox groups | Deployed in web 40300b5e. Production desktop/mobile checks pass for both groups and persisted collapse on reload. Live transition cases additionally covered by local tests. |
| 12 | Only the named Trureturing Trigger is used for Trigger testing | Active constraint. Older disposable-Trigger harnesses must not be reused unchanged. |
| 13 | Persist Trigger/manual provenance and owner-defined quick messages | Pending schema, public API, configuration, UI and production verification. Required messages: Contribute to Github source; Introduce Truretruing. |
| 14 | No unavailable Sandbox/Workspace expansion controls or children | Sandbox gating retained; preparing/deleting/failed Workspaces now also hide children and expansion controls. Local tests pass; production verification pending. |
| 15 | Standard Sandbox states; deleted hidden; deletion dialog explains removal | Resource-specific labels implemented; snapshots no longer replace Live/Pausing. Explicit Needs attention remains for genuine failures. Production coverage pending. |
| 16 | IME confirmation Enter never sends; ordinary Enter sends | Deployed in web 40300b5e. Production desktop/mobile synthetic composition, keyCode 229, Shift+Enter and ordinary Enter checks pass; draft retained. Ordinary-send requests intercepted before reaching the Agent. Native OS IME selection still requires a real keyboard check, not claimed by browser-generated events. |
| 17 | Agent header and sidebar show consistent resource state | Implemented in web 566c9979: one selected-Agent status projection uses timestamped detail, navigation and live observations; stale history cannot override newer state. Local tests include stream/navigation transitions. Production transition verification pending. |
| 18 | Notifications animate on showing and dismissal | Pending shared notification audit, implementation and reduced-motion checks. |
| 19 | Real native terminal and chat share one conversation, all CLI features, matching theme and reliable sizing | Pending pinned-engine feasibility and implementation. An independent second conversation or imitation terminal is not acceptance. |
| 20 | gh through NyxID GitHub OAuth; identity per user; three-service Connect/Verify flow | Pending runtime/proxy implementation and connection-flow verification. Do not hardcode author identity in shared profiles. |
| 21 | Agent, Workspace and Sandbox deletion completes correctly | Durable live-child Workspace cascade deployed in 565f6286. Production zero/one/two-child deletion passed in 1,933 / 14,971 / 16,887 ms; desktop/mobile dialogs, reload recovery, page closure, Deleted Agents and removed Worktrees verified. Test Sandbox retirement completed. Same-Sandbox snapshot concurrency remains blocked by #4; do not call the full requirement complete. |

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
- 2026-09-08 14:48 UTC: README Trigger repeat passed on the existing named
  Trigger. Launch `lch_cfba2c6e8be84a4e9cb4020f7f8b2883` reused the owner Sandbox,
  Workspace, Agent and Worktree and preserved both full turn-array hashes.
  Total measured duration: 516,183 ms. Home restore occupied approximately
  330 seconds; initial runtime provisioning approximately 107 seconds.
  Both Launch and Sandbox restore screens were observed at desktop/mobile sizes
  with unavailable children hidden, followed by restored history and enabled
  chat. First prompt was not replayed. Evidence: current-readme-repeat-X5hH1g.
  Source was GitHub's authenticated rendered README delivered at its source
  origin, not an actual signed-in GitHub browser page. Fresh sign-in/no-review
  transition acceptance remains separate.
- 2026-09-08 15:03 UTC: Web fa3f6850 deployed at
  `sha256:4ec6feb00786fd57f3828053fa04cc5ee04d364fea6ac0b53f00bff45d8a0f7e`.
  Linux/amd64 build, fixed HIGH/CRITICAL image scan, remote digest verification,
  dependency audit (zero findings), secret scan, format, TypeScript, ESLint and
  1,131 frontend tests pass. Rollout healthy; API/runtime configuration unchanged.
  Rollout evidence: unattended-web-rollout-j8q9IB. Preview action evidence:
  profile-preview-actions-42fabN (production), profile-preview-actions-mzNvYu
  (local). Those geometry/action checks forwarded zero mutations.
- Production Workspace creation evidence: workspace-profile-counts-P4vsSj,
  verified-0.json, verified-1.json, verified-2.json and creation-result.json.
  Each Workspace has a resolved commit and the expected distinct Agent/Profile/
  Worktree mapping. These are actual browser admissions, not mocked creations.
  The aggregate deletion check failed separately with workspace_has_live_agents;
  creation-result.json intentionally does not claim deletion acceptance.
- Both explicitly created test Sandboxes were retired through the public API;
  all four temporary test profiles were retired. Owner histories were rehashed
  after cleanup and remained unchanged. No disposable Trigger was created.
  The owner Sandbox subsequently became Paused with latest epoch 29, saved at
  2026-09-08T15:07:21Z; this verification issued no owner pause or rotation.
- Runtime snapshot investigation: /home/cma and checkpoint scratch use overlayfs.
  A tiny isolated copy-on-write probe from a home file was rejected with
  Inappropriate ioctl for device. The probe created no cloned files; its empty
  /tmp directory disappeared with the subsequent normal runtime teardown.
  Simply removing writer fencing would not prove point-in-time consistency.
  Host disk remains 56 GiB free; Docker restart or local artifact pruning was
  not necessary.

## Next Verification Work

- Verify the deployed selected-Agent status projection during actual production
  message and stop transitions, including desktop/mobile and refreshed history.
- Verify deletion while the same Sandbox is snapshotting after #4 is corrected.
  Standalone live-child Workspace deletion now passes production acceptance.
- Continue the complete register above, especially consistent nonblocking
  snapshots, storage cleanup, Trigger quick messages/provenance, GitHub proxy
  authority and the genuine synchronized terminal. These remain unsatisfied.

## Latest Deployment Evidence

- 2026-09-08 15:45 UTC: API and web 565f6286 deployed directly from the feature
  branch. API digest `sha256:fef1e63af7bb2c67cc76fd13f2fee9dd4460da4263aa6719fc1d3d409fe72a35`;
  web digest `sha256:11736782a8785b9ffaca2cd634968e40c7f7c66a446e8490bc096eff71a58d31`.
  Runtime configuration unchanged. Both rollout health checks passed with zero
  restarts; both owner histories were rehashed and preserved. Evidence:
  lifecycle-api-rollout-jIAtig, lifecycle-api-postflight-0CZ3B7,
  unattended-web-rollout-wsCprt.
- Source verification: 1,589 isolated Rust tests, 1,133 frontend tests, Clippy,
  formatting, TypeScript, ESLint, dependency policy/audit, source secret scans,
  Linux/amd64 builds, fixed HIGH/CRITICAL scans and remote digest checks pass.
  Mongo 8.0.19 replica-set transaction test passes. The initially attempted 8.0.13
  image download stalled and was stopped; the cached pinned 8.0.19 image was used.
  A shared-process logging assertion failed under plain cargo test but passed
  with the repository's process-isolated nextest runner; no test was bypassed.
- Production evidence workspace-profile-counts-S6C9hG/result.json: creation with
  zero/one/two Agent Profiles passed again in 5,075 / 16,109 / 15,669 ms.
  Workspace deletion with one/two live children passed via real desktop/mobile
  clicks. Reload and page closure required no repeat writes. All affected Agents
  became Deleted and their Worktrees Removed before success was shown. Temporary
  Sandbox and all four test profiles were retired; no Trigger was created.
- README repeat current-readme-repeat-P0tw8P passed, Launch
  lch_09015d714df7455692491772cb31077d. Existing hierarchy and both complete
  history hashes were preserved; first prompt was not replayed. Revival took
  507,807 ms, so the slow restore requirement remains open. Source was GitHub's
  rendered README delivered at its source origin, not a signed-in GitHub browser
  page. Fresh sign-in/no-review and the full three-service connection journey
  are not proved by this repeat.
- Status synchronization source 566c9979 passed 1,138 frontend tests, TypeScript,
  ESLint, format and source secret scanning. Its web image build and vulnerability
  scan pass; production transition verification is tracked separately above.
- Disk remained between 53 and 57 GiB free. No Docker restart, cache pruning,
  owner pause or runtime rotation was issued during this batch.

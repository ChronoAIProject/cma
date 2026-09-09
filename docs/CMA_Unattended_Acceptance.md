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
| 6 | Terminated Sandbox snapshots are deleted | API 8a6cac05 deployed. Fresh production pause/resume and deletion test zosocG confirms zero snapshot objects and HTTP 404 for the previously readable file while the conversation record remains. Read-only observer pEIoAc verifies all 691 legacy objects across 18 Deleted Sandboxes removed (20.407 GiB), with exact owner snapshot keys, both histories and retained Sandbox records preserved. Provider version-history and unclassified orphan audits remain separate. |
| 7 | All lifecycle screens show secret-free bounded live logs and sync time | Live activity deployed, with latest 100 allowlisted entries, bounded scroll, sync time and reconnect/access-loss handling. Production Sandbox/Agent pause/resume and all three deletion dialogs pass. Named README Launch/Sandbox restore displays pass at desktop/mobile, followed by restored chat. Some lifecycle rows remain generic completed-stage entries; every transition and creation surface is not yet comprehensively proved. |
| 8 | New Agent fields are understandable, profiles have anchored hover details, deleted choices are excluded | Initial batch df6e58e3; production creation exposed mobile hover previews intercepting Create workspace. Corrected and deployed in fa3f6850: previews leave dialog header/footer reachable, selection dismisses pending/open preview. Real clicks and geometry pass at 1440x900, 390x900 and 390x600, both appearances; 1,131 frontend tests pass. |
| 9 | No review/consent flash for no-review Triggers | Local consent persistence and six desktop/mobile, light/dark connection-only browser cases pass, including reload with already accepted software trust. Production connected-owner README admission passes without a review in ysYXXI. Missing-connection and fresh-sign-in production verification remain pending. |
| 10 | Profiles and Marketplace use CMA-styled Ornn-inspired cards/details | Shared cards deployed in web 7c01741f and author names in API 7e7d458e. Production 18-case verification passes all three profile kinds in both directories at 1440/390/320px, light/dark, full-card detail navigation and Marketplace Use availability. Screenshots inspected. Evidence: profile-cards-64FpRT/result.json. |
| 11 | Separate collapsible Live and Paused Sandbox groups | Deployed in web 40300b5e. Production desktop/mobile checks pass for both groups and persisted collapse on reload. Live transition cases additionally covered by local tests. |
| 12 | Only the named Trureturing Trigger is used for Trigger testing | Active constraint. Older disposable-Trigger harnesses must not be reused unchanged. |
| 13 | Persist Trigger/manual provenance and owner-defined quick messages | API 20e580fb/web 623cba22 deployed; both requested messages remain published in named Trigger revision 3. Production browser checks HVxuxo and avwGVx pass at 1440/390/320px, both themes, for source attribution, exact intercepted clicks, descriptions, draft preservation and authoring. Actual introduction delivery kZHSEl and contribution delivery y6wQuC pass: one click, one completed reply, preserved historical prefix and second Agent history, unchanged hierarchy and desktop/mobile reload. Contribution correctly reports no new artifacts in the clean Worktree and creates no empty PR. Positive ready-artifact PR publication is not exercised. |
| 14 | No unavailable Sandbox/Workspace expansion controls or children | Sandbox gating retained; preparing/deleting/failed Workspaces now also hide children and expansion controls. Local tests pass; production verification pending. |
| 15 | Standard Sandbox states; deleted hidden; deletion dialog explains removal | Resource-specific labels implemented; snapshots no longer replace Live/Pausing. Explicit Needs attention remains for genuine failures. Production coverage pending. |
| 16 | IME confirmation Enter never sends; ordinary Enter sends | Deployed in web 40300b5e. Production desktop/mobile synthetic composition, keyCode 229, Shift+Enter and ordinary Enter checks pass; draft retained. Ordinary-send requests intercepted before reaching the Agent. Native OS IME selection still requires a real keyboard check, not claimed by browser-generated events. |
| 17 | Agent header and sidebar show consistent resource state | Production desktop/mobile Ready -> Working -> Ready -> Stopped transitions passed with zero MutationObserver mismatches, including reload. Evidence: agent-status-sync-oU399d/result.json. Normal-stop notice correction in web 57f793aa also passed actual live transitions: agent-status-sync-uzRQlH/result.json. |
| 18 | Notifications animate on showing and dismissal | Shared fades deployed in web 5d840795 across creation/deletion, Agent notices, profiles, operator settings, Trigger messages and dialogs. All 1,194 frontend tests pass. Production notification-motion-anY733 verifies real opacity changes, inert dismissal, immediate focus/scroll release, quick reopen and reduced-motion removal at 1440/390/320px. Retry errors are injected read failures; zero resource mutations forwarded. |
| 19 | Real native terminal and chat share one conversation, all CLI features, matching theme and reliable sizing | Pending pinned-engine feasibility and implementation. An independent second conversation or imitation terminal is not acceptance. |
| 20 | gh through NyxID GitHub OAuth; identity per user; three-service Connect/Verify flow | API/web 7c913fa6 and runtime d052e746 deployed. The owner explicitly received api-github access with original pins preserved. Shared Sandbox Profile revision 2 and named Trigger revision 3 require all three services. Actual runtime gh 2.100.0 REST and GraphQL calls through NyxID identify chronoai-shining; connected-owner README admission and named service links pass production checks. Contribution execution and missing-connection/fresh-sign-in production verification remain pending. |
| 21 | Agent, Workspace and Sandbox deletion completes correctly | Durable live-child Workspace cascade deployed in 565f6286. Production zero/one/two-child deletion passed in 1,933 / 14,971 / 16,887 ms; desktop/mobile dialogs, reload recovery, page closure, Deleted Agents and removed Worktrees verified. Test Sandbox retirement completed. Same-Sandbox snapshot concurrency remains blocked by #4; do not call the full requirement complete. |

## Baseline

The previously deployed API includes bounded concurrent lifecycle reconciliation;
see `CMA_Production_Verification.md` for image digests and recorded checks. That
improves unrelated deletion latency but does not remove snapshot writer fencing.
The latest recorded README-trigger revival took 478,740 ms; performance work remains.

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

- Untouched-Agent pause/resume and the initial three-minute snapshot delay now
  pass production verification. The full thirty-minute boundary has not been
  observed in production. The named README Trigger repeat on the new runtime
  passed in current-readme-repeat-DsHXDD; restore performance remains open.
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
- 2026-09-08 16:10 UTC: Status synchronization passed actual production
  Ready -> Working -> Ready -> Stopped transitions at 1440x900 and 390x900.
  Both MutationObservers recorded zero header/sidebar mismatches and reload
  retained Stopped. Evidence: agent-status-sync-oU399d/result.json. The harmless
  prompt ran only in a temporary default Agent; its Sandbox was retired and both
  owner histories were preserved. No Trigger was created.
- Normal-stop refinement 57f793aa passed 1,142 frontend tests, TypeScript,
  ESLint, formatting, tracked-source secret scanning, Linux/amd64 build, fixed
  HIGH/CRITICAL image scanning and remote digest verification. The manifest pins
  web digest fcd64fe955e26eb953fa4098d86b25de1f54e1be244070efbca04a9db97a10a4.
  A broad working-directory scan reported two private-key markers in generated
  pem dependency .rmeta files; the tracked-source scan passed without exceptions.
  Read-only baseline normal-stop-notice-hpzdfU confirms the retained stopped
  Agent reload is already clean; a live Stop transition is needed to verify the
  refinement, because that is where the previous misleading notice appeared.
- Web 57f793aa rollout completed healthy, API/runtime unchanged. Evidence:
  unattended-web-rollout-IOfDDv. Production live transition verification
  agent-status-sync-uzRQlH passed at desktop/mobile with zero status mismatches
  and zero false normal-stop error notices. Its exact test Sandbox was retired;
  owner histories remained unchanged. Read-only reload check also passed in
  normal-stop-notice-9ZEBab.
- API source 7b84e10d changes initial periodic snapshot scheduling from immediate
  to the configured interval after the persisted runtime binding time. Existing
  snapshot cadence and manual/automatic pause capture remain unchanged.
  All 1,591 isolated Rust tests, 181 focused checkpoint tests, Clippy, formatting,
  doc-test gate and source scanning pass. Existing timing fixtures were advanced
  to the newly required age; no test was skipped or weakened. Linux/amd64 build,
  fixed HIGH/CRITICAL scan and remote digest verification pass. Production
  behavior verification follows deployment; #4 is still unsatisfied.
- Profile card source 7c01741f passed 1,146 frontend tests and local read-only
  browser verification against real production profiles. All three kinds in
  Profiles and Marketplace passed full-card navigation at 1440, 390 and 320px,
  in both themes. Evidence: profile-cards-AIhuVT/result.json. Screenshot review
  found a pre-existing account UUID author label; directory-name correction is
  needed before calling the full discovery experience accepted.
- 2026-09-08 16:38 UTC: Initial snapshot timing API 7b84e10d deployed at
  sha256:133603555de7332cdc156a9bd35e87e9b536dca1de5e1787d0c3d2f9bc843162.
  Evidence: lifecycle-api-rollout-kQcR2Z and lifecycle-api-postflight-Z6PPNf.
  Initial postflight 3OfsJS saw a transient /auth/session 503; retry and later
  authenticated reads passed. This is not evidence of an outage-free rollout.
- Production timing observation initial-snapshot-delay-BuaUr1 recorded no early
  periodic capture through 178,316 ms. The subsequent manual pause failed with
  restore_failed before checkpoint publication. Runtime logs proved a sealed
  home archive and writer release after abort; the unused Agent home had no
  sessions directory. The exact temporary Sandbox was retired successfully.
  Do not treat that timing run as manual-pause acceptance.
- 2026-09-08 16:46 UTC: Profile cards web 7c01741f deployed at
  sha256:20392b31d02955365def18447d4500c8b1874b5921d47d352936b36774f86994.
  Evidence: unattended-web-rollout-bUOG2d. Author-name API 7e7d458e subsequently
  deployed at sha256:1f2eb45a2a81e85379538e90bc90428eee8564c27fcff254427905217c382636.
  Evidence: lifecycle-api-rollout-TU314f and lifecycle-api-postflight-kTKjTJ.
  Both owner histories preserved, no helper/runtime Pods, zero container restarts.
  The author-name change passed all 1,591 isolated Rust tests, Clippy, source
  scanning, and six Mongo marketplace transaction tests. Its image scan and
  remote digest verification passed. Missing public names display Author
  unavailable; system-owned profiles display CMA. Ownership IDs are unchanged.
- Production profile-cards-64FpRT passed 18 cases with zero mutations. Screenshots
  show By Shining rather than the account UUID, wrapping card titles at 320/390px,
  correct light/dark appearance and native full-card detail navigation.
- Provider correction 14c05f26 persists new empty Codex conversations using the
  provider's own metadata operation. An offline Linux/amd64 process restart/load
  proof passed against the exact production archive with zero turns. Original
  and patched bridge bytes are independently pinned. Runtime image build and
  production pause/resume acceptance follow; #4 remains unsatisfied.
- Disk checks remained at 54-55 GiB free. Only disposable local probe containers
  and the exact failed timing-test Sandbox were removed; no owner history,
  persistent local volume, worktree or Docker cache was deleted. No merge to
  develop or main was performed.
- Runtime 14c05f26 deployed at
  sha256:dc2742368ac98d3497e3841cc46997b7d2f2051453c5dcb6c9e184a11eeaa1d0
  in a fully checked runtime-absent window. API/web image identities are unchanged;
  the runtime ConfigMap pin and declarative pod-template annotation reload the API
  environment. Evidence: persistence-runtime-rollout-NHgQUo and
  persistence-runtime-postflight-QlOyUS. Both owner Agent histories preserved,
  zero helper/runtime Pods and zero container restarts at postflight.
- The runtime build repeats the offline zero-turn restart/load proof. Its
  Linux/amd64 image scan, exact executable hashes, runtime contract campaign,
  remote digest verification and ten image-evidence regressions pass. Local
  kubeconform was unavailable; Kubernetes schema validation with client dry-run
  accepted both changed manifests without applying them.
- Production initial-snapshot-delay-xfAfzR passed: no initial periodic capture
  during the three-minute observation, manual pause in 23,550 ms, revive in
  26,095 ms, and the same underlying conversation digest in a distinct replacement
  runtime. Agent, Workspace and Worktree IDs and the zero-turn transcript were
  preserved. No model call or first prompt was sent. Both owner history hashes
  remained unchanged, and the exact test Sandbox was retired successfully.
  These timings concern a small default Sandbox, not the large Trureturing home.
- Initial retry mJyE8x reached Ready but its read-only verifier traversed a
  concurrently removed plugin scratch directory. The harness now reads only
  sessions; the same restriction was added to the offline test. The retry's
  exact test Sandbox was cleaned up. This harness failure is not pause acceptance
  and does not replace the successful xfAfzR evidence.
- Disk remained at 49 GiB free after the runtime image and local cache were built.
  No Docker restart, worktree deletion or cache pruning was necessary.
- README Trigger repeat current-readme-repeat-DsHXDD passed on runtime 14c05f26,
  Launch lch_df34aa7e0b1f4b8481da775a8548b092. Duration 505,850 ms; the existing
  hierarchy and both complete turn-array hashes were preserved, and the first
  prompt was not replayed. Launch/Sandbox progress, unavailable children, restored
  chat and enabled composer were observed at desktop/mobile sizes. Source was
  GitHub's authenticated rendered README delivered at its source origin, not a
  signed-in GitHub browser page. Fresh sign-in/no-review remains unproved.
- A subsequent read at 17:36 UTC observed checkpoint epoch 32, started at
  17:34:14 UTC, shortly after revival completed at 17:32 UTC. Latest completed
  snapshot was epoch 31 at 16:14:23 UTC. Source inspection confirms restored
  snapshots bypassed runtime-age scheduling, causing an immediate overdue
  capture. A correction now takes the later snapshot/binding baseline, with
  restart, configurable-interval and exact-boundary regression cases. Deployment
  and production timing verification remain pending; #4 remains unsatisfied.
- Web 224bd525 deployed at
  sha256:59c27ce6e4502d6f7181b30d6dbd7e440b20288c28c2b0394ae84ceb281411a6.
  Evidence: unattended-web-rollout-thCsyT; rollout healthy, API/runtime unchanged.
  All 1,157 frontend tests, TypeScript, ESLint, formatting, dependency audit,
  source scanning, Linux/amd64 build, fixed HIGH/CRITICAL image scan and remote
  digest verification pass. Local browser evidence live-activity-readonly-uWh3Jm
  and production live-activity-readonly-zTI7kV cover both Sandbox and Agent pages
  at 1440/390/320px, both themes, bounded scrolling, latest-entry following and
  timestamp visibility. Screenshots were inspected; no commands were forwarded.
- The first local browser check found Agent progress above the conversation could
  be scrolled out by chat following. Progress now follows the last message, and
  the browser proof requires viewport intersection as well as DOM visibility.
  Resize observation keeps the log pinned across font/theme changes, unless the
  reader manually scrolls away.
- Production live-deletion-flow-x3Pqhr and live-deletion-flow-YfxcZ5 both observed
  live pause and resume logs on desktop/mobile Sandbox and Agent pages. Neither
  is complete deletion acceptance: the first stopped at a navigation-readiness
  race before deletion, the second inspected an empty pre-acceptance event view
  too early. Both exact disposable Sandboxes were retired in finally cleanup.
  A further regression now loads Agent events when deletion is already complete
  even if the request response remains unconfirmed; source db43770a.
- API e1c3260c deployed at
  sha256:6e1be506cc2150ad7b6cbed3431b196302705d4203d7ec5a6be8d95735aa5a40.
  Evidence: lifecycle-api-rollout-yPbL2E and lifecycle-api-postflight-f8NauX.
  The owner Sandbox had automatically paused with snapshot epoch 33; zero runtime
  Pods and settled records were checked before dispatch. Runtime/web identities
  were unchanged, both complete owner histories were preserved, and postflight
  reported zero helper Pods and zero container restarts. All 1,592 isolated Rust
  tests, Clippy, formatting, doc-test gate, dependency policy, image scan and
  remote digest checks pass. The 173 environment-gated tests were not run by that
  nextest invocation. Full production snapshot timing remains unobserved.
- Snapshot epoch 32 completed at 17:38:47 UTC after starting at 17:34:14 UTC:
  273 seconds before publication. This is not nonblocking-snapshot acceptance.
  Host disk remained at 48-49 GiB free. No Docker restart, cache pruning or
  worktree removal was necessary; owner resources were not force-paused.
- Web db43770a rollout i7dQav completed healthy. Production read-only evidence
  live-activity-readonly-ijVJ01 covers six Sandbox/Agent viewport cases and both
  appearances. All 1,158 frontend tests, type/lint/format, image scan and digest
  checks passed. API/runtime were unchanged.
- Deletion attempts BglalQ and UYVolr confirmed Agent and Workspace removal but
  stopped before Sandbox UI deletion. UYVolr captured HTTP rate-limit refusal in
  place of navigation. Both disposable Sandboxes were retired in finally cleanup.
  The concurrently running README verifier xTCdJv also received HTTP 429. These
  are not successful end-to-end results; no owner pause or repeated click followed.
- The admitted named README Launch lch_d95d499a7c0248ed83887153c05a4fbd continued
  after the verifier closed. Read-only continuation admitted-readme-observation-9ojyo6
  observed Home restore, queued delivery, Active and Ready. Both complete history
  hashes and reused Agent identity were preserved, with no first-prompt replay.
  Desktop/mobile Launch and Sandbox logs were bounded and timestamped. Ready was
  observed at 18:32:12 UTC, following admission at approximately 18:24:21 UTC.
  Three subsequent minutes (183,205 ms) had no early capture despite overdue
  snapshot epoch 33. No full thirty-minute boundary or fresh sign-in was tested.
- Web 7e400e66 removes deleted subtree reads from initial navigation, refresh and
  minute boundaries. All 1,159 frontend tests, TypeScript, lint, format, audit,
  source scan, Linux/amd64 build and image scan pass. Production digest
  16d0b7a63d2d3ce6dd3ec9df79d4d07ad0034cf803783c6bf53dd3c5fa327095;
  rollout unattended-web-rollout-G6jxUR healthy, API/runtime unchanged.
  Browser evidence navigation-retired-reads-oB5bKY (local) and rWERyC (production)
  covers 1440/390/320px, both themes, reload and a minute of observation. All 29
  Deleted Sandboxes generated zero detail requests, with no HTTP 429 or mutations.
- Production live-deletion-flow-9BRTqz passed Agent, Workspace and Sandbox deletion
  through actual sidebar controls, visible bounded activity, completion dialogs,
  hard refresh and removal. Agent Worktree was Removed. Owner histories were
  preserved and the exact disposable Sandbox was retired. This does not prove
  concurrent deletion during a writer-fenced snapshot.
- Read-only storage audit checkpoint-storage-audit-2qwui9 found 746 current logical
  objects, all mapped to known CMA Sandboxes. The Active owner's one archive used
  55 objects; 18 Deleted Sandboxes retained 691 objects in 29 archives, totaling
  20.407 GiB. Backend-version history and runtime-local archive retention remain
  separate, unproved checks. The audit performed no storage mutations.
- Snapshot retirement sources 0aca3dae/8a6cac05 pass 1,593 isolated Rust tests,
  Clippy, formatting and tracked-change secret scans. The 175 environment-gated
  tests were skipped by nextest; all 15 archive Mongo integration tests were run
  separately against an ephemeral, failpoint-enabled local replica set and pass.
  Temporary test containers were removed with no persistent volumes. Production
  rollout and physical snapshot cleanup evidence follow separately.
- API 8a6cac05 deployed at
  sha256:f77c06036019b2788b51026a17fa2f3a3cacc2a6edda06be36dd9643a506511d.
  Linux/amd64 build, fixed HIGH/CRITICAL image scan, remote digest verification
  and server-side manifest dry-run pass. Rollout lifecycle-api-rollout-MUzTDL
  followed a fresh zero-runtime inventory. Postflight lifecycle-api-postflight-XF1ykz
  confirms both complete owner history hashes, unchanged web/runtime pins, no
  helper/runtime Pods and zero restarts. Previous preflight refusals correctly
  waited while the normal owner pause was still tearing down its runtime.
- The owner automatically saved epoch 34 at 18:51:04 UTC after pause admission
  at 18:46:31 UTC, again approximately 273 seconds to publication. No owner pause,
  forced teardown or runtime rotation was issued. #4 remains unsatisfied.
- Production live-deletion-flow-zosocG passed real desktop/mobile Sandbox/Agent
  pause and resume progress, Agent/Workspace/Sandbox deletion, bounded activity,
  completion dialogs and hard refresh. Its snapshot was listed and readable
  before deletion, absent from storage afterwards, and its exact key returned
  HTTP 404. The deleted Agent record remained readable. All disposable runtime
  resources were retired; both owner histories remained unchanged. Screenshots
  and snapshot-after-delete.json were inspected.
- Background cleanup observer retired-snapshot-backlog-imedu7 is watching only
  the previously audited 18 Deleted Sandbox prefixes. The initial 691 objects
  are decreasing after deployment; it performs no deletions itself. A successful
  terminal result must additionally preserve the owner's exact epoch-34 object
  list, both complete history hashes, and all 18 retained Sandbox records.
- Disk stayed at 45-48 GiB free during this batch. No Docker restart, worktree
  deletion or Docker cache pruning was necessary; only exact disposable local
  Mongo test containers and production test Sandboxes were removed.
- 2026-09-08 19:10 UTC: Legacy cleanup observer retired-snapshot-backlog-pEIoAc
  passed with zero remaining objects across all 18 audited Deleted Sandboxes.
  The application removed 691 objects, totaling 21,911,491,522 bytes; the verifier
  performed no deletions. Exact epoch-34 owner snapshot keys and metadata, both
  complete conversation hashes, and all retained Deleted Sandbox records match
  the pre-cleanup evidence. Provider version-history was not audited.
- The earlier observer imedu7 stopped on storage-list HTTP 500 while the cleanup
  was still advancing. The read-only helper now has bounded retry for transient
  responses and renews its machine token before expiry. The successful rerun
  needed zero retries; no root cause for the earlier upstream 500 is claimed.
  Host disk remains at 45 GiB free; Docker was not restarted or pruned.
- Trigger message sources 20e580fb/623cba22 pass 1,597 isolated backend tests,
  1,173 frontend tests, 1,744 TypeScript SDK tests, Rust SDK tests and shared
  118-operation public-contract replay. The 175 environment-gated backend tests
  were not part of the isolated run. Type checks, formatting, lint, Clippy,
  documentation checks, dependency policy and tracked-change secret scans pass.
- Local read-only browser evidence trigger-quick-messages-jsqnNX and
  quick-message-authoring-HbCjlK covers 1440/390/320px, both themes, named source,
  exact guarded shortcut requests, draft retention, eight-message mobile bounds,
  descriptions and authoring fields. Zero commands reached production, and both
  owner histories and the saved Trigger draft remained unchanged. Screenshot
  review caught bounded-row and overlapping-tooltip defects, corrected before
  the final build. Theme checks wait for existing control transitions to settle.
- API/web Linux/amd64 builds, fixed HIGH/CRITICAL image scans and remote digest
  verification pass. The vulnerability database refresh initially blocked in
  docker-credential-desktop. Using an empty temporary Docker credential directory
  for the public database download completed it in 18 seconds without changing
  saved credentials, restarting Docker or weakening scan policy. Host disk has
  42 GiB free. Runtime 14c05f26 stays unchanged; deployment acceptance follows.
- API 20e580fb and web 623cba22 deployed at digests
  37abdce9b17aad7c80763fb1b6fb06fc06a94f8f6c52de7938a65714fc675655 and
  e52e4c5863d75e65c8901a91894f1d40f34b7cde5102e0d128044dc4559fdb13.
  Evidence: lifecycle-api-rollout-7TVXXp, unattended-web-rollout-oDkMTt and
  lifecycle-api-postflight-q2a3a4. Fresh all-owner runtime-absence guard passed;
  both complete owner histories, runtime pin and thirty-minute checkpoint
  interval were preserved. Both deployments are healthy, no helper Pods and
  no container restarts. Server-side manifest validation passed.
- configure-trigger-messages-mAVfRO performed exactly one draft-version-guarded
  save and one publication on Truretruing Formalize and Contribute Trigger.
  Revision 2 contains Contribute to Github source and Introduce Truretruing.
  All other content, original first message, profile references, reuse policy,
  placements and both histories were preserved. No new Trigger was created.
- Production trigger-quick-messages-HVxuxo and quick-message-authoring-avwGVx
  pass actual deployed browser views at 1440/390/320px and both themes. Exact
  shortcut clicks were intercepted before reaching the model; hover, navigation,
  reload and publication did not send. These runs do not prove completed model
  delivery. A subsequent actual UI introduction click was admitted once from
  Paused in live-introduction-shortcut-kZHSEl; its result remains unclaimed until
  the durable completed turn and unchanged historical prefix are verified.
- GitHub transport feasibility: /tmp/cma-gh-socket-proof.jexgnI/result.json
  proves the genuine gh 2.89.0 executable supports its upstream http_unix_socket
  setting for REST and GraphQL. It sends only an inert placeholder credential;
  a foreign-host request was refused by the local socket fixture. No production
  request, TLS interception or patched gh executable was used. The owner's
  current Sandbox grant still contains only chrono-llm and ornn-api. Runtime
  proxy implementation and an explicit durable api-github grant remain required;
  neither is implied by the owner's separate NyxID service connection.
- 2026-09-09 04:46 UTC: live-introduction-shortcut-kZHSEl passed actual production
  delivery of Introduce Truretruing. Exactly one click from Paused produced one
  completed turn and a 6,169-character assistant reply. Both old conversation
  histories and all hierarchy IDs were preserved; desktop/mobile reload passed.
  The original unsent draft was retained after clicking. Total verification took
  871,091 ms; Live was observed after roughly nine minutes and the completed
  reply after 824,980 ms. This is delivery acceptance, not acceptable latency.
  Screenshots were reviewed. The main Agent now has one authorized additional
  turn; future preservation baselines must include it.
- Snapshot restore sources 81ccc3b9/2c287473 replace sequential archive-part
  downloads with four ordered requests. A shared semaphore caps all concurrent
  multipart downloads at four 32 MiB buffers, including verified parts waiting
  for an earlier part or a slow reader. Every part and the entire archive retain
  exact size and SHA-256 checks; consumer cancellation drops pending requests
  and returns the shared permits. All 1,603 isolated backend tests pass, including
  six new ordering, integrity, backpressure, cancellation and shared-budget tests.
  Clippy, formatting, dependency policy and tracked-change secret scans pass;
  175 environment-gated tests remain skipped by the isolated invocation.
- The repository file-size gate reports 13 pre-existing oversized source files,
  unrelated to this snapshot change. No limit was weakened or exception added.
  The intermediate 81ccc3b9 local build was cancelled after the shared memory
  budget refinement; only the final 2c287473 image is a deployment candidate.
  Host disk remains at 41 GiB free, with no Docker restart or pruning needed.
  Production latency acceptance follows deployment; #4 remains unsatisfied.
- The final 2c287473 Linux/amd64 API build, fixed HIGH/CRITICAL image scan and
  remote digest verification pass. The deployment manifest pins
  sha256:143574b0c5da9ca4477119fa48649d196f608232e5698b28ce8bac0fca4c33c4.
  Initial read-only rollout guard 8xohIg correctly refused while the owner's
  normal automatic pause was still tearing down its runtime. The new snapshot
  was sealed at 05:05:05 UTC, 281 seconds after pause admission; no owner pause
  or runtime rotation was issued by this verification.
- GitHub read-only integration /tmp/cma-gh-nyxid-proof.oVe153 passed real gh
  2.89.0 REST account/repository and GraphQL viewer requests through a Unix socket
  and the authenticated local NyxID CLI to api-github. Both identity projections
  resolve the same owner account, with no hardcoded identity and no upstream
  writes. Foreign hosts are refused; gh sees only an inert placeholder. This is
  local transport evidence, not deployed runtime proxy or Sandbox grant acceptance.
- API 2c287473 deployed at
  sha256:143574b0c5da9ca4477119fa48649d196f608232e5698b28ce8bac0fca4c33c4.
  Evidence: lifecycle-api-rollout-DWsSY4 and lifecycle-api-postflight-OLndUU.
  Both full owner histories, web/runtime pins and the thirty-minute interval
  were preserved. Rollout is healthy with zero restarts and no helper Pods.
  Earlier checks 8xohIg/fpOUYr refused the still-existing runtime; ogAnoW refused
  because serial read-only validation exceeded the thirty-second freshness
  limit. Three bounded read-only validation workers retained every check and
  the same deadline; the fresh successful guard dispatched in a runtime-free
  window. No owner pause or force teardown was performed.
- Production snapshot-download-timing-cXswoy passed the actual private download
  endpoint for epoch 35: 1,805,702,751 bytes in 83,741 ms, exact SHA-256 verified,
  and both owner history hashes unchanged. The snapshot stayed current and the
  Sandbox stayed Paused. Anonymous access returned 401 and an unavailable epoch
  returned 404. No archive was saved locally. Sampled API memory peaked at
  121 MiB. The earlier recorded comparable large-home download took 271 seconds;
  this run is approximately 69% shorter, not a same-object controlled benchmark.
  Full README-trigger revival is being observed separately in
  current-readme-repeat-alV1hu; its admitted Launch must not be repeated.
- Production current-readme-repeat-alV1hu passed with exactly one README Trigger
  click, Launch lch_88b886793b6d4b09b0a19b7dbf86ad50, in 368,142 ms. All hierarchy
  IDs and both complete histories were preserved, with no first-prompt replay.
  Compared with the earlier 505,850 ms run, observed revival was 27% shorter.
  Home restore still occupied approximately 277 seconds and queued delivery
  approximately 61 seconds; latency is not considered finished. Desktop/mobile
  Launch and Sandbox progress was bounded and timestamped, with unavailable
  children hidden. A subsequent 181,965 ms had no early snapshot; this was not
  an overdue-snapshot or full thirty-minute boundary test. The README came from
  GitHub's authenticated rendered API at its source origin, not a signed-in
  GitHub browser page or fresh-sign-in acceptance. Do not repeat this Launch.
- Settled production trigger-quick-messages-03lBaD passed at 1440/390/320px in
  both themes: source name, descriptions, enabled shortcuts, exact intercepted
  requests and retained drafts. Zero commands were forwarded and both complete
  histories were unchanged. Reviewed settled screenshots confirm correct layout;
  the earlier README harness captured loading and an unfinished responsive grid
  transition, which are not settled visual acceptance evidence. Owner Sandbox
  and both Agents remain available after revival; no API/runtime rollout is
  permitted while any owner runtime remains. Host disk has 41 GiB free.
- Runtime GitHub transport source 41f068fa enables the genuine CLI only for
  the exact api-github Sandbox grant. Configuration and the private Unix socket
  live outside the snapshotted home. Scoped NyxID credentials stay in the
  supervisor, rotate through existing custody generations and are never given
  to gh. Foreign hosts, ambiguous destinations and credential-bearing responses
  are refused. Local tests cover exact REST/GraphQL requests, credential renewal,
  missing authority, private permissions, stale-socket recovery and active-socket
  protection. The genuine local CLI test passed for 2.89.0 and then 2.100.0.
- All 1,608 isolated backend tests, workspace Clippy, formatting, doc-test gate,
  dependency policy, configuration parity/docs and changed-source secret scans
  pass. Nextest skipped 176 environment-gated tests; the new genuine CLI test
  was run explicitly. All nine image-evidence regressions pass. The same 13
  pre-existing file-size violations remain; no gate was weakened.
- Candidate 41f068fa passed its full Linux runtime/proxy campaign but failed
  the fixed HIGH/CRITICAL scan because upstream gh 2.89.0 contained vulnerable
  dependencies. It was not pushed or deployed. The latest upstream 2.100.0
  binary still contained golang.org/x/mod 0.39.0 with CVE-2026-56864/56865.
  Sources de17fcae/d052e746 instead build the exact upstream release commit
  45437bc7eeeb3359bbfddd1742f79de7652fd3e2 with pinned Go 1.26.8 and x/mod 0.40.0.
  CLI source is unchanged, module checksums remain verified and the final build
  uses readonly module resolution. The first source build caught an absent
  indirect-module checksum; d052e746 explicitly downloads that module before
  the readonly build. Neither failed candidate was deployed.
- Runtime d052e746 Linux/amd64 build and final image scan pass at
  sha256:5c5e15c03701b81bbeaf0f214f75f4cb9ed064b185022ac5586c734fbd6ada33.
  Evidence: runtime-d052e746-contract.json, runtime-d052e746-trivy.json and the
  corresponding build/archive records in the verification scratch directory.
  All 18 real image checks pass with networking disabled, including installed
  gh REST/GraphQL through the actual supervisor proxy, foreign-host refusal,
  writer fencing, complete home restore and exact temporary-container/volume
  cleanup. Installed gh SHA-256 is
  9032e3045268a1fa9b594f50a4627f33d8575a11d258a4c1e62a82bf70ef89e8.
- Registry upload is not complete: gcloud credential refresh first returned
  internal_failure, then explicitly required interactive reauthentication.
  The refusal also occurred with the installed Python 3.14 interpreter; no
  credential was printed. A fresh gcloud auth login is required before upload
  and guarded deployment. No production manifest pin changed in this batch.
- All-owner deployment guard FGmWgv refused a still-existing runtime. A new
  runtime fd9736c0-f1e8-47e2-acc1-40eade65e3e5-0 subsequently appeared; the owner
  Sandbox was observed reviving from 05:46:24 UTC. This continuation admitted
  no owner Launch, message, pause or runtime rotation. The earlier Paused read
  preserved both full histories at five turns and one turn. Do not roll the
  API or runtime while any owner runtime remains; refresh history baselines
  before deployment because the owner is using production concurrently.
- Disk remained at 35 GiB free after both builds and recovered to 37 GiB after
  cleanup. Docker has 45 running local
  containers, so it was not restarted or broadly pruned. The rejected 41f068fa
  image had no referencing containers; only its redundant local image, OCI
  archive and extracted layout were removed. Source
  commits and verification reports remain available to reproduce it.
- Trigger service checks now cover reused Sandboxes and Agents at admission
  and explicit Start, using the frozen Trigger service list without rewriting
  instance profiles or grants. Five new regressions cover all three requested
  services, missing/inactive/unknown states, revocation before Start, malformed
  and foreign responses, provider outage, idempotent replay and legacy Drafts
  with empty requirements. Newly observed services produce durable events.
  The shared HTTP contract now correctly returns a Draft for disconnected
  GitHub on repeat Resume instead of incorrectly admitting work.
- This connection-check batch passes all 1,613 isolated backend tests (176
  environment-gated tests skipped), workspace Clippy, formatting, the doc-test
  gate, secret scans, 1,397 TypeScript SDK contract checks and the Rust SDK's
  complete shared-contract replay. The same 13 existing file-size violations
  remain; no CI gate was weakened. Production deployment is still blocked by
  Google interactive reauthentication and the owner's live runtime.
- Read-only production observation at 2026-09-09T06:05:11Z found the owner
  Sandbox Live on runtime 14c05f26, with one Workspace and no checkpoint or
  lifecycle operation in progress. Both Agent histories retain the previously
  recorded five-turn and one-turn hashes. No production mutation was made.
  Disk monitoring after tests reports 36 GiB free.
- Source 4491f42d contains the verified reused-resource service-check fix and
  is pushed without a merge. The subsequent connection-assist UI keeps Connect
  available while pending, reopens an unexpired link without another handoff,
  and makes Verify immediate with explicit pending feedback. Late/foreign
  handoff results are refused. Fixed a simultaneous three-service completion
  race that could lose earlier results and prevent automatic continuation.
  Connection-only dialogs now say Connect required accounts and Verify
  connections, distinct from publisher-required review.
- All 1,180 frontend tests, TypeScript, ESLint, changed-file formatting, source
  secret scanning, production Vite build and production-dependency audit pass.
  Seven new UI regressions cover reopen, immediate verification, in-flight
  controls, foreign/stale results, direct recheck and simultaneous completion.
  Browser evidence connection-recovery-local-wT5QQb/result.json passes six
  desktop/mobile and light/dark cases at 1440/390/320px, with inspected screenshots,
  bounded dialog geometry, no clipped buttons, exactly one mock Start after all
  three services connect and zero production requests. This is local evidence,
  not NyxID production OAuth acceptance. The first browser harness run omitted
  the optional Trigger preview response; that fixture omission was corrected.
- Google refresh still explicitly requires gcloud auth login. No registry push,
  manifest pin or deployment was performed in this batch. The d052e746 runtime
  candidate remains local; the api-github instance grant, no-review fresh-login
  journey and actual contribution remain unverified. Host disk has 35 GiB free;
  no additional Docker restart or cleanup was needed.
- No-review admission now retains the original Landing confirmation while
  connections are missing, without admitting provisioning. Start and partial
  connection rechecks preserve its exact frozen trust and timestamp; publisher
  review still requires explicit confirmation. Both stores reject rewritten or
  removed confirmations. No serialized field was added. Older API validation
  rejects these unstarted confirmed records, including cancelled records;
  rollback must retain compatible validation rather than erase consent history.
- All 1,617 isolated backend tests pass (177 environment-gated tests skipped).
  All 41 selected disposable Mongo replica-set tests pass, including real store
  reopen, retained pending consent, immutable confirmation and one continuation.
  The initial full build caught and corrected a test-only TriggerId constructor.
  Mongo initially exposed an older cooldown assertion expecting capture before
  the thirty-minute default; 2014776f tests the configured one-minute interval
  and default interval independently without changing production behavior.
  Exact labelled disposable containers were removed by the test harness.
- This consent batch passes Clippy, formatting, the doc-test gate, 1,397
  TypeScript SDK contract checks and complete Rust SDK shared-contract replay.
  Eight focused frontend checks, TypeScript and ESLint pass. Browser evidence
  recorded-consent-local-w4pbPe/result.json passes 1440/390/320px, both themes:
  reload with prior software trust, no observed review/checkbox insertion,
  bounded connection dialog, three connection completions and exactly one mock
  Start. Desktop/mobile screenshots were inspected. Zero production requests
  were sent by that harness; it is not production OAuth acceptance.
- Google authentication at 06:38 UTC still requires interactive renewal. The
  earlier CMA browser cookie returned HTTP 401 during the 06:39 UTC read-only
  observation, so no newer runtime or history status is claimed from that read.
  No registry push, manifest pin or rollout occurred. Disk has 33 GiB free.
- Sources 2014776f/d4fedc84 are pushed on the existing feature branch without
  a merge. Renewed CMA access through the locally authenticated NyxID CLI at
  06:41 UTC, using the real state/PKCE callback and checking the exact owner.
  Evidence: cli-owner-session-rTwYoz/result.json. This was authentication only:
  no Trigger visit or resource command, and not a fresh-browser UX proof.
- At 06:42 UTC the owner Sandbox was Live on unchanged runtime 14c05f26, with
  one Workspace and no capture/lifecycle operation. The main Agent now has six
  turns, SHA-256 c51e47b1b45c5459d6ebec1b7595c504bea18b06034a8c81896c488c5eba4c77.
  At 06:44 UTC its original five-turn prefix matched the previous full-history
  hash exactly. The other Agent retains its one-turn hash
  1e9e314f75d3045ce6b26e8d9be46b3f7ba580143c23c509afb5b553b11be303.
  This continuation sent no message, Launch, pause or other owner-work mutation.
  Subsequent preservation checks must include the owner's newly added turn.
- Corrected Checking what's needed to remain blocked for missing/inactive/unknown
  accounts, with no completed timestamp. Start completes it only after successful
  verification; preparation-time connection loss and recovery update the same
  durable step and event stream. Partial checks preserve the blocked step without
  duplicate completion. Updated shared HTTP expectations, preserving the exact
  u64 overflow fixture after a mechanical JSON edit initially rounded its value.
- The progress batch passes all 1,617 isolated backend tests and 41 selected
  Mongo integration tests, Clippy, Rust formatting, 1,397 TypeScript SDK contract
  checks and Rust SDK replay. Nineteen focused frontend tests, TypeScript, ESLint
  and formatting pass. Browser recorded-consent-local-Xrgcol/result.json passes
  all six viewport/theme cases, connection-only reload and actual completed-step
  rendering after verification. Screenshots were inspected; no production
  requests were sent. The same 13 existing source-size violations remain; no
  gate was weakened. Disposable Mongo containers are absent. Disk has 32 GiB free.
- Final frontend run passes all 1,182 tests across 108 files. The final Rust
  doc-test gate also passes. These commits are source/test evidence only;
  production API, web and runtime images have not changed during this turn.

## Urgent admin owner visibility — 2026-09-09

- Implemented retained public creator names on authorized admin Sandbox/Profile
  inventory and Sandbox detail, actor and user-target names on Activity, and
  immutable creator identity plus Owner name on the ordinary Sandbox page.
  Resource authorization precedes name resolution. Mongo resolves a bounded
  page in one projected query; missing names remain explicitly unavailable.
  Operations groups named Agents beneath their Workspaces with profile links.
  Original ownership and conversation authority are unchanged.
- Local validation: 177 Layer 1 tests, 29 focused frontend tests, both SDK suites,
  six Mongo integration tests, Clippy, web typecheck/lint/build, and desktop/mobile
  browser proof pass. Browser evidence: cma-operator-browser-uOkAow (eleven
  screenshots, keyboard navigation, no browser errors). Disposable Mongo was
  removed without persistent volumes.
- Production read-only check at 07:30 UTC confirms Shining's admin account sees
  another owner's Sandboxes. Two foreign Sandboxes are Live. Shining's original
  Sandbox is Paused at revision 741 with its Workspace and two Agents retained.
  No owner message, Launch, pause, deletion or runtime mutation was sent.
- Google Cloud authentication recovered. Source 2759e53c passed Linux/amd64
  builds, image vulnerability scans and remote digest verification. Web rollout
  admin-web-rollout-RPYxdw completed first; its verified image is
  sha256:c97093e52292bea6cfe239a7aa60bc4fb6d3e5463d795b839cb7f686e160d806.
- The owner then explicitly authorized pausing live Sandboxes and rolling out.
  Only Shining's Sandbox remained Live; one normal pause request was admitted
  at 07:59:25 UTC (admin-hotfix-authorized-pause-dhSipV). Snapshot epoch 40 was
  saved at 08:04:02 UTC, and pause completed at 08:07:12 UTC. The two foreign
  Sandboxes were already Paused. No runtime was forcibly deleted.
- API rollout admin-api-rollout-cR6ejI completed at approximately 08:08 UTC with
  image sha256:d4ed2928d52b78aad91ec86c3ec7556e576b23ada8cf92efbd6e36874624f890.
  The guard verified all-owner paused/retired inventory, absent runtimes,
  settled lifecycle work, valid snapshots and current transcripts immediately
  before dispatch. Postflight admin-api-postflight-4L5FJI preserved the full
  hashes of all 35 Sandbox records, their Workspace/Agent summaries and both
  owner conversations (eight turns and one turn). Runtime configuration and
  the thirty-minute snapshot setting are unchanged. Both system pods are
  healthy with zero restarts. Feature-branch pins are committed and pushed;
  neither develop nor main was merged.
- Production acceptance admin-visibility-prod-PzR2y2 passes with actual owners
  Shining and calvin tan, the foreign Workspace/Agent hierarchy, 200 foreign
  Activity entries, 26 Profile inventory rows, and the ordinary Sandbox Owner
  field. Human creator/actor fields contain names. Anonymous admin reads are
  denied and ordinary foreign resource routes remain owner-scoped. Desktop
  light and mobile dark browser checks pass with no resource commands, page
  errors or horizontal overflow. Settled repeat admin-visibility-prod-HQDwHD
  also passes after awaiting profile-name resolution; desktop hierarchy and
  mobile Owner screenshots were inspected.
  The full acceptance register remains open, including service grants and
  nonblocking snapshots. Host disk has 28 GiB free; no cleanup was needed.

## Resumed service-access work — 2026-09-09

- Restored the named service-grant WIP after admin production acceptance, retaining
  the hotfix's public-name module and both SDK owner fields. A grant appends
  explicit owner authority while fully Paused, preserves original profile and
  creation scope, skips pre-grant credential generations and obtains replacement
  authority before revival delivery. Replay after revival is a no-op.
- All 1,629 isolated backend tests pass. Three older pause fixtures were corrected
  to retain their admitted service scope when replacing test credentials; the
  production guard against implicit scope rewrites remains enforced. Mongo
  service-access integration verifies atomic publication, rejected forged grants,
  restart durability and immutable original scope; its disposable container was
  removed without persistent volumes.
- Shared public contract capture and both SDK replays pass. The TypeScript suite
  has 1,765 tests, including the new grant event. Rust SDK tests and documentation
  pass. The new oversized-body case retains HTTP 413 and the existing hierarchy
  body's invalid_json code; only its draft expectation and new inventory entry
  were aligned. Unrelated fixture formatting changes were removed.
- The Sandbox page reads the effective public service list, including owner
  grants, with Agent lineage retained for older servers. Thirty-eight focused UI
  and transport tests, web typecheck, lint and production build pass. The paused
  page resolves named service links even when Agent details are unavailable.
- Runtime d052e746 is now uploaded and its remote Linux/amd64 digest is verified:
  sha256:5c5e15c03701b81bbeaf0f214f75f4cb9ed064b185022ac5586c734fbd6ada33.
  At this checkpoint it was registry evidence only; deployment and instance
  authorization subsequently completed as recorded below.
- API/web source 7c913fa6 and runtime d052e746 deployed from the feature branch.
  API digest f42cf6504627e37f207b727383f4c53a966da51e8a23bf609809e4396c87b4ac;
  web digest be48809869efc9e33ae8a353a9b8db3fa46799d26555a1624c6012715ee96a75.
  Linux builds, vulnerability scans, remote digest checks and rollouts pass.
  Evidence: service-access-web-rollout-CYhPWc, service-access-api-rollout-Ta0rfB,
  service-access-api-postflight-49XKwS. All 35 Sandbox records, child summaries
  and both complete owner transcripts were preserved across the API rollout.
  Admin visibility was reverified in admin-visibility-prod-7jRqzN.
- Owner service-access command truretruing-github-grant-FYRT0G appended api-github
  while fully Paused, preserving the original creation scope, Profile revision 1,
  latest checkpoint, hierarchy and conversations. No original scope was rewritten.
  Configuration evidence truretruing-three-service-profile-vHnrry publishes the
  same Sandbox Profile at revision 2 and named Trigger at revision 3 with
  chrono-llm, ornn-api and api-github. Both quick messages, no-review behavior,
  Workspace/Agent pins, public ID and README placement were preserved.
- Runtime evidence runtime-github-readonly-OfWjfY passes with genuine gh 2.100.0:
  REST /user and GraphQL viewer both identify chronoai-shining through the
  runtime private socket and NyxID GitHub proxy. No GitHub writes or Agent
  messages were sent by that check. Effective-service browser evidence
  effective-services-prod-t1bXFF passes at 1440/390/320px in both themes:
  GitHub OAuth, Chrono LLM and ornn-api link to their original NyxID service pages.
- README repeat current-readme-repeat-ysYXXI passes, Launch
  lch_8369f8ad113a4d43926804fd102dd51c, in 478,740 ms. All three account checks
  succeeded. Existing Sandbox, Workspace, Agent and Worktree identifiers and
  both complete transcript hashes were preserved, with no first-prompt replay.
  Main Agent retained eight turns and the second retained one. Home restore
  took approximately 283 seconds; runtime provisioning approximately 99 seconds;
  queued delivery approximately 60 seconds. Performance remains unsatisfied.
  Launch/Sandbox desktop/mobile logs were bounded, timestamped and updated.
  An overdue prior snapshot did not trigger another capture during the first
  182,823 ms after revival; the full thirty-minute boundary was not observed.
  Source was authenticated GitHub rendered README delivered at its source origin,
  not a signed-in GitHub browser page or fresh-login connection acceptance.
  Host disk has 26 GiB free. No Docker restart or cleanup was necessary.
- Actual contribution quick-message evidence live-contribution-shortcut-y6wQuC
  passes. One real browser click admitted one completed turn, retaining the draft,
  original eight-turn prefix, second Agent's complete history and all identifiers.
  The Agent inspected the current Worktree and conversation, found no modified,
  staged, untracked or ahead-of-source artifacts, and explained why no contribution
  was made. It correctly created no empty PR or commit. Desktop/mobile reloads
  and screenshots pass. This exercises the requested no-artifacts behavior;
  successful PR publication is separate from the read-only GitHub proxy proof.
  Subsequent preservation checks must retain the new ninth turn.
  Service-link repeat effective-services-prod-RVX4XD passes after both Agent
  settings and their profile and skill names finish loading; the mobile screenshot
  was inspected. Earlier service-only screenshots caught pending Agent references.

## Restore performance and landing update — 2026-09-09

- Runtime source 7630fb16 removes the redundant full payload decompression during
  restore. It validates archive identity and the complete manifest before staging,
  then checks file hashes, payload length and the compression trailer while
  extracting before replacing home. Upload publication retains its independent
  full validation. Corrupt/truncated trailers, trailing members and payload length
  mismatches preserve the old home and remove incomplete staging.
- All 174 runtime tests pass (two environment-gated tests skipped), including
  28 focused checkpoint checks. Clippy, formatting, workspace doc-test gate and
  source scanning pass. Linux/amd64 build, image scan, remote digest verification
  and all 18 real-image runtime checks pass. Candidate archive digest:
  sha256:481dff74bcc87cef36ed29ac0d94465ae9fdc94d6dc97b0e3e717437f7be22ba.
  Evidence: runtime-7630fb16-contract.json and matching build/scan/push records.
- Controlled local runtime-restore-benchmark-ZETDBg uses the same 256 MiB fixture,
  two CPU limit and three restores per image. Median restore falls from 2,144 ms
  to 1,763 ms (17.8%). Every restored file hash matches. Exact temporary containers
  and volumes were removed. This is local comparison evidence, not production
  latency acceptance, and does not remove the snapshot writer hold.
- One authorized normal pause was admitted in admin-hotfix-authorized-pause-EL71KF.
  Snapshot saved at 09:02:21 UTC. Read-only authorized-pause-observation-BQlP4r
  preserved both complete histories, now nine turns and one turn, through teardown.
  A new user-admitted named-Trigger Launch lch_fc6da80f167a45e3b928050bd054baaf
  arrived at 09:04:46 UTC during teardown and started revival. The observer stopped
  when state changed; no repeated pause or runtime rollout was dispatched.
  Candidate 7630fb16 remains undeployed and the runtime manifest still pins d052e746.
- The user additionally requested a one-line signed-out product title, larger
  cloud and larger matching invitation type. Web source 276b830b implements a
  responsive one-line title and a 264px by 88px cloud with bold 24px Shrikhand.
  Signed-in character behavior and activation timing remain unchanged.
  Six focused existing tests, formatting, TypeScript, lint, build and source
  scan pass. Local browser cloud-landing-a9ndQD covers 1440/768/390/320px, both
  themes, hover/five-second reveal, rendered Three.js, reduced motion, static
  fallback and single sign-in handover.
- Web 276b830b deployed at
  sha256:f8374a4c8c9d0aaa796f419a91760393f9bad48d1a07ec435b35e5f7aafa30e0.
  Linux build, image scan, remote digest check and server manifest dry run pass.
  Rollout service-access-web-rollout-Slh7xI is healthy; API/runtime remain unchanged.
  Production cloud-landing-bSjsEv passes all six browser cases with one visible
  title line and larger matching-font cloud, no horizontal overflow or page
  errors, plus real NyxID sign-in handover. Desktop/mobile screenshots inspected.
  Host disk has 25 GiB free; no broad Docker restart or cleanup was needed.

## Notification animation rollout — 2026-09-09

- Web source 5d840795 adds shared 200ms showing/dismissal fades to notification
  messages and dialogs. Dismissed elements become inert and accessibility-hidden
  immediately. Modal focus and scroll locks release immediately; a new opening
  cancels the pending removal. Conditional creation, deletion, review and Advanced
  parents retain their exit lifecycle. Credential recovery aborts outstanding
  checks and closes its popup as soon as the containing notice exits.
- All 1,194 frontend tests pass. Six presence tests include rapid reopen, focus,
  timer cleanup and live reduced-motion changes. Existing disappearance assertions
  now verify the inert exit frame and its eventual removal. TypeScript, ESLint,
  formatting, production build and source secret scan pass. An independently
  committed js-yaml 4.3.2 lockfile update fixes the newly reported high-severity
  empty-merge CPU advisory; npm ci and the high-severity audit gate pass. Three
  moderate development-only Vitest findings remain reported by npm audit.
- Linux/amd64 image build, fixed HIGH/CRITICAL vulnerability scan and remote
  digest verification pass. Web 5d840795 is deployed at
  sha256:59eb529a8b6e594bb641ab6787f73c34ac034ca89d236ed349d8983b4b6035dd.
  Server manifest dry run and rollout pass, with healthy API and web pods and
  zero restarts. Rollout evidence: service-access-web-rollout-M8INfv.
- Browser evidence notification-motion-Ye347e (local) and
  notification-motion-anY733 (production) passes at 1440/390/320px, light/dark,
  including reduced motion. Measured opacity changes cover Advanced, deletion
  confirmation, New Agent and profile retry notices; focus/scroll release and
  quick reopen pass. Profile failures were injected read responses; no resource
  mutation reached production. Screenshots inspected. The observer was corrected
  to stop attributing reopened or disconnected DOM frames to a prior exit phase.
- Latest landing requirements remain accepted after the web rollout:
  cloud-landing-Xytl4Q passes all six production cases, including one-line title,
  larger cloud and matching Shrikhand text, real sign-in handover and no overflow.
  Desktop/mobile screenshots were inspected. Disk still has 25 GiB available.

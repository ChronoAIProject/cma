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
| 9 | No review/consent flash for no-review Triggers | Pending browser verification against the required Trigger, including fresh sign-in and resumed Launch. |
| 10 | Profiles and Marketplace use CMA-styled Ornn-inspired cards/details | Shared cards deployed in web 7c01741f and author names in API 7e7d458e. Production 18-case verification passes all three profile kinds in both directories at 1440/390/320px, light/dark, full-card detail navigation and Marketplace Use availability. Screenshots inspected. Evidence: profile-cards-64FpRT/result.json. |
| 11 | Separate collapsible Live and Paused Sandbox groups | Deployed in web 40300b5e. Production desktop/mobile checks pass for both groups and persisted collapse on reload. Live transition cases additionally covered by local tests. |
| 12 | Only the named Trureturing Trigger is used for Trigger testing | Active constraint. Older disposable-Trigger harnesses must not be reused unchanged. |
| 13 | Persist Trigger/manual provenance and owner-defined quick messages | API 20e580fb/web 623cba22 deployed; named Trigger revision 2 publishes both requested messages. Production browser checks HVxuxo and avwGVx pass at 1440/390/320px, both themes, for source attribution, exact intercepted clicks, descriptions, draft preservation and authoring. Actual introduction delivery kZHSEl passed: one click from Paused, one completed reply, preserved historical prefix and second Agent history, unchanged hierarchy and desktop/mobile reload. GitHub contribution execution remains dependent on #20. |
| 14 | No unavailable Sandbox/Workspace expansion controls or children | Sandbox gating retained; preparing/deleting/failed Workspaces now also hide children and expansion controls. Local tests pass; production verification pending. |
| 15 | Standard Sandbox states; deleted hidden; deletion dialog explains removal | Resource-specific labels implemented; snapshots no longer replace Live/Pausing. Explicit Needs attention remains for genuine failures. Production coverage pending. |
| 16 | IME confirmation Enter never sends; ordinary Enter sends | Deployed in web 40300b5e. Production desktop/mobile synthetic composition, keyCode 229, Shift+Enter and ordinary Enter checks pass; draft retained. Ordinary-send requests intercepted before reaching the Agent. Native OS IME selection still requires a real keyboard check, not claimed by browser-generated events. |
| 17 | Agent header and sidebar show consistent resource state | Production desktop/mobile Ready -> Working -> Ready -> Stopped transitions passed with zero MutationObserver mismatches, including reload. Evidence: agent-status-sync-oU399d/result.json. Normal-stop notice correction in web 57f793aa also passed actual live transitions: agent-status-sync-uzRQlH/result.json. |
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

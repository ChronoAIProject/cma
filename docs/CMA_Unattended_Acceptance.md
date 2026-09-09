# Unattended Production Acceptance

Scope: the owner's 21 requirements dated 2026-09-08, updated by the public
Sandbox Profile warmup and availability requirements dated 2026-09-09. Build from
`feat/trureturing-trigger-loop-20260907` and deploy directly; do not merge to
`develop` or `main`. A passing local test is not production acceptance.

Preserve existing owner identities, histories and work. Future Trigger checks
must use or amend **Truretruing Formalize and Contribute Trigger**, never create
another disposable Trigger. Never record credentials, private logs or tokens.
Monitor disk usage and inspect exact targets before removing stale artifacts.

## Acceptance Register

### Current priority: public Profile warmup and availability

- Implement owner-controlled Enable warm up and a target count from one to five,
  exclusively for published public Sandbox Profiles.
- Prepare the runtime and credential-free environment before a visitor arrives;
  atomically assign one ready runtime, then bind that visitor's ownership and
  scoped credentials. Concurrent claims, retries and controller restarts must
  never assign one runtime to two users or expose another user's credentials.
- Immediately replenish claimed slots to the configured count. Reconcile pool
  records and provider inventory durably, including stale Profile/image revisions.
- Public Agent, Workspace and Sandbox Profiles remain usable but not editable
  by other users. Provide an explicit editable private-copy path; private copies
  use fresh provisioning and never consume their source Profile's pool.
- Add Always keep alive to Sandbox Profiles, including their existing Sandboxes;
  disable inactivity pause while preserving explicit manual pause.
- Configure the existing Truretruing Sandbox Profile with warmup enabled, five
  ready instances and Always keep alive enabled, then prove the behavior in prod.

The availability and warm-runtime implementation is deployed. Truretruing has
warmup enabled with a target of five and Always keep alive enabled. Five corrected,
installed, credential-free environments have been observed in production; full
claim/replenishment, keepalive and retained acceptance work is tracked below.
Terminal synchronization is explicitly deferred by the owner; the other earlier
requirements remain.

| #   | Required behavior                                                                                          | Current evidence and remaining work                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| --- | ---------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | Live Sandbox creates Workspace with zero Agent Profiles                                                    | Production UI and public API pass: empty-directory Workspace Ready with a resolved commit, zero Agents and zero Worktrees in 4,750 ms. Evidence: workspace-profile-counts-P4vsSj/creation-result.json.                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| 2   | Live Sandbox creates Workspace with one Agent Profile                                                      | Production mobile UI and public API pass: Workspace and one correctly pinned Agent/Worktree Ready in 24,396 ms, with no first prompt sent. Same creation evidence as #1.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| 3   | Live Sandbox creates Workspace with multiple Agent Profiles                                                | Production UI and public API pass: Workspace and two distinct correctly pinned Agent/Worktree pairs Ready in 15,009 ms, with no first prompts sent. Same creation evidence as #1.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| 4   | Snapshots never interrupt creation, deletion or Agent interaction                                          | Not satisfied: deployed whole-home capture holds writers. Requires a consistent point-in-time copy without a capture-duration writer lock, then concurrent production checks.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| 5   | Storage physically retains only the latest snapshot                                                        | Latest storage API audit checkpoint-storage-audit-avJWl4 finds one visible archive per remaining Sandbox, 110 objects and no unmatched visible objects. Latest restored runtime has one sealed archive and no staging files. Raw provider objects, version history and runtime scratch retention across repeated captures remain unverified; the service's logical listing does not prove physical retention.                                                                                                                                                                                                                                                        |
| 6   | Terminated Sandbox snapshots are deleted                                                                   | API 8a6cac05 deployed. Fresh production pause/resume and deletion test zosocG confirms zero snapshot objects and HTTP 404 for the previously readable file while the conversation record remains. Read-only observer pEIoAc verifies all 691 legacy objects across 18 Deleted Sandboxes removed (20.407 GiB), with exact owner snapshot keys, both histories and retained Sandbox records preserved. Provider version-history and unclassified orphan audits remain separate.                                                                                                                                                                                        |
| 7   | All lifecycle screens show secret-free bounded live logs and sync time                                     | Live activity deployed, with latest 100 allowlisted entries, bounded scroll, sync time and reconnect/access-loss handling. Production Sandbox/Agent pause/resume and all three deletion dialogs pass. Named README Launch/Sandbox restore displays pass at desktop/mobile, followed by restored chat. Some lifecycle rows remain generic completed-stage entries; every transition and creation surface is not yet comprehensively proved.                                                                                                                                                                                                                           |
| 8   | New Agent fields are understandable, profiles have anchored hover details, deleted choices are excluded    | Initial batch df6e58e3; production creation exposed mobile hover previews intercepting Create workspace. Corrected and deployed in fa3f6850: previews leave dialog header/footer reachable, selection dismisses pending/open preview. Real clicks and geometry pass at 1440x900, 390x900 and 390x600, both appearances; 1,131 frontend tests pass.                                                                                                                                                                                                                                                                                                                   |
| 9   | No review/consent flash for no-review Triggers                                                             | Local consent persistence and six desktop/mobile, light/dark connection-only browser cases pass, including reload with already accepted software trust. Production connected-owner README admission passes without a review in ysYXXI. Missing-connection and fresh-sign-in production verification remain pending.                                                                                                                                                                                                                                                                                                                                                  |
| 10  | Profiles and Marketplace use CMA-styled Ornn-inspired cards/details                                        | Shared cards deployed in web 7c01741f and author names in API 7e7d458e. Production 18-case verification passes all three profile kinds in both directories at 1440/390/320px, light/dark, full-card detail navigation and Marketplace Use availability. Screenshots inspected. Evidence: profile-cards-64FpRT/result.json.                                                                                                                                                                                                                                                                                                                                           |
| 11  | Separate collapsible Live and Paused Sandbox groups                                                        | Deployed in web 40300b5e. Production desktop/mobile checks pass for both groups and persisted collapse on reload. Live transition cases additionally covered by local tests.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| 12  | Only the named Trureturing Trigger is used for Trigger testing                                             | Active constraint. Older disposable-Trigger harnesses must not be reused unchanged.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 13  | Persist Trigger/manual provenance and owner-defined quick messages                                         | API 20e580fb/web 623cba22 deployed; both requested messages remain published in named Trigger revision 3. Production browser checks HVxuxo and avwGVx pass at 1440/390/320px, both themes, for source attribution, exact intercepted clicks, descriptions, draft preservation and authoring. Actual introduction delivery kZHSEl and contribution delivery y6wQuC pass: one click, one completed reply, preserved historical prefix and second Agent history, unchanged hierarchy and desktop/mobile reload. Contribution correctly reports no new artifacts in the clean Worktree and creates no empty PR. Positive ready-artifact PR publication is not exercised. |
| 14  | No unavailable Sandbox/Workspace expansion controls or children                                            | Sandbox gating retained; preparing/deleting/failed Workspaces now also hide children and expansion controls. Local tests pass; production verification pending.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| 15  | Standard Sandbox states; deleted hidden; deletion dialog explains removal                                  | Resource-specific labels implemented; snapshots no longer replace Live/Pausing. Explicit Needs attention remains for genuine failures. Production coverage pending.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| 16  | IME confirmation Enter never sends; ordinary Enter sends                                                   | Deployed in web 40300b5e. Production desktop/mobile synthetic composition, keyCode 229, Shift+Enter and ordinary Enter checks pass; draft retained. Ordinary-send requests intercepted before reaching the Agent. Native OS IME selection still requires a real keyboard check, not claimed by browser-generated events.                                                                                                                                                                                                                                                                                                                                             |
| 17  | Agent header and sidebar show consistent resource state                                                    | Production desktop/mobile Ready -> Working -> Ready -> Stopped transitions passed with zero MutationObserver mismatches, including reload. Evidence: agent-status-sync-oU399d/result.json. Normal-stop notice correction in web 57f793aa also passed actual live transitions: agent-status-sync-uzRQlH/result.json.                                                                                                                                                                                                                                                                                                                                                  |
| 18  | Notifications animate on showing and dismissal                                                             | Shared fades deployed in web 5d840795 across creation/deletion, Agent notices, profiles, operator settings, Trigger messages and dialogs. All 1,194 frontend tests pass. Production notification-motion-anY733 verifies real opacity changes, inert dismissal, immediate focus/scroll release, quick reopen and reduced-motion removal at 1440/390/320px. Retry errors are injected read failures; zero resource mutations forwarded.                                                                                                                                                                                                                                |
| 19  | Real native terminal and chat share one conversation, all CLI features, matching theme and reliable sizing | Explicitly deferred by the owner on 2026-09-09 in favor of public Profile warmup and availability.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| 20  | gh through NyxID GitHub OAuth; identity per user; three-service Connect/Verify flow                        | API 7c913fa6, web 5d840795 and runtime 7630fb16 deployed. The owner explicitly received api-github access with original pins preserved. Shared Sandbox Profile revision 2 and named Trigger revision 3 require all three services. Actual runtime gh 2.100.0 REST and GraphQL calls through NyxID identify chronoai-shining; connected-owner README admission, named service links and the no-artifacts contribution message pass production checks. Positive ready-artifact PR publication and missing-connection/fresh-sign-in production verification remain pending.                                                                                             |
| 21  | Agent, Workspace and Sandbox deletion completes correctly                                                  | Durable live-child Workspace cascade deployed in 565f6286. Production zero/one/two-child deletion passed in 1,933 / 14,971 / 16,887 ms; desktop/mobile dialogs, reload recovery, page closure, Deleted Agents and removed Worktrees verified. Test Sandbox retirement completed. Same-Sandbox snapshot concurrency remains blocked by #4; do not call the full requirement complete.                                                                                                                                                                                                                                                                                 |

## Baseline

The previously deployed API includes bounded concurrent lifecycle reconciliation;
see `CMA_Production_Verification.md` for image digests and recorded checks. That
improves unrelated deletion latency but does not remove snapshot writer fencing.
The latest recorded README-trigger revival reached Ready in 377,000 ms;
performance and continuous activity feedback work remain.

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
  authority. Terminal synchronization is deferred under the updated scope.

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

## Restore optimization deployment — 2026-09-09

- Owner Sandbox completed its normal pause with a snapshot saved at 09:30:39 UTC.
  No additional owner pause was requested. Runtime 7630fb16 was configured after
  all 35 Sandbox records were Paused or Deleted and no runtime/operation/lease
  remained. API application source stays 7c913fa6 and web stays 5d840795.
  Manifest server dry run and API rollout pass with runtime digest
  sha256:481dff74bcc87cef36ed29ac0d94465ae9fdc94d6dc97b0e3e717437f7be22ba.
- Rollout preflight initially rejected one passive old-key revocation record
  left by the explicit service-access grant. Source and existing recovery tests
  show that this record remains until rotation or retirement; it is not active
  work. The guard now accepts it only for a fully Paused Sandbox with no work,
  runtime or lease, and only when it differs from current/pending credentials.
  It does not delete or alter the debt. Full-record postflight comparison proves
  its preservation alongside the rest of the hierarchy.
- service-access-api-rollout-cyJeBD and service-access-api-postflight-jlKXAD pass:
  all 35 exact Sandbox records, Workspaces, Agents and both owner histories are
  preserved. Main Agent has nine turns with SHA-256
  7904c24cdd76addc8f9832caf5beafdfe3750fe319b81fe161ca411d8d0e41c7;
  second Agent has one turn with its existing hash.
- Read-only physical object listing checkpoint-storage-audit-avJWl4 finds
  exactly one archive per remaining Sandbox: 52, 55 and 3 chunks, respectively,
  with no unmatched objects. Deleted Sandboxes have no archives. This is current
  object-list evidence only; provider version history and runtime scratch
  retention remain open. Disk has 24 GiB free.
- Named README Trigger Launch lch_1ae3cc65b8a44b1e8c352f274aa9e430 reached
  Ready at 09:48:59 UTC, 377,000 ms after admission. It reused the same Sandbox,
  Workspace, Agent and Worktree on runtime 7630fb16, retained the exact nine/one
  turn hashes and reported Existing conversation preserved. Read-only supplemental
  current-readme-repeat-vSj378/readiness-result.json passes these checks with
  both Agents Ready and no active snapshot/lifecycle operation.
- The original vSj378 harness did not pass its complete campaign: all early live
  activity samples had the same last entry, failing the activity-advancement
  assertion after readiness and transcript checks. That failure is retained;
  continuous restore feedback remains unaccepted. No second Trigger admission or
  additional pause was issued to repeat the test. The observation-only scratch
  probe found one 1,806,570,892-byte sealed runtime archive and no upload/capture/
  restore-staging temporary object after extraction. Total revival is still over
  six minutes; this production observation is not a controlled speed benchmark.
- Supplemental vSj378/view-result.json passes restored Agent views at
  1440/390/320px after source/history reads and responsive layout settle. The
  message field and its complete placeholder fit inside each viewport, with
  no horizontal document overflow and zero forwarded mutations. Mobile screenshot
  inspected. The original immediate-resize screenshot captured the grid midway
  through its transition; the settled viewport checks verify the completed layout.

## Pause transfer and revival settlement fixes — 2026-09-09

- Source 6587dd72 separates already-published checkpoint hold validation from
  capture/export. The production control transport retains authentication,
  request/receipt correlation and writer-recovery behavior while avoiding a
  repeated full archive transfer. Core still compares the epoch, object identity,
  digest and writer token with the pinned checkpoint. Initial capture retains
  complete export and independent archive validation; no runtime wire change.
- Source a0e306c6 bounds internal post-restore Agent/metadata settlement retries
  to five seconds or the configured shorter delay. External failures and
  ambiguous-dispatch grace remain unchanged. Conflict handling still preserves
  concurrent input, parent revisions and replacement worker leases.
- All 1,634 tests in the default isolated backend suite pass (178 excluded by
  the existing profile), including three new authenticated transport tests and
  a settlement test for both one- and sixty-second configured retry delays.
  Existing dispatch recovery now verifies the full sixty-second backoff.
  Formatting, Clippy, doc-test gate, dependency policy and source scans pass.
  The Linux/amd64 API image build is in progress; these two changes are not yet
  deployed or accepted for production latency. Snapshot capture still blocks
  writers, and the native synchronized terminal remains unimplemented.
- Read-only observation finds the owner's Sandbox naturally Pausing at
  10:08:43 UTC, in pause_agents after successful checkpoint publication.
  No additional pause was admitted. Disk has 23 GiB free.
- Storage audit precision: checkpoint-storage-readonly.mjs uses the authenticated
  service bucket listing. Inspection of the storage service source shows that
  this route projects logical objects and hides internal mutation records. The
  earlier "physical object listing" wording must not be read as a raw provider
  or version-history audit. Its 110 visible-object count remains valid evidence
  for the service API only; physical storage acceptance is still open.

## Availability and durable warm runtime implementation — 2026-09-09

- Production readback confirms API a0e306c6 and Web 5d840795 at 1/1 Ready.
  The pause-transfer/settlement API above was deployed successfully; evidence is
  service-access-api-rollout-szle2s and service-access-api-postflight-xAiUzb under
  /tmp/cma-trureturing-loop.LvTezb. All 35 Sandbox records and owner histories
  were preserved. current-readme-repeat-Hax3pL/result.json records the named
  README-derived repeat reaching Ready in 338,892 ms, preserving both histories
  and avoiding first-prompt replay. This proves the existing-owner repeat only;
  fresh sign-in, missing connections and performance acceptance remain open.
- Commit a65fafb4 adds mutable availability outside published environment
  revisions, owner-only settings, public/published warmup eligibility, one-to-five
  validation, private-copy defaults and transactionally fenced keepalive policy.
  The original 1,197-test frontend run had one stale button-label expectation;
  its corrected routed test, production build, E2E typecheck and format checks
  now pass. The audit passes the required high-severity threshold with three
  moderate development-tool advisories. Backend and both SDK suites passed.
- The warm-runtime worker is now connected to production composition in source.
  It reserves durable credential-free environments, proves installation before
  Ready, atomically transfers exclusive ownership at admission and wakes immediate
  replenishment. Runtime identity and installation receipts are reused before
  user keys are minted. Lost warm runtimes receive a new persisted generation
  under the same newly owned Sandbox ID. Profile edits/unshare, worker leases,
  runtime reclamation and stale-pool capacity are fenced.
- Four focused runtime tests pass, including credential ordering, exact runtime
  reuse, lost-runtime recovery and invalid installation/identity refusal. Eight
  real replica-set admission tests pass, including concurrent allocation capped
  at five, ten competing owners, Trigger replay/quota preservation, private-copy
  isolation, unshare cleanup, changed-revision worker fencing and ownership
  handoff. The isolated Mongo containers were removed without persistent volumes.
- These availability/warmup changes are not deployed yet. Truretruing warmup five
  and keepalive remain to be enabled and verified after image/rollout checks.
  This entry does not close the retained snapshot, live-progress, storage or
  fresh-user verification requirements. Terminal synchronization remains deferred.
- The default backend suite passes 1,642 tests (186 excluded/ignored by default).
  A subsequent warm-runtime recovery review found that startup reconciliation
  could apply checkpoint-loss failure before the initial provisioning worker
  replaced a disappeared warm pod. The credential-free initial step now retains
  provisioning authority; all 86 lifecycle-worker tests and Clippy pass afterward.
  Dependency policy, source secret scanning and deployment configuration checks
  pass. The repository file-size check reports 12 pre-existing oversized files;
  none is introduced by this warm-runtime change, and that gate remains open.
- Production preflight observation finds only CMA API/Web pods, three Paused
  Sandboxes and the remaining records Retired. Truretruing's existing public
  Profile is draft version 2. No owner pause was issued for this build.

## Warm runtime rollout and shared identity correction — 2026-09-09

- API f4b309f3 and Web a65fafb4 were built for Linux/amd64, scanned, pushed and
  deployed directly from the feature branch. Commit d0fcf9e5 records their
  immutable manifest pins. The deployed API digest is
  672212def83aadbd6225430ec73ed81fcc0b53d387723b75cdebf27026e19a1e;
  the Web digest is
  99f1edc8006ae502462c8f70346dcb021c7609e64ae861c0d28cf82905951f20.
  Runtime 7630fb16 is unchanged. Both deployments were rechecked at 1/1 Ready.
  Evidence: service-access-api-rollout-15D239,
  service-access-api-postflight-gsNaWA and service-access-web-rollout-ReNEZc,
  under /tmp/cma-trureturing-loop.LvTezb. All 35 owned Sandbox records,
  existing child summaries and both owner transcript hashes were preserved.
  Three existing Sandboxes were already Paused; no owner pause was issued.
- The actual Profile UI saved and reloaded warmup enabled, count five, and
  Always keep alive. Evidence: warmup-profile-configuration-R7x4kW. Desktop
  inspection passed; the mobile check found no horizontal overflow. Availability
  remains separate from immutable environment publication.
- The existing namespace quota allowed only four 4-CPU/8-GiB warm environments
  alongside CMA's own resource requests. The exact sandbox-quota ResourceQuota
  was checked and patched from requests 20 CPU/40 GiB and limits 40 CPU/80 GiB
  to requests 40 CPU/80 GiB and limits 80 CPU/160 GiB; the pod cap remains 50.
  The patch fenced the existing resource UID and old values. These are ceilings,
  not allocated capacity. The cluster autoscaler supplies requested node capacity.
  Evidence: warmup-quota-XFjERM. This consumed-platform quota is documented here;
  no additional manifest was added to the fixed infra file list.
- The initial revision-2 pool reached five installed runtimes after the quota
  correction, but inspection discovered four hardcoded Git author/committer
  environment variables in the shared Profile. That cohort was not accepted as
  reusable across users. It was replaced through normal Profile publication and
  pool reconciliation; no unowned pod was manually deleted.
- Production Sandbox Profile revision 3 removes those four variables and pins
  Workspace Profile revision 2. The Workspace runs credential-dependent identity
  setup after ownership and credential delivery: gh api user goes through CMA's
  local NyxID GitHub proxy, validates the returned login/numeric account ID, and
  sets the repository's user.name and GitHub noreply user.email. Worktrees inherit
  that repository identity. Agent Profile revision 2 explicitly directs GitHub API
  operations through gh. The existing named Trigger revision 4 pins all three
  corrected revisions. Availability, placement, quick messages and first prompt
  were preserved. Existing owned Sandbox pins and both original conversations
  were rechecked unchanged. Evidence: truretruing-user-identity-vkR1jv.
- The identity setup was exercised with real gh and git against two different
  fixture account responses over a local Unix socket. Both produced the matching
  effective Git author identity; malformed identity was rejected before writing
  repository configuration. An initial run found that gh can rewrite its config
  into YAML. The published command therefore reads the socket setting through
  gh config get, rather than assuming the file remains JSON. This is fixture
  coverage of two identities, not a claim of a second authenticated prod account.
- corrected-warm-pool-JgCsW8 confirms five replacement environments with the
  corrected public environment values, successful installation receipts, runtime
  phase Installed and authenticated admin 404s proving no owned Sandbox records
  yet exist. The installed phase precedes credential delivery. No fixed Git
  identity remains in their pod environment. A separate direct Mongo read was
  unavailable from the local network; no temporary diagnostic pod was created.
  Actual warm admission is the subsequent proof that a durable Ready reservation
  transfers to an owned Sandbox with the same runtime identity.
- A fresh README-derived launch is running in fresh-warm-readme-JMKRsK. To
  exercise a first visit with the existing owner account, the same named Trigger
  was temporarily published with Always new policies (revision 5), then its
  original reuse policies were immediately restored (revision 6) after admission.
  This retained the corrected Profile pins, existing placement and first prompt.
  The initial observations prove an existing warm Sandbox was assigned, retained
  runtime generation 1, became Active at 52,423 ms and reached Launch Ready at
  103,412 ms. First-response, replenishment and UI results are still pending at
  this entry; readiness is not first-response completion.
- Host disk remains at 18 GiB free. No Docker restart, broad cache cleanup, merge
  to develop/main or owner resource deletion was performed. The earlier 12-file
  size-gate failures and retained nonblocking-snapshot, complete lifecycle,
  fresh-sign-in/missing-connection and physical storage audits remain open.

## Warm admission and private-copy production acceptance — 2026-09-09

- The original first-prompt check exposed an onboarding problem beyond Sandbox
  provisioning: the configured formal-answer workflow started a full Lean build
  to answer a beginner introduction. Launch Ready was observed, but the response
  did not complete in the fifteen-minute observation window. After verifying
  that the exact test Agent still contained only that test's first prompt, its
  obsolete response was interrupted through the public API. Evidence:
  obsolete-welcome-interrupt-PLqMxP. This test is not first-response acceptance.
- The named Trigger's welcome now requests a concise README/documentation-based
  introduction, three examples and a follow-up question, without builds, tests,
  dependency installation or research-artifact creation for onboarding. The
  research profiles, quick messages, placement, service requirements and normal
  reuse policies are unchanged. Evidence: trigger-welcome-refinement-845hzz.
  After the subsequent controlled fresh-admission test, production Trigger
  revision 11 restores the normal reuse policies and retains this welcome.
- Fresh launch lch_ad7b80be18a24ec1b32863de61da9995 was admitted from the actual
  GitHub-rendered README badge in fresh-warm-readme-qPTEZr. It consumed the warm
  runtime already created at 11:53:00 UTC; ownership and credential generation 1
  were added without changing the provider runtime or generation. Active was
  observed at 45,158 ms. A transient transcript GET returned HTTP 503 during
  startup, ending that browser observation; it did not cancel the durable Launch
  or Agent response. Verification continued against the same admitted Launch,
  with no additional admission or prompt.
- warm-readme-continuation-5HM3qf passes actual completed first response, exactly
  one initial prompt, correctly owned hierarchy, matching Git identity in the
  main repository and Worktrees through gh 2.100.0, enabled composer, reload
  persistence and preserved original owner histories. The final reply is the
  requested concise introduction, three examples and next-step question, and
  explicitly distinguishes documentation from executed tests. Its last reply
  timestamp is 275 seconds after admission; this includes model work and is not
  the Sandbox provisioning duration. No transcript read failures occurred during
  this continuation. The earlier 503 remains recorded and is not an outage-free
  journey claim.
- The same evidence confirms five installed, credential-free unowned runtimes
  after assignment. Replacement pod sbx_42d821d7f2f3464f8767eaa9ddbdbe19 was
  created at 12:14:23 UTC, five seconds after the 12:14:18 UTC admission. The
  claimed runtime is absent from the unowned pool. The private-copy test also
  independently observed the first claim's replacement already installed in the
  replenished five-member pool.
- Agent private-copy UI creation, editing, save and reload passed in
  private-copy-production-fubEB6 before its later Workspace verifier failure.
  Workspace and Sandbox copies passed the same browser checks in
  private-copy-production-MHCVoL, including mobile screenshots, private ownership,
  independent IDs and source revision provenance. The Workspace editor requires
  Verify repository before saving; earlier attempts omitted that step or blocked
  its read-only resolver POST. All copies created by those attempts were retired.
  The accepted run used the actual resolver and normal save action.
- The private Sandbox copy defaulted to warmup disabled, count one and keepalive
  disabled, with warmup unavailable while private. It was privately published
  and actually provisioned to a ready Workspace/Agent in 225,922 ms, with zero
  Agent prompts. Its Sandbox ID and provider runtime were absent from the source
  warm pool. The same five public warm IDs remained available afterward. The
  exact private test Sandbox and all temporary copies were retired through the
  public API; cleanup settled successfully. The source Profiles remain intact.
- One attempted concurrent fresh Trigger launch was correctly refused with
  sandbox_limit_reached while the two test Sandboxes and original owned Sandbox
  occupied the owner's quota. That refusal consumed no warm runtime. The accepted
  fresh launch above followed completed private-test cleanup; the quota was not
  increased or bypassed.
- Screenshot inspection caught an invalid mobile verification assumption: the
  first resize screenshot was captured during the grid-track transition, while
  overflow clipping kept document.scrollWidth within the viewport. The check now
  requires the whole composer form to intersect the viewport. The independent
  warm-mobile-layout-VREUAD production check covers a fresh 390px load and
  desktop-to-mobile resizing after settling; both show the intended 56px rail,
  334px content area and fully visible composer. The settled screenshot was
  inspected. No layout source change was necessary.
- These checks use the authenticated owner's account and GitHub's rendered
  README served at its source origin. They do not prove a second authenticated
  production account, a fresh browser sign-in or missing-service connection flow.
  Actual keepalive inactivity, periodic-boundary and manual-pause observations
  are recorded below. Consistent nonblocking snapshots remain unimplemented.

## Large snapshot observation and transfer cleanup — 2026-09-09

- warm-periodic-boundary-p609zv passed the full configured thirty-minute boundary:
  runtime binding was at 11:53:00 UTC, no capture was observed before 12:23:00,
  and the first capture was observed at 12:23:12. Epoch 1 published at 12:38:51.
  Runtime logs put local archive sealing at 12:31:39 with 5,934,511,069 compressed
  bytes. Capture plus publication therefore took approximately fifteen minutes
  forty seconds. This is interval acceptance and evidence of poor large-save
  performance, not writer noninterference acceptance.
- Manual pause of the separate completed welcome test reached Paused at
  12:29:49 UTC after admission around 12:24:45. The first verifier stopped on
  an incorrect strict-null assertion against an omitted optional runtime field.
  warm-pause-continuation-02Pgta continued the same admitted operation with zero
  commands: the authoritative record is Paused with no binding, no matching pod,
  a new snapshot, preserved transcript and Keep alive still enabled. Desktop and
  mobile pages hide workspace descendants and show Resume. The mobile screenshot
  was inspected; its late-loading service names/activity were not treated as
  complete detail-page acceptance.
- completed-warm-test-retirement-wo5wO8 retired only that test Sandbox after
  checking its main transcript and unused configured Agent. All 56 listed
  logical snapshot objects were absent afterward, including direct download
  404s. This does not audit raw storage versions or soft-deleted data.
- Keep alive observation warm-keepalive-2il0dy saw the Sandbox remain Active
  through 940 quiet seconds, beyond the 900-second inactivity policy. Its combined
  assertion failed because snapshot capture disabled manual pause. A subsequent
  read in warm-keepalive-e5C7TI found the Sandbox Reviving. Public lifecycle events
  show Active directly to Reviving, without an inactivity Pause. Kubernetes
  recorded an OOM kill on its node at 12:39:57, the old pod stopped at 12:39:59,
  and CMA began recovery at 12:40:17 from the newly published checkpoint. This
  does not pass continuous keepalive acceptance or a successful restore claim.
- Runtime source 090c2209 streams the compressed archive digest while writing,
  removing a full local archive reread. Capture and restore buffer filesystem I/O
  in 256 KiB blocks, and capture logs expose bounded, content-free byte counts
  and stage timing. Its local round-trip, corruption, archive-bound and replay
  checks passed. The initial image built successfully but was not deployed.
- Runtime source d730d6ae additionally releases the held local transfer archive
  after its writer token is retired and before processes resume. A successful
  restore releases its downloaded compressed transfer before Agents start.
  Cleanup failure preserves the held fence for an exact retry. Referenced remote
  recovery points and unrelated local objects remain intact. Production transport
  imports the exact remote bytes before every restore, including retries after a
  supervisor restart; that path is now explicit in the restart regression test.
  All 176 runnable local runtime tests and package Clippy pass; two explicit
  ignored tests remain skipped. The Linux/amd64 image build and fixed
  HIGH/CRITICAL vulnerability scan pass. The candidate digest is
  00bfd9b736be7fbfa986a41f250ceefcd5ee26ab6bcddae5cd3082488781ef4d;
  remote digest verification also passed. The source manifests now pin this
  candidate for the next runtime rollout; production verification remains pending.
  This reduces retained transfer
  storage, but does not prove that peak capture/extraction memory or
  uninterrupted snapshots are solved.
- The remaining old warm test was checked to contain its single interrupted
  test prompt and an untouched second Agent. Both original user histories still
  match their recorded hashes. Public retirement was admitted in
  old-warm-test-retirement-PWGzVL while its automatic recovery was in progress.
  Retirement completed, its runtime is gone, and all 178 listed logical snapshot
  objects were removed. Both temporary warm-test Sandboxes are now Deleted.
  No original user Sandbox was retired. Host disk has 16 GiB free.
- Runtime d730d6ae was deployed at 13:09 UTC through immutable manifest source
  d4f77b19. Evidence: snapshot-runtime-rollout-X9LNW4 and
  snapshot-runtime-postflight-DA1vzl under /tmp/cma-trureturing-loop.LvTezb.
  The existing API and web image digests remain unchanged; the API reloaded its
  configured runtime image and both deployments returned to 1/1 Ready, with no
  container restarts. The public landing page returned HTTP 200. The all-owner
  preflight found three Paused and 35 Retired Sandboxes, with no owned runtime.
  Postflight verified all 38 complete Sandbox records and child-summary hashes
  unchanged, including valid paused recovery points and passive superseded-key
  revocation debt. The owner's original one-turn and nine-turn histories still
  match their saved hashes. No owner Sandbox commands were sent.
- At 13:10:20 UTC, all five replacement warm runtimes were installed on digest
  00bfd9b736be7fbfa986a41f250ceefcd5ee26ab6bcddae5cd3082488781ef4d,
  with credential-free installation receipts, no fixed Git identity and no old
  warm runtime remaining. Truretruing availability remains warmup enabled,
  count five and Always keep alive enabled; the global checkpoint interval is
  still 1,800 seconds. The new deployment helper initially rejected unsupported
  owner-only transcript assumptions, existing passive paused-key debt and an
  omitted optional pagination cursor before any mutation; the corrected guard
  retains complete admin record hashes and owner-visible transcript hashes.
  It records point-in-time checks, not an atomic database writer lease.
- This rollout verifies image identity, installation and preservation. It does
  not yet verify actual production archive release after capture/restore, peak
  memory, improved large-save timing or uninterrupted snapshots. Those remain
  active implementation and acceptance work, together with clean continuous
  keepalive, fresh-sign-in/missing-service admission and the remaining register.

## Restore storage reuse and continued runtime acceptance — 2026-09-09

- fresh-warm-readme-LhjEWU passed the existing README Trigger on runtime
  d730d6ae. The Sandbox reached Active in 37,057 ms, Launch Ready in 98,093 ms,
  and its single welcome response completed in 272,672 ms. Replenishment started
  after 5,471 ms and five credential-free installed warm runtimes were verified.
  Per-user GitHub identity, desktop/mobile conversation views, reload and both
  original user histories passed. The named Trigger's original reuse policies
  were restored immediately after admission, in revision 13. This is the same
  authenticated account and GitHub-rendered README journey, not fresh sign-in.
- The new test Sandbox is retained for an independent quiet keepalive boundary
  and its first thirty-minute periodic save. Its ID is
  sbx_3b222900f13c48f89b04cd3a078e0a11, runtime bound at 13:13:09 UTC. Observers
  warm-keepalive-vLg3TD and warm-periodic-boundary-Ni07X8 are running; neither
  boundary is claimed complete yet. No original Sandbox is being mutated.
- Restore now reuses regular-file storage between its staged tree, live home
  and temporary rollback tree when the filesystem supports hard links. The
  supervisor keeps all home writers and mutations fenced until replacement and
  scratch cleanup finish, so no live write can change the retained source.
  The linked inode is checked against the opened regular file; special modes
  use a streamed copy without modifying the source. Cross-device, unsupported
  link and link-count limits retain the streamed copy fallback. Existing
  destinations and symlink sources remain refused.
- Four new tests prove inode reuse with exact data and mode preservation,
  original-inode rollback after a failed replacement, permission sanitization
  without source mutation, and collision/symlink refusal. All 180 runnable
  runtime tests and package Clippy pass; two existing explicit ignored tests
  remain skipped. Source scanning and formatting pass. The new restore change
  has not yet been built or deployed. It reduces restore duplication and does
  not implement a consistent live snapshot or prove peak memory safety.
- Follow-up cleanup makes supervisor-owned restore directories removable even
  when they inherit read-only modes. Only temporary directory modes change;
  hard-linked regular-file permissions and published home directory modes remain
  intact. Staging cleanup failure is now reported before restore succeeds, and
  a failure while clearing the old home also enters the existing rollback path.
  The new read-only-tree regression and all 181 runnable runtime tests pass;
  two existing ignored tests remain skipped. Package Clippy also passes.

## Current goal status and node-loss finding — 2026-09-09

- Quiet keepalive observation warm-keepalive-vLg3TD passed at 13:33:37 UTC:
  the test Sandbox remained Active after 948 quiet seconds, beyond the normal
  900-second inactivity threshold, with manual pause available and no capture
  in progress. This proves suppression of inactivity pause at that boundary;
  it does not prove continuous availability through a provider or node failure.
- Runtime source a70ab086 is built, scanned and uploaded as manifest
  sha256:d0fd7ffb930887c2ae8e5532f73c118d604ccdc13fa59045526ded8f33008cd3.
  All 21 actual Linux image checks pass, including archive removal after writer
  release, reimport, restore and staging cleanup. The 256 MiB restore benchmark
  runtime-restore-benchmark-lt43QO measured same-filesystem medians of 1,901 ms
  before and 1,648 ms after (13.3% faster); cross-filesystem fallback measured
  1,695 ms and 1,715 ms. These are local fixture results, not production latency
  or peak-memory acceptance. This image is not deployed; production remains on
  d730d6ae. Tooling commit 0f228780 changes the verification probe only.
- The fresh test's periodic observer warm-periodic-boundary-Ni07X8 failed before
  its first due capture at 13:43:09 UTC. Kubernetes records a node Shutdown and
  NodeNotReady event at 13:41:02 UTC, then node recovery at 13:42:53 UTC. The
  original runtime Pod UID was replaced. Public Sandbox events record Active
  to Failed at 13:41:08 UTC with runtime_lost and no completed checkpoint.
  No inactivity pause or capture was admitted. The immediate failure is a node
  shutdown; the underlying reason for that shutdown is not established here.
- Read-only evidence warm-node-loss-dzL3WP records the public lifecycle events,
  provider events and current state. The test Sandbox remains Failed, with no
  runtime binding or snapshot; its single completed conversation turn is still
  readable. Both original owner conversation hashes remain unchanged. The API
  and web deployments each remain 1/1 Ready and Available. No pause, retirement,
  rollout or other production mutation was issued during this investigation.
- The remaining snapshot work now also includes recovery when a runtime is lost
  before its first checkpoint. The failed test cannot be used for the planned
  pause/restore acceptance. Consistent nonblocking capture, raw storage version
  cleanup, missing-service/fresh-sign-in admission, positive contribution PRs
  and the outstanding UI acceptance cases remain unfinished. Do not mark the
  goal complete or describe the remaining work as deployment alone.
- Disk has 15 GiB free. The unused intermediate runtime-090c2209.oci.tar and one
  untagged converted Docker image were removed after checking they were unused.
  Both are reproducible build artifacts; no user files or production snapshots
  were deleted by that cleanup.

## Persistent storage capability proof — 2026-09-09

- This turn follows the node-loss finding with a bounded, credential-free storage
  experiment in a temporary namespace, cma-storage-check-20260909. It uses the
  exact a70ab086 Linux runtime image, production gVisor runtime class, UID 10001,
  disabled service-account token mounting, an XFS persistent disk and exclusive
  ReadWriteOncePod access. No CMA Sandbox record, owner conversation or live
  production runtime was modified by the experiment.
- File-level FICLONE still fails with ENOTTY on the real XFS persistent mount.
  Changing the disk filesystem alone therefore does not provide the required
  fast immutable source inside this gVisor runtime. Evidence:
  storage-capability-L1nIID/capability.json under /tmp/cma-trureturing-loop.LvTezb.
- A CSI disk snapshot completed in 74,816 ms for the initial fixture. The second
  snapshot published its creation timestamp while still not ready at 17,406 ms;
  it became ready at 67,444 ms. During that pending-copy interval, the runtime
  modified an existing file, deleted two files and created two directories with
  new content. The snapshot remained not ready after those operations finished.
  Evidence: storage-snapshot-concurrency-dczp3b/capture.json and mutation.json.
- A distinct PVC restored from that second snapshot contains the exact original
  file content and both subsequently deleted files, and excludes both subsequent
  directory creations. The original live volume contains the newer changes.
  The restored subdirectory is owned by UID 10001 and writable by a new non-root
  runtime without fsGroup, proving that an initialized subPath can retain its
  ownership without changing the shared provider's Pod template. This is a
  filesystem capability proof; it does not yet prove CMA conversation/metadata
  consistency, a universally safe capture barrier or uninterrupted app flows.
- The source test Pod was deleted with a UID precondition. A replacement Pod
  was required to run on a different node and mounted the same PVC UID/PV.
  Latest file content, new directories, prior deletions and further writes all
  passed in 22,832 ms. Evidence: storage-node-replacement-x1cikT/result.json and
  snapshot-restore.json. This is a controlled Pod replacement, not a second
  actual node shutdown or a production CMA Agent recovery.
- Cleanup storage-capability-cleanup-B6Kqxn verified the exact isolated targets
  and removed both completed test Pods, two PVCs/PVs, two disk snapshots and
  their contents, the snapshot class and namespace. CSI deletion finalizers
  completed. The GCP identity cannot directly read compute snapshots, so direct
  raw GCP inspection was unavailable; no broader IAM permission was requested.
  Synthetic test data is discarded; local verification evidence is retained.
- The adapter now has typed existing-volume references, rejects implicit
  provider allocation/deletion and malformed or overlapping mount paths before
  network access, and includes every mount field in the runtime request's
  fingerprint. Volume-free requests preserve their existing golden identity
  and wire shape. This is a prerequisite only: runtime rendering still supplies
  no volumes, and no production configuration or image was changed this turn.
  Source commit 9e5a1a1a passes all 127 adapter tests, all-target/all-feature
  Clippy, whole-workspace all-target/all-feature compilation, formatting,
  diff validation and the adapter source secret scan.
- Next implementation must durably allocate and own each Sandbox home, initialize
  credential-free directories, transfer warm ownership before credentials, retain
  the disk across runtime loss and release it through recorded lifecycle cleanup.
  Snapshot orchestration must then capture and export an immutable disk view
  while live operations continue, with conversation metadata from the same view,
  correct post-capture creation/deletion handling and bounded latest-only cleanup.
  No capture-duration writer fence may be carried forward as completion of the
  user's uninterrupted-save requirement. All retained acceptance remains active.
- Final production readback warm-node-loss-lcwaAs reconfirmed the original owner
  histories unchanged and the failed test conversation still readable. API/web
  remain 1/1 Ready and Available. The failed test is retained for the recovery
  work; no replacement success is claimed for it. Local disk remains 15 GiB free.

## Durable home allocation and lifecycle integration — 2026-09-09

- Layer 3 now implements namespaced, initialized persistent home allocation,
  exact storage-generation verification, paginated inventory and conditional
  release. Existing-volume mounts cover the home, supervisor journals and
  transfer scratch. Their immutable storage identity participates in the runtime
  request fingerprint; installation and credential delivery verify it again.
  Missing recorded storage never falls back to a newly allocated empty home.
  Runtime teardown alone does not release the disk. Source: 40fd84d0.
- The Sandbox worker records its allocation intent before storage I/O, then
  records the returned disk identity before runtime provisioning. Warm workers
  persist those same boundaries in separate claimed iterations. Warm admission
  transfers the home with its existing runtime in the admission transaction.
  Retirement releases storage after runtime teardown, and reconciliation reads
  warm authority before fresh owned authority to clean up late orphan allocations.
- Local fault tests prove recovery after an allocation committed but its response
  was lost. Another test holds allocation while user deletion completes, then
  lets the expired worker create its disk: its writeback conflicts and the next
  reconciliation removes that exact unowned disk without minting credentials.
- The first full regression run exposed excessive stack use after embedding the
  new receipt in every Sandbox/request value. Optional home records now use
  indirection, preserving their JSON representation while limiting async-frame
  growth. All 1,667 runnable backend tests subsequently pass with the ordinary
  test stack; 187 tests remain excluded/ignored by the default run. All-target,
  all-feature Clippy and documentation checks pass. Dependency policy and source
  secret scans pass. The unchanged file-size gate still reports 12 existing
  violations; no new source file exceeds its limit.
- Six real MongoDB replica-set warmup tests pass, including exclusive allocation,
  private-copy isolation, replay, stale-revision cleanup and atomic home transfer.
  The home-transfer test expires the readiness worker lease before admission,
  then verifies that worker cannot publish over the newly owned Sandbox. An
  initial test incorrectly expected an actively leased warm runtime to be
  claimable; the existing exclusion behaved correctly. Disposable database
  containers were removed and no persistent database volume was used.
- This is not deployed or enabled: production composition still has no home
  policy/store. A credential-free image-derived seed, source-backed permissions
  and validated deployment configuration remain necessary. Persistent native
  recovery must also be implemented: current restore bootstrap rejects an
  already installed journal, and runtime-loss handling still requires an archive.
  Do not activate mounts before those lifecycle paths and real Agent recovery
  are proved. Consistent nonblocking snapshot orchestration remains unfinished.
- Read-only production evidence warm-node-loss-XbcINm preserves both original
  owner history hashes. The retained failed test is still Failed with no runtime
  or snapshot, and its completed reply is still readable. API/web remain 1/1
  Ready. No production mutation or rollout occurred in this batch. Fifteen old
  Rust incremental-cache directories were removed after all compiler processes
  finished; they are reproducible caches, with source and compiled test binaries
  retained. Host free space returned from approximately 11 to 14 GiB.

## Persistent runtime status and restore completion — 2026-09-09

- Added an internal read-only runtime status request bound to the exact Sandbox,
  provision generation and persistent home fingerprint. The adapter verifies the
  original disk before and after the request. Every supervisor start gets a new
  boot identity, including a process replacement behind the same provider handle.
  Status exposes installation/credential readiness, pending restore and live
  Agent bridge count; it contains no credentials or home contents. Observation
  bypasses the operation gate and does not repair installation or mutation journals.
- Mounted runtimes establish a bounded, durable home binding outside the home
  tree before installation. An existing installation without that binding is
  refused. Missing mounted homes are not recreated empty; missing, foreign,
  corrupt, oversized or symlinked binding records cannot authorize credential
  delivery. Binding publication uses a synced temporary file and atomic
  publication, and incomplete unpublished writes are discarded on startup.
- Persistent archive restore records Started before replacement and Completed
  only after durable home replacement and scratch cleanup. Restart recognizes a
  completed inherited installation without extracting the old archive again.
  Exact restore replay acknowledges completion and discards duplicate imported
  scratch while preserving later edits, deletions and newly created directories.
  Credentials remain process-local and must be delivered again after restart.
  Unmounted runtimes retain the existing archive/bootstrap behavior.
- Nine runtime regressions and one adapter regression were added. Two initial
  assertions used a Workspace operation ID that violated the existing exact
  identity contract; correcting the test request exercised real post-restart
  Workspace creation successfully. A targeted run passed with one transient
  nextest leaky-process observation. The subsequent complete backend run passed
  all 1,677 runnable tests, with no leak reported; 187 remain excluded/ignored.
  All-target/all-feature Clippy, documentation checks (zero doctests), dependency
  policy, source secret scanning and configuration-key checks pass. The file-size gate still has its same 12
  existing violations, with no newly oversized source.
- Further inspection disproved the proposed CloseAgent-then-reopen shortcut:
  runtime mutation authority permanently retires an Agent process identity on
  close, and its original open receipt also cannot replay without a live bridge.
  Persistent recovery needs explicit new runtime/Agent process authority while
  preserving filesystem mutation receipts and deletion tombstones. Do not weaken
  cleanup dominance or treat this status protocol as complete Agent recovery.
  Control-plane recovery-source selection, process-generation fencing, real
  native conversation reopening, initialized seed/configuration/permissions,
  image build and production activation remain unfinished. Nonblocking snapshots
  and all remaining retained production acceptance are still active.
- Read-only production evidence warm-node-loss-NO0rJS again preserves both
  original owner transcript hashes. The retained test remains Failed revision
  16, with no runtime or checkpoint and its completed reply still readable.
  API and web remain 1/1 Ready. This batch made no production mutation or rollout.
  Host free disk space is approximately 12 GiB; no additional cleanup was needed.

## Persistent Agent process generations — 2026-09-09

- Persistent home binding format 2 records the highest admitted runtime
  generation. Startup advances it before journal recovery or credentials;
  startup and subsequent mutations from an older generation are refused.
  The detached Workspace dispatcher rechecks this authority after taking its
  operation gate. Production has not enabled persistent homes, so there is no
  deployed format-1 home binding to migrate.
- Agent Open/Close receipts and their mutation authority now belong to the
  admitted runtime generation. Filesystem mutation receipts, Worktree/Agent-home
  deletion tombstones and Workspace journals remain shared across generations.
  Cleanup dominance within one process generation remains unchanged. Records
  carry their process generation as well as their storage path; copied records
  and symlinked generation directories cannot authorize a replacement process.
- Six new regressions use real shell child processes and Git Worktrees. They
  prove re-opening after normal close and abrupt process exit in a new runtime,
  refusal of same-generation duplicate opens and stale closes, preservation of
  filesystem receipts/deletion tombstones, and refusal of older home authority,
  copied journals and aliased directories. They are process lifecycle evidence,
  not native Codex conversation-resume or production acceptance.
- The first targeted parallel run passed 15 tests with two nextest leaky-process
  observations. A serial investigation passed all 15 without leakage, followed
  by the complete runtime suite: 196 runnable tests passed, two existing ignored
  tests skipped, no leak reported. All-target/all-feature runtime Clippy,
  formatting and source secret scanning pass. The next step is the pinned Linux
  image and native conversation verification, then durable control-plane source
  selection and production activation. Nonblocking snapshot work remains active.
- Runtime cb0c99d9 subsequently completed its exact-source Linux/amd64 build at
  manifest sha256:1aa2d73c547af3cf0ad2ad985ed4ef8504d496fde8fc43691494fb9230976391.
  Native Docker loading and source/platform identity verification passed, followed
  by the fixed HIGH/CRITICAL image vulnerability scan and all 21 image contract
  checks. The contract campaign removed its disposable container and volume.
  Evidence: runtime-cb0c99d9-build.json, runtime-cb0c99d9-archive.json,
  runtime-cb0c99d9-trivy.json and runtime-cb0c99d9-contract.json under the retained
  scratch directory. These checks do not prove native conversation reopening.
- The status readback still finds API and web each 1/1 Ready at the previously
  deployed digests. This candidate has not been pushed or deployed, and no
  production mutation was issued. Native conversation recovery, control-plane
  integration and consistent nonblocking snapshots remain unfinished. Host disk
  had approximately 11 GiB free before local image loading and scanning; two
  earlier targeted Docker cache pruning attempts reclaimed zero bytes.

## Native conversation reopening and process adoption — 2026-09-10

- Local campaign native-home-recovery-am2zjZ uses the exact cb0c99d9 Linux image,
  the pinned Codex 0.147.0 and ACP bridge 1.4.0, real Workspace/Worktree creation,
  and three retained Docker volumes. It completes an actual native conversation
  turn, writes a Worktree file, then forcibly kills the entire container without
  a close receipt or snapshot. A same-generation replacement refuses reopening
  with agent_process_not_live. A generation-2 replacement recognizes completed
  installation, receives credentials again and loads the original conversation.
- The native session ID remains 01a086db-d725-7121-9b15-777d0f031ae4. Its original
  transcript byte prefix retains SHA-256
  b2448e29e475739c9712664063c4491ebc35e5e136b5b2150afa2122085a63f8,
  the Worktree file is unchanged, and ACP replays the original conversation.
  A second prompt completes with the first prompt and reply in its actual native
  model request. Exactly two user-message records exist afterward; no initial
  prompt was replayed. The response service is a local synthetic fixture inside
  the network-disabled container. This proves native kernel persistence and
  reopening, not production model access, the public Trigger or node recovery.
  All three disposable containers and volumes were removed; only evidence remains.
- The first campaign attempt PPWIbG used a hyphen in a Workspace ID, which the
  existing runtime identity contract refuses. Correcting that fixture to use
  native_workspace produced the passing complete campaign above; the application
  contract was unchanged. Both attempts cleaned their exact local resources.
- RuntimeBinding now optionally persists the supervisor boot identity. Owned,
  warm and replacement provisioning read the exact Sandbox/generation/home-bound
  status before publishing that binding. Generic Sandbox persistence refuses
  overwriting or clearing a known boot ID on the same provider runtime. Older
  volume-free bindings retain their serialized representation and adoption path.
- Worker attachment checks the recorded process before continuing runtime work.
  Continuous reconciliation also probes process identity and required readiness;
  provider Running alone cannot mask a replaced process. Failed status reads do
  not erase bindings or reclaim resources. Persistent warm checks retire a
  restarted or credentialed runtime instead of republishing it as available.
- Seven new core/worker regressions cover durable identity, foreign observations,
  compatibility, process replacement before credentials, control-plane adoption,
  unavailable status reads and contaminated/restarted warm environments. The full
  backend suite passes 1,690 runnable tests, with 187 existing excluded/ignored.
  All-target/all-feature Clippy, documentation checks (zero doctests), dependency
  policy and source secret scanning pass. The file-size gate retains the same 12 existing violations; no newly
  oversized source was introduced.
- This is still incomplete and not deployed. Reconciliation now detects a process
  replacement, but durable runtime-loss recovery still selects archive checkpoints.
  Native-home source selection, preservation of pause/rotation and child-operation
  intent, exclusive old-runtime removal, mounted archive migration and production
  storage/configuration remain required before activation. Consistent nonblocking
  snapshots and all remaining retained acceptance are still active.
- Read-only production observation warm-node-loss-uNAulp again preserves both
  original owner conversation hashes and the failed test's completed reply. The
  retained test remains Failed revision 16 with no runtime or checkpoint. API and
  web each remain 1/1 Ready. No production mutation occurred in this batch. Host
  disk reached approximately 9.4 GiB free during compilation; no user data or
  production resources were deleted.
- After compilation, two obsolete local OCI archives, runtime-1727bb8d.oci.tar
  and runtime-d052e746.oci.tar, were removed. Their regular-file identity, exact
  build/manifest digest and retained Git source commits were checked first.
  They total 811,852,288 bytes and are reproducible build artifacts; their
  metadata/evidence, current candidate archive and deployed runtime archive remain.

## Original Sandbox restore hotfix — 2026-09-10

- Runtime source e8db57e7 falls back to verified streaming copy when successful
  hard links report incompatible device/inode identity on the production
  filesystem. Five checkpoint-copy unit tests pass. The initial local image
  accidentally contained a host ARM64 executable and failed with Exec format
  error. It was superseded by a verified Linux x86-64 binary and runtime digest
  9e0281826070abd4e3b65e053737f12184d1adbc08cb8d9582d933069ce1a983.
  The API ConfigMap and Deployment template now use this corrected runtime pin.
- The original Sandbox had already entered terminal restore_failed. A narrowly
  scoped operator repair, guarded by revision 915 and the exact failure class,
  returned it to Paused, cleared the failed runtime binding, and synchronized
  its aggregate and query fields. A local preimage was retained with mode 0600.
  An initial repair timestamp/denormalized-field mismatch briefly caused reads
  to return 503; it was corrected and the public API confirmed Paused revision
  916 with revive allowed before verification began. No Workspace, Agent,
  conversation, transcript or checkpoint record was deleted or replaced.
- Named README Launch lch_b301a148c8474e98a5851795c9fb6e47 subsequently reached
  Ready with the original Sandbox, Workspace, Agent and Worktree. The original
  two transcript hashes remained unchanged; first prompt replay was absent.
  Desktop/mobile restored history, enabled chat and no horizontal overflow
  assertions passed. Home restore took approximately 225 seconds; Sandbox
  activation occurred 377 seconds after revival began. Source evidence is
  current-readme-repeat-YYmNn8 in the retained scratch directory.
- The full harness result remains failed: its Live activity advancement check
  sampled the restore screen just before the next poll and saw the same last
  entry. Subsequent API inspection confirms events advanced from sequence 679
  through 697, including runtime preparation, Agent readiness and activation.
  Continuous activity visibility needs a synchronized browser observation;
  the post-revival snapshot timing stage was not reached by this harness.
  Nonblocking consistent snapshots and all other open acceptance items remain.
- Readback found API/web, the original replacement runtime and five pool
  runtimes Running/Ready. The temporary Mongo diagnostic Pod was removed. Host
  disk has approximately 52 GiB free.

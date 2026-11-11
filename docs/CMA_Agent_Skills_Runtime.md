# Live Agent skill installation and upgrades

The authenticated `GET /api/v1/profile-authoring/skills` catalog supplies Ornn
skill IDs and exact versions. The worker independently resolves the selection
with the Sandbox's delivered, owner-bound credential. An owner may install or
upgrade one skill on an idle, ready Agent without replacing its process,
conversation, home, immutable profile, or original resolved profile material.

## Admission and observation

`POST /api/v1/agents/{id}/skills` accepts `{"id":"ornn-skill-guid","version":"1.1"}`
and requires `Idempotency-Key`. A successful HTTP 202 records admission; poll
`GET /api/v1/agents/{id}/skills` for runtime confirmation. Versions must be exact,
not catalog aliases. There are at most 32 distinct profile/live skill IDs and
128 retained live installation attempts per Agent. Upgrades do not consume an
additional skill slot. Baseline skill identities cannot be replaced here.

GET returns one row per skill ID with its effective installed version. During
an upgrade the previous row remains `installed` and top-level `pending` is
true. After success that row changes to the confirmed new version. After a
failed upgrade the previous version remains `installed`, `pending` becomes
false, and the row's `error` identifies the failed target and failure code.
Failed first installations remain `failed`. Clients must check `pending`, the
requested version, and `error`; an old row's `installed` status alone does not
confirm an upgrade.

```json
{
  "items": [{
    "id": "ornn-skill-guid",
    "name": "Research",
    "version": "1.1",
    "source": "ornn",
    "status": "installed"
  }],
  "pending": false,
  "can_install": true
}
```

`name`, `error`, and `unavailable_reason` are optional. `can_install` requires an
idle ready Agent, an eligible parent Sandbox, available history capacity, and
read-only observation of the exact bound runtime advertising installation.
At the distinct-ID limit, a supported upgrade must remain possible. Retained
runtimes can support adding skills while lacking replacement support;
`can_install: true` does not by itself promise upgrades. Pending skill work
blocks further mutations and conversation input until it settles.

Named refusals distinguish the causes:

| Result | Meaning |
| --- | --- |
| 409 `agent_skill_upgrade_disabled` | New upgrades are closed by Agent Manager policy; retry with a new key after activation. |
| 409 `agent_skill_history_full` | The Agent has 128 retained attempts; existing skills/conversation remain usable. No history is truncated. Further installations require another Agent. |
| 409 `agent_not_ready` | The Agent or parent is busy/not ready; no skill operation was admitted. |
| 409 `agent_skill_install_unsupported` | The exact runtime does not report support for installation. |
| 409 `agent_skill_version_upgrade_unsupported` | The exact runtime does not report support for replacement; the detail names the installed/requested versions. |
| 503 `instance_service_unavailable` | Bounded admission CAS retries could not commit; contention does not prove the Agent is busy. |
| Failed upgrade, `runtime_skill_upgrade_conflict` | A receipt, parent, unsafe entry or recovery snapshot conflicts with the expected installation. Preserve evidence and resolve the named conflict before retrying. |
| Failed upgrade, `agent_skill_upgrade_modified` | An archive-owned path is missing or its type/bytes changed. Save edits outside the skill directory and restore the installed archive's files before a new attempt. |
| Failed upgrade, `agent_skill_upgrade_previous_unavailable` | The checksum-pinned previous archive is absent from the warm cache and cannot be fetched (including deletion/unsharing). Restore access or the verified cache, then make a new attempt. |
| Failed upgrade, `agent_skill_upgrade_retention_full` | Retention would exceed 32 trees or 512 MiB. Archive and remove settled retained trees outside discovery; never remove a pending operation's evidence. |

The global idempotency contract is unchanged. All completed outcomes, including
refusals, are retained and replayed for **24 hours**. Reuse the unchanged body
and key after an uncertain reply. A definitive busy/not-ready/unsupported
refusal is non-admission for that key: after recovery, make a new attempt with
a **new** key. The web client clears the retained intent on a definitive 409
and generates a fresh key for the next user action; uncertain replies retain
the original key. A Manager crash before the global outcome is recorded may
return `idempotency_recovery_required`; this route adds no separate recovery
ledger or request-key field. Historical cached false-busy responses expire
under the same retention policy.

## Durable model and worker

`Agent.work.live_skills` is append-only installation history. An upgrade adds a
pending entry whose optional `previous` value is the exact effective
`ResolvedAgentSkill`. Old profile material and prior installed/failed entries
remain immutable. The latest confirmed installation overrides the profile's
effective version; failed attempts never do. Validation requires `previous` to
match that confirmed history, permits only the last entry to be pending, and
counts distinct IDs for quota. Store admission atomically fences the Agent
revision, conversation sequence, and exact active parent.

The 128-entry admission bound keeps this hot-read history below a conservative
512 KiB BSON budget: each bounded reference, two material records, failure code
and field overhead fit within 4 KiB. No archive bytes or URLs enter this history.
That leaves most of Mongo's 16 MiB document limit to the Agent's other fields;
it is not a substitute for their own bounds. Validation's backwards history
comparisons are bounded by 128 entries. The capacity regression exercises maximum
version/name lengths, refusal, and unchanged existing history. Legacy additive
homes have at most 32 entries and remain valid.

The existing Agent worker lease serializes work. Before runtime mutation it
resolves and durably pins the target's source, ID, exact version, name, and archive
SHA-256. Retries reconstruct the same request from this material and the retained
previous evidence without another catalog lookup. Download locators are not
content identity. Only a correlated runtime receipt publishes success. Resolution
or terminal replacement failure preserves the previous effective version and
keeps the failed entry. An owner may submit another upgrade attempt after it
settles. Failed additive installations retain their original operation identity
and cannot be silently reinterpreted as a new installation.

Stop, lease takeover, retained-home revival, and parent cleanup use the existing
worker fencing. Revival reattaches the recorded conversation and reschedules
pending skill work before queued input is delivered. A stale worker cannot pin
new material or publish confirmation after its lease is superseded.

## Fenced runtime replacement

Additive installation retains `agent:{agent}:skill:{id}:install:v1`. Each upgrade
uses `agent:{agent}:skill:{id}:upgrade:{sequence}:v1`, where sequence is its
one-based immutable history position. The request hash binds that sequence,
target material, previous material and previous receipt. Repeated targets and
retries after failure therefore cannot collide with an older installation.
Per-skill authority advances only with a newer sequence and fence. Old operation
replays cannot regain mutation authority. Agent-home removal dominates all
later skill requests and recovery, including already-completed receipts.

The replacement operation follows these durable steps:

1. Persist fenced runtime admission. Use the checksum-verified `warm-skills/`
   archive cache first for both versions; fetch only cache misses through the
   existing credential proxy. A missing/unfetchable previous version is a terminal
   named refusal, so the Agent is not kept busy indefinitely. Cache corruption
   retains the existing checksum/cache-invalid refusal; it never authorizes using
   unverified bytes. After proof publication, recovery needs neither archive.
2. Extract the previous archive and compare **every archive-owned relative path,
   entry type and regular-file byte** with the installed tree. Do not compare any
   POSIX permission bits, including the top directory, execute/setgid/group/other
   bits, or ownership/timestamps. Inode and change metadata are used only to detect
   races during a read, not as installation identity. New files use the existing
   extraction modes. A missing or modified archive-owned path is refused unchanged.
3. Extra regular files/directories, including Python `__pycache__/*.pyc`, are
   allowed and retained with the entire old tree. They are not copied into the
   new version. Symlinks, special entries, hard links, unreadable content, over
   8,192 entries, more than 64 directory levels, or over 256 MiB are refused. Live installations also require the
   exact previous `.cma-install-receipt.json`; profile skills must lack that marker.
   Neither a marker alone nor a marker supplied by an archive proves the tree.
4. Stage under the same Agent home's `.cma-skill-upgrades/{request-sha256}/`,
   outside `skills/` and provider discovery. Check retained trees plus incoming
   old bytes against 32 trees / 512 MiB, reserving receipt space. No automatic
   age/count eviction deletes owner data; exceeding either admission bound yields
   `agent_skill_upgrade_retention_full`. Temporary extraction adds at most two
   archive expansion limits. Owner writes after admission remain owner-controlled.
5. Place the new receipt inside the staged `new/` tree. Sync staging, then publish
   runtime-state `skill-upgrades/{request-sha256}.json` with exact old/new content
   snapshots (including extra old files, excluding the separately checked receipt
   and all modes). Repair ambiguous proof-publication sync before exchange.
6. Recheck both snapshots and parents, then atomically exchange staged `new/`
   and `skills/{id}` with descriptor-relative Linux `renameat2(RENAME_EXCHANGE)`
   or macOS `renameatx_np(RENAME_SWAP)`. Linux invokes the raw syscall. The target
   is always a complete old or new tree. The supervisor gate serializes mutations.
7. Sync both affected parents. Remove only the extracted comparison archive
   `previous/`; retain the exchanged old installation at `new/` and the small
   proof. Publish the journal receipt. Recovery recognizes the new receipt/content
   after exchange and repeats the sync/cleanup without redownloading. Once proof
   exists, a conflicting tree keeps the admitted operation pending with
   `agent_skill_upgrade_recovery_required`; a terminal failure could otherwise
   expose the old version after publication. Preserve evidence for reconciliation. Replaced
   files, modes, caches and owner extras survive as the original inodes.

The owner may archive and remove a **settled** operation's retained directory to
free retention capacity after its result is confirmed. Keep runtime journal and
proof files; never remove a pending directory. Agent-home deletion removes the
retained trees under the existing explicit lifecycle authority. Preservation is
chosen over silently classifying/deleting files by a cache-like name: a user may
put valuable data under `__pycache__` too. Modified archive files are refused
because their bytes no longer prove the expected version; the refusal names the
owner action instead of overwriting those changes.

### Historical mode compatibility

The extraction/materialization history was inspected with `git log -p`:
`bb6f20baf` introduced ZIP file modes masked to `0777` (default `0644`) and
umask-created directories; `069fd663e` changed durability and applied the same
modes through open file descriptors; `48adbe78c` added cache-backed preparation;
`38550a154` retained those extraction rules. Live installs from `45ca94327` use
the same preparation/extraction path; `cca1c5b44` changes atomic publication's
Linux syscall, not normalization. There is no historical directory-mode identity
contract to rely on.

The [recorded remount failure](CMA_Native_Production_Verification.md#combined-performance-and-full-home-release--pr-1016)
changed `0600` to `0660` and `0750` to `0770` on the same PVC. `f9b7dec50`
introduced `OnRootMismatch` for new pools and `2af7b713e` tightened selection to
the exact policy. Existing pool/template/home records retain their original
policy; #1019's fresh-home proof does not cover every retained home. No production
access was used for this assessment. Tests materialize profile skills through
`agent_home::materialize`, live skills through `agent_skill_install::install`,
import a Python helper to create actual bytecode, change group/directory modes,
and upgrade both. Crash recovery also succeeds after group-bit changes.

## Skill upgrade reader floor

| Surface | Compatibility rule |
| --- | --- |
| `AgentSkillInstallation.previous` | Absent/`None` is omitted. Ordinary additive records remain byte-for-byte predecessor-readable. The **first admitted upgrade** writes this field and raises the Agent Manager reader floor, even if it later fails or completes. Historical entries must remain readable. |
| Runtime `AgentSkillInstallRequest.upgrade` | Absent is omitted and the old install hash/identity is unchanged. The first upgrade authority/journal write raises the retained runtime-state reader floor to the replacement implementation. |
| Runtime `skill-upgrades/` proofs | Written only for upgrade work; retain with the runtime's persistent state. Never remove evidence to enable an older reader. |
| Runtime `agent_skill_upgrade` status | False is omitted. True is returned only when a standalone status request explicitly accepts `application/vnd.chrono.agent-skill-upgrade-status.v1+json`. Embedded and unnegotiated status retain the predecessor shape. |

**Promotion ordering and rollback floor:** deploy this release's compatible
Agent Manager readers/workers and drain predecessors **before** promoting a
runtime advertising `agent_skill_upgrade`. From that promotion onward, an Agent
Manager rollback below this release is not permitted, even with admission closed.
Older strict status readers reject an unknown advertised capability and cannot
be an approved recovery set for these live Sandboxes. Negotiated legacy responses
help rolling coexistence; they do not waive this operational rollback floor.

The first admitted upgrade additionally raises the Mongo reader floor; the first
runtime authority/journal write containing `upgrade` raises the persistent-home
reader floor. Older runtime binaries have strict nested request decoders and
cannot recover those records. These floors survive failure, completion and writer
closure. Retain the recorded runtime/home/conversation and latest runtime default;
never erase evidence, downgrade a home or interrupt a live Sandbox to fit an old
binary. Runtime adoption remains subject to the existing owner/idle update policy.

## Reader floor and rollback record: 2026-11-11-am-agent-skill-upgrade

| Required field | Evidence |
| --- | --- |
| Release / migration identity, phase and owners | `2026-11-11-am-agent-skill-upgrade`, issue #1209; expand-only source change, activation pending. Agent Manager owns Mongo/admission; Layer 4 owns home/journal formats; release operator owns promotion/drain verification. Candidate source and executable checksums must be attached to the release receipt before activation; no published image is claimed here. |
| Affected stores and all writer triggers | `Agent.work.live_skills.previous` on explicit idle owner POST; Agent worker pin/settlement and recovery; global command replies; runtime upgrade requests nested in V1 JSON authority/journal and indexed ledger payloads; retirement import/archive preserve those exact payloads; runtime proof and retained-tree directories. Startup does not backfill or admit upgrades. GET reads and runtime status observation do not write upgrade intents. |
| Old/new format and preservation contract | Optional `previous`/`upgrade` omitted for additive records; existing envelope versions unchanged. New true `agent_skill_upgrade` status field negotiated on explicit requests. No index/TTL/validator/key/client-journal changes. Original profile, resolved material, IDs, history, request keys, fences, PVC and conversations retained. |
| Compatible reader floor / allowed recovery artifacts | Agent Manager and runtime implementations in the final #1209 source, including bounded history, gate, content/retention proofs and typed failure settlement. Runtime floor covers strict `AgentSkillInstallRequest.upgrade` inside JSON/indexed journals, authority, import and retirement archive readers—not only Mongo. No predecessor executable is certified. Exact candidate/recovery image digests and cross-image drills remain **unverified**, so no production recovery artifact is approved by this local record. |
| Writer gate and cohort | `CHRONO_AGENT_SKILL_UPGRADE_ENABLED=false` by default in parser and API ConfigMap. Closed new version admission returns `agent_skill_upgrade_disabled`; ordinary additive work/read/exact accepted key replay continue. Open admission still requires owner authority, idle state and exact runtime support. Core gate covers non-REST callers; workers only settle retained admitted work after closure. Serving/recovery processes must share the compatible floor and incompatible workers must drain before the operator activates a bounded cohort. Drain/cohort receipts pending. |
| Backfill execution | None: no transform or startup migration. Existing homes are proved on explicit upgrade. Current executor is the Agent worker with durable lease/fence and per-record CAS, then the serialized runtime mutation gate/journal. No separate manual migrator, cross-store transaction or global migration lock is claimed. Prerequisites are compatible Manager deployment/drain, runtime promotion proof and release qualification receipts. |
| Backup and dry-run | No bulk rewrite/deletion/backfill requiring a migration backup. Per-request exchange preserves the previous tree outside discovery. Recovery unit is Agent Mongo history/commands plus the same home, journals/proofs/retained trees and exact runtime. Production backup/restore artifacts and recovery rehearsal are not claimed; do not restore stale Mongo over later accepted home effects. |
| Rollback / forward recovery | Close the writer flag in a compatible Manager and let admitted work settle. From runtime capability promotion, retain this Manager floor; from first journal write, retain this runtime reader/recovery floor. Keep latest runtime default and existing live runtime/home/conversation; use compatible forward repair, not a pre-release Manager/runtime rollback. No operation unsets fields or deletes receipts to lower the floor. Abort activation if capability, drain, retention, reader or continuity checks fail. |
| Verification and completion | Local gates and source identities are recorded in the issue handoff; old-shape/new-reader and strict-predecessor negative fixtures, gate/replay/settlement, real materializers, remount/cache/extra-data policy, crash boundaries, fencing and retirement preservation are covered. Pinned-provider proof checks the next latest catalog, same process/conversation and new disk bytes. Historical/candidate image matrix, production cohort observations, recovery drills and reviewer/operator sign-off remain unexecuted. This is not production migration completion. |

## Binding version-only PUT assessment

Keep `binding_skill_version_update_unsupported` for this release. Binding PUT
currently compares a pinned profile digest and publishes binding configuration;
Agent skill POST independently admits asynchronous work and only later confirms
the effective version. Reusing it directly in PUT could publish a desired binding
version before filesystem success, race channel turns/reconfiguration, or leave
several requested skills partially upgraded. The binding's immutable profile
must not be rewritten as though all runtime effects had already succeeded.

A future implementation can retain the existing Agent/conversation, but needs a
durable binding update intent fenced by binding revision and Agent identity,
a barrier against conflicting channel work, retained per-skill target/previous
receipts, and explicit partial-failure/recovery semantics for multiple skills.
Publish confirmed binding state only after all correlated receipts; use the
existing global idempotency contract. Gateway channel configuration and Manager
binding state need reconciliation across their separate stores. That is a separate
workflow, not just calling the current POST inside PUT.

Today the channel owner who has authority over the bound Agent can select that
Agent in Agent Manager and POST each exact version while idle, using a new key
for each attempt, then poll GET for confirmation. This changes the effective
live installation and retains the binding/profile/conversation; it does not change
the binding's declared template for future Agents. Organization-owned bindings
still require the applicable Agent-management grant; a channel-management role
alone is not permission to bypass Agent authorization. Do not retire/recreate the
conversation to apply the skill version.

## Root cause and proof

Before `c9e2d7b91`, changing the version of a profile-resolved skill returned
`AgentOperationError::Conflict` before the idle check
(`677c27440:api/crates/layer2-core/src/sandbox/agent_live_skills.rs:172–182`).
Layer 1 mapped that to `agent_operation_in_flight`. The live-installed comparison
had the same defect. Develop replaced it with the named unsupported refusal.
Runtime execution was not reached by that original request. The remaining
ready-child/parent-publication mismatch and exhausted admission CAS retries
also no longer make false busy claims.

The image build's existing `live_skill_install.test.cjs` runs with pinned Codex
0.147.0 and the exact patched ACP bridge. A loopback model fixture requires no
provider credentials. It verifies the same native child PID, bridge and
conversation across replacement, the next request's latest `<skills_instructions>`
catalog containing only the new description while the old tree remains outside
discovery, every old occurrence preceding
that catalog in immutable history, and exact new files on disk. The image test
runs on Debian/glibc, where the Python exchange helper's `renameat2` is exported;
macOS uses `renamex_np`. The test list is unchanged.

Rust regressions cover admission, busy refusal/new attempts, key replay,
predecessor serialization, material pinning, sequential profile/live upgrades,
lost replies, lease takeover, atomic exchange recovery boundaries, destination
conflicts, parent symlinks, journal replay, and home-removal dominance. Local
native proof and macOS filesystem tests do not claim a Linux runtime-image or
production rollout qualification. Production acceptance still needs evidence
bound to the candidate source/image and environment.

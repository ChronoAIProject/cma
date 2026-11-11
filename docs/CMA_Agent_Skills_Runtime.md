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
1,024 retained live installation attempts per Agent. Upgrades do not consume an
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
| 409 `agent_not_ready` | The Agent or parent is busy/not ready; no skill operation was admitted. |
| 409 `agent_skill_install_unsupported` | The exact runtime does not report support for installation. |
| 409 `agent_skill_version_upgrade_unsupported` | The exact runtime does not report support for replacement; the detail names the installed/requested versions. |
| 503 `instance_service_unavailable` | Bounded admission CAS retries could not commit; contention does not prove the Agent is busy. |
| Failed upgrade, `runtime_skill_upgrade_conflict` | The runtime could not prove the expected previous destination or its receipt/content. |

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

1. Persist fenced runtime admission. Stage the pinned previous and target
   archives using existing checksum, size, path and ZIP extraction checks.
2. Compare the current destination with the extracted previous archive: all
   relative paths, entry types, file bytes and permissions must match. Symlinks,
   special entries, hard links and extra/missing files are refused.
3. For a live-installed previous version, require its exact
   `.cma-install-receipt.json` as well as the full tree comparison. For a profile
   skill, require no runtime install marker and prove the full tree against the
   original pinned archive. This also supports homes created before upgrades
   existed; an unavailable previous archive leaves the old installation intact
   until proof can be obtained. A reserved marker supplied by an archive is
   refused.
4. Place the new receipt inside the complete staged target. Sync the staged
   tree, then durably publish a small proof under runtime state `skill-upgrades/`
   binding the request hash and the previous/new tree hashes. Repair an ambiguous
   proof-publication sync before exchanging directories.
5. Recheck previous content, staged content and parents, then atomically exchange
   the staged target and `skills/{id}` using descriptor-relative Linux
   `renameat2(RENAME_EXCHANGE)` or macOS `renameatx_np(RENAME_SWAP)`. The Linux
   implementation invokes the raw syscall. The destination is always a complete
   old or complete new directory. The supervisor operation gate serializes
   filesystem mutations.
6. Sync both affected parents, remove the old staged tree, and publish the runtime
   journal receipt. The small proof remains as recovery evidence. If a crash
   occurs after exchange, the target receipt and full new-tree hash identify
   publication; recovery syncs and cleans up without fetching either archive.

No marker alone proves an unmodified installation. Unrelated skills, baseline
links, instructions, credentials, and dirty work are preserved. Owner changes
that prevent exact comparison receive a typed conflict instead of being
silently overwritten.

## Skill upgrade reader floor

| Surface | Compatibility rule |
| --- | --- |
| `AgentSkillInstallation.previous` | Absent/`None` is omitted. Ordinary additive records remain byte-for-byte predecessor-readable. The **first admitted upgrade** writes this field and raises the Agent Manager reader floor, even if it later fails or completes. Historical entries must remain readable. |
| Runtime `AgentSkillInstallRequest.upgrade` | Absent is omitted and the old install hash/identity is unchanged. The first upgrade authority/journal write raises the retained runtime-state reader floor to the replacement implementation. |
| Runtime `skill-upgrades/` proofs | Written only for upgrade work; retain with the runtime's persistent state. Never remove evidence to enable an older reader. |
| Runtime `agent_skill_upgrade` status | False is omitted. True is returned only when a standalone status request explicitly accepts `application/vnd.chrono.agent-skill-upgrade-status.v1+json`. Embedded and unnegotiated status retain the predecessor shape. |

The minimum complete Agent Manager implementation is `afe28c28c`; the runtime
replacement implementation is `7b0df83c9`. Recovery builds must contain these
implementations or compatible successors.

Deploy the compatible Agent Manager first and drain predecessor workers before
selecting the new runtime. The runtime advertises replacement only after it
implements the operation and passes the pinned-provider proof. Old retained
runtimes omit the capability and are refused up front. Publishing a new image
never replaces a live Sandbox merely to enable upgrades; use the existing
owner-authorized/idle managed-runtime update policy.

Older Managers reject unknown status fields. Negotiation prevents that failure:
an old Manager does not send the new media type, so even the newer runtime sends
its original status shape. Before any upgrade is admitted, an API rollback can
therefore retain the newer runtime without this field breaking live Sandboxes.
After an upgrade intent is written, **any API rollback/recovery image must retain
the `previous` reader, history validation and replacement worker from this
release**. A pre-upgrade API is not an approved rollback target after that write.
A safe rollback keeps those readers and the newer runtime/default, and can roll
back the web or disable new upgrade admission in a compatible forward repair.
Never downgrade a live Sandbox, erase history, or restore a database snapshot to
make an incompatible API read it. This floor applies to failed attempts too.

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
catalog containing only the new description, every old occurrence preceding
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

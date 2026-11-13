# Lifecycle, skill upgrade and Always on acceptance runbook

Operator procedure for the release that carries #1164 (retained conversation
reopen deadlines), #1209 (idle in-place skill upgrade) and #1330
(administrator-only Always on). It is written for the release lead and SRE who
hold staging and production authority. The authors of the change have not
executed any step below against a deployed environment; every result must be
recorded as new evidence.

Each evidence record names the candidate API, web and runtime image digests,
the source SHA, environment, time window, sample count N, configuration and
flags (including `CHRONO_AGENT_SKILL_UPGRADE_ENABLED` and the platform
inactivity settings revision) and where the raw requests, responses and logs
are kept. A cancelled run, a run that passed only after a retry, or a run
against a different digest is a failure, not a pass.

## 1. Rollout order and rollback floors

1. Deploy Agent Manager and web from the candidate with
   `CHRONO_AGENT_SKILL_UPGRADE_ENABLED=false`, automatic pause unchanged and no
   Always on writes. Wait until every predecessor API pod has drained.
2. #1330 raises the **authorization** rollback floor: every API image used for
   rollback after this deployment must enforce administrator-only Always on,
   because a predecessor API would reopen owner self-approval. Its data floor
   is unchanged. See [CMA automatic pause](CMA_Auto_Pause.md).
3. Promote the runtime image from this release through the normal explicit
   `CHRONO_SANDBOX_IMAGE` promotion, only after step 1. Existing Sandboxes adopt
   it through the owner's update command or the 02:00 Asia/Singapore managed
   update; never force a rotation. Once any runtime advertising
   `agent_skill_upgrade` is live, do not roll Agent Manager back below this
   release. See the
   [skill upgrade reader floor](CMA_Deployment_Readiness.md#skill-upgrade-reader-floor).
4. Set `CHRONO_AGENT_SKILL_UPGRADE_ENABLED=true` in staging for TC-057, and in
   production only after TC-057 passes. Closing it again stops new upgrades;
   already admitted upgrades still settle and stay readable.
5. #1164 changes only the control plane. It needs no runtime promotion and
   applies to the next reopen after step 1.

## 2. #1330 before automatic pause is enabled

Follow "Existing exceptions and pre-enable review" and "TC-130 staging
verification" in [CMA automatic pause](CMA_Auto_Pause.md) exactly, first in
staging and then in production. In summary:

1. Read every page of `GET /api/v1/admin/sandboxes?always_on=true&limit=100`.
2. Re-approve or revoke each retained `true` through
   `POST /api/v1/admin/sandboxes/{id}/always-on`, then match each `configure`
   activity in `GET /api/v1/admin/activity?action=configure`.
3. Set `inactivity.auto_pause_enabled=true` and `pause_after_seconds=900` with
   `PATCH /api/v1/admin/settings`, then wait the full 900-second grace.
4. TC-130: an owner `true` returns 403 `always_on_approval_required` with no
   write; an approved Sandbox stays running; an unapproved idle Sandbox pauses
   after 15 minutes; a revoked exception pauses; an owner can relinquish.

## 3. #1164 TC-047 and TC-048 lifecycle continuity

Use a staging Sandbox owned by a test identity, with two Agents in distinct
Worktrees and long retained conversations (at least 100 turns each; include an
Agent whose history takes more than 30 seconds to load if one is available).
Ask each Agent to create `~/cycle-fixture/<agent>.bin` (1 MiB of random bytes)
and to leave an uncommitted Git change in its Worktree.

Record a baseline before the first cycle:

- Sandbox and Agent IDs from `GET /api/v1/sandboxes/{id}` and
  `GET /api/v1/agents/{id}`.
- Native conversation IDs, read-only from Mongo:
  `db.agents.find({_id: {$in: [...]}}, {"record.conversation.conversation_id": 1})`.
- The native Claim, Sandbox and PVC UIDs, read with the same `kubectl` identity
  checks used in [native production verification](CMA_Native_Production_Verification.md).
- SHA-256 of each fixture file from
  `GET /api/v1/agents/{id}/files/download?path=...`. Compare content only and
  record modes separately; fsGroup remounts can change group permission bits.
- Each Agent's transcript length from `GET /api/v1/agents/{id}/transcript`.

Run 100 cycles. In cycle i:

1. `POST /api/v1/sandboxes/{id}/pause` with a fresh `Idempotency-Key`; poll
   `GET /api/v1/sandboxes/{id}` until `paused` and record accept-to-paused time.
2. For odd i, `POST /api/v1/sandboxes/{id}/revive`. For even i, send one
   message to the first Agent with `POST /api/v2/agents/{agent_id}/responses`
   (message-driven revival). Poll until `active` with both Agents ready; record
   accept-to-ready time and the phase timeline from
   `GET /api/v1/sandboxes/{id}/events`.
3. Check that the Sandbox, Agent and native conversation IDs, the Claim,
   Sandbox and PVC UIDs, fixture hashes and Git dirty state are unchanged, that
   every message sent in the cycle has exactly one terminal reply, and that the
   transcripts grew only by those turns.
4. Any operation not finished within 120 seconds must show a typed state:
   `reviving` with its recorded lifecycle phase, or a typed failure. Record it.

Pass: 100 of 100 cycles keep every identity and hash, no message is lost or
duplicated, pause p95 is at most 15 seconds and resume p95 at most 30 seconds.
Report the measured latencies; do not claim five-second recovery.

TC-048 needs `kubectl exec` authority on the staging Sandbox Pod:

- Recoverable case: during a revival, once the runtime Pod is Ready, stop the
  first Agent's bridge with `kill -STOP` on its `codex-acp` process. The Sandbox
  stays `reviving` in `ConversationReopen`; after the five-minute idle window
  the attempt fails as retryable. Send `kill -CONT`; the next attempt must
  rejoin the same conversation and reach `active` with every identity and hash
  unchanged.
- Terminal case: keep the bridge stopped until the lifecycle reports its typed
  failure (`failed` with a recorded failure class). As the owner, call
  `DELETE /api/v1/sandboxes/{id}` once; expect 202 and eventually `retired`
  without a second DELETE. Check that the original Claim, Sandbox and PVC UIDs
  and every owned Pod are gone and that no lifecycle lease remains.
- Owner recovery before retirement: where the deployment has opened
  `CHRONO_RETAINED_REOPEN_ENABLED`, a failed Sandbox can instead be assessed
  with `GET /api/v1/sandboxes/{id}/runtime-recovery` and, with the owner's
  consent, recovered on the same image and home through the matching POST. Follow
  [same-image retained recovery](CMA_Retained_Recovery.md); a refusal leaves
  retirement available.

Acceptance decision for the release lead: a silent reopen now waits up to five
minutes, renewed by durable output, before it is classified as a retryable
failure. During that wait the public state is the typed `reviving` phase, not
an error. If the gate requires a failure classification within 120 seconds,
record that this design does not meet it literally and decide whether the
typed in-progress state satisfies TC-047 and TC-048.

The failed incident Sandbox `sbx_f5e89e635c2948dcb53adbc4a0f90367`, if it still
exists, can be retired by its owner with the same `DELETE`.

## 4. #1209 TC-057 idle skill upgrade

Preconditions: section 1 steps 1 to 4 are complete in staging, and the target
Agent runs the promoted runtime (`GET /api/v1/agents/{id}/skills` reports
`can_install: true`). Use an Ornn skill with two published versions, installed
once from an Agent profile and once through the live install endpoint.

For each of 10 idle upgrades (alternating versions if needed):

1. `POST /api/v1/agents/{id}/skills` with `{"id": <skill>, "version": <target>}`
   and a fresh `Idempotency-Key`; expect 202 and a pending row.
2. Repeat the identical request with the same key; expect the identical
   response and no second admission (one pending `SkillInstallationChanged`
   log entry).
3. Poll `GET /api/v1/agents/{id}/skills` until the single row for that skill
   shows the target version `installed`. Download `skills/<id>/SKILL.md` with
   the files API and compare it with the target archive. Send one prompt that
   names the skill and confirm the reply follows the new version.

Busy path: start a long turn, then request an upgrade; expect 409
`agent_not_ready`, never `agent_operation_in_flight`. After the turn ends, a new
key succeeds. With the flag `false`, a version change returns 409
`agent_skill_upgrade_disabled`; on a runtime without the capability it returns
`agent_skill_version_upgrade_unsupported`. A failed attempt names its reason in
the row (for example `agent_skill_upgrade_modified` or
`agent_skill_upgrade_previous_unavailable`) and leaves the Agent idle.

Pass: 10 of 10 upgrades succeed, no `agent_operation_in_flight`, and no
half-installed skill or wrong version.

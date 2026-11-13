# CMA automatic pause

CMA automatic pause is a deployment-wide operator policy for reclaiming idle
Sandbox runtimes while retaining each Sandbox home, identity and history. It is
disabled by default in code for controlled rollout. The
[#1330 decision](https://github.com/ChronoAIProject/cma/issues/1330) of
2026-11-09 requires it enabled for all public users before launch, with the
existing 900-second (15-minute) idle threshold. Always on is an
administrator-approved exception.

## Policy and grace period

Operations > Platform settings stores `inactivity.auto_pause_enabled`, the
existing `inactivity.pause_after_seconds` threshold (60 through 86,400), and an
additive `inactivity.grace_started_at` timestamp. Saving an enablement change or
threshold change sets the timestamp to the server's current RFC3339 time. The
reconciler waits for one complete threshold after that timestamp. Restarts and
unrelated settings changes do not reset the grace period. Disabling the policy
clears the timestamp and prevents new automatic pause decisions.

The existing 15-second reconciler still performs the final fail-closed proof.
It fences live Agents and the mutable policy in a transaction before scheduling
the durable pause lifecycle. Missing or unavailable evidence leaves a Sandbox
running. The lifecycle preserves the retained home and resumes on the next
admitted message or explicit resume.

The proof includes nonterminal durable command receipts from UI, API, channel,
and trigger sources, even when an Agent checkpoint has not caught up yet. An
open desktop stream registers against its Sandbox for the lifetime of the
connection and also blocks the proof; the marker is removed when the stream
closes. These activity blockers are additive and content-free.

`always_keep_alive` was historically a profile or binding availability setting,
and lifecycle-created Sandboxes used it by default. While automatic pause is
off, that behavior remains unchanged. When an administrator enables automatic
pause, the old availability value is not silently copied to every new policy
record: a Sandbox pauses unless an administrator approves its per-Sandbox
`always_on` exception. Retained true values are also honored until reviewed
below. This makes existing running Sandboxes eligible after the grace period
without rewriting their immutable profile. A legacy Sandbox with no preference
therefore behaves as `always_on = false`; setting `always_on = true` creates an
additive preference on that Sandbox only. Existing readers default a missing
preference to `None`.

## Controls and audit surface

Owners may relinquish an exception through
`POST /api/v1/sandboxes/{id}/always-on` with
`{ "always_on": false, "expected_revision": number }` and an `Idempotency-Key`.
Layer 2 refuses `true` with HTTP **403**, code
**`always_on_approval_required`**, problem type
`https://spec.chrono-ai.fun/cma/1.0/problems/always-on-approval-required`, and
detail “Always on requires approval by an administrator. Owners may only turn it
off.” The refusal applies even when already on or the supplied revision is
stale; ownership/capability checks still run first (non-owner 404, missing
capability 403). Nothing is written to the Sandbox by this refusal.

Owner `false` retains revision checks and lifecycle restrictions. If the
preference is unset or already false, it returns the current Sandbox unchanged,
including revision and timestamps, without persisting a preference. If true, it
stores false and advances the revision once. Exact idempotent replays return the
original result; use a new key only for a new command.

Administrators approve (`true`), re-approve (`true` again), or revoke (`false`)
through `POST /api/v1/admin/sandboxes/{id}/always-on` with
`{ "always_on": boolean, "expected_revision": number }`. Layer 2 requires
`ControlAnySandbox`; other callers receive HTTP 403 `forbidden`. Each accepted
write advances the Sandbox revision and records this activity through the
existing `ActivityRecord` facility (values in `detail` are strings):

```json
{
  "id": "<activity id>",
  "at": "<RFC3339 time>",
  "actor": "<administrator subject>",
  "action": "configure",
  "target": { "kind": "resource", "id": "sandbox:<Sandbox id>" },
  "outcome": { "status": "succeeded" },
  "detail": { "setting": "always_on", "status": "true", "revision": "2" }
}
```

Revocation records `status: "false"`. Stale revisions return HTTP 409
`operator_conflict` without a change or success activity. The existing lifecycle
guard also returns 409 for Retiring/Retired Sandboxes; their historical
preference is not rewritten. As with `SandboxOperatorService::retire_sandbox`
and platform-settings audit, the activity append follows the successful store
write. An append failure returns HTTP 503 `operator_service_unavailable`, but
the Sandbox write may already have landed. After any uncertain result, read the
Sandbox and activity log, then re-approve or revoke with the current revision to
establish a confirmed audited result. Do not interpret absence of an activity as
proof the write failed.

Enumerate exceptions with
`GET /api/v1/admin/sandboxes?always_on=true&limit=100`. This requires
`ReadAllSandboxes`. Follow `next` using the same filter and `cursor`; combine
optional owner/status/runtime filters only if intentionally narrowing the
inventory. Omitted status includes all states. `always_on=false` includes both
false and missing preferences. The list has no snapshot guarantee during
concurrent changes; repeat the review after stopping approval/revocation changes
for its final pass. Read each admin detail for current revision and pinned
resources.

Mongo applies the `record.profile.always_on` filter to the existing
`_id`-ordered page query. The existing limit caps the returned page at 200
records (plus one lookahead), before the existing Agent-count lookup. No new
index or startup DDL is added: startup DDL extraction is tracked in
[CMA startup DDL inventory](CMA_Startup_DDL_Inventory.md) (#1328/#1329,
M-OPS-02).

The page limit bounds returned records and lookups, not documents examined. In
the worst case Mongo scans in `_id` order until the page fills or the remaining
collection is exhausted. Sparse exceptions can therefore make even an empty page
expensive. This admin-only enumeration is acceptable for a rare operator audit;
do not poll it or use it on a hot path. Follow cursors sequentially, keep the
filters stable, and monitor Mongo checkout latency during the pre-enable audit.
No inventory scan or exception count is added to auto-pause preview.

Admins can also inspect idle candidates through
`GET /api/v1/admin/auto-pause/preview` and confirm an immediate scan with
`POST /api/v1/admin/auto-pause/pause-idle-now` and `{ "confirmed": true }`.
That action bypasses grace, but rechecks activity, policy fences and Always on
before every durable pause admission.

The owner Overview shows the stored state and administrator approval
requirement, with only a turn-off action for an enabled exception. The admin
detail keeps its switch. A stored true value alone cannot prove who approved it.
API responses include `always_on` when a preference has been explicitly set.

## Existing exceptions and pre-enable review

Do not rewrite existing records silently. Historical owner-written true values
remain honored; there is no persisted approval marker and this release adds
none. Before staging enablement, and before the separately authorized production
rollout, confirm that every serving API replica enforces administrator approval
and older owner-enable writers have drained. Rollback images must enforce this
rule too: compatibility with the persisted fields alone does not preserve the
authorization boundary. Then complete the review below.

1. Read every page of `GET /api/v1/admin/sandboxes?always_on=true&limit=100`.
   Save IDs, owners and current lifecycle states. Read
   `GET /api/v1/admin/sandboxes/{id}` for revision and CPU/memory requirements.
   These are read-only inventory requests (normal read/list audit still
   applies).
2. Record Retiring/Retired rows separately as retirement/history; continue
   counting any still-allocated resources until cleanup is complete. Do not
   resurrect or rewrite those records to force approval. Review each remaining
   exception's business approval and capacity. Through the admin POST above,
   re-approve true or revoke false with the freshly read revision. A 409
   requires re-reading and reviewing the newer state; do not retry an old
   decision blindly. Re-approval intentionally advances revision even if already
   true.
3. Verify each successful mutation in `GET /api/v1/admin/activity` with
   `action=configure`, `actor=<administrator-subject>` and `limit=100`,
   following its cursor. Match target, value and returned revision. Save the
   activity IDs with the inventory; do not infer approval from the old value
   alone.
4. Repeat the true inventory and list those exceptions separately in the
   capacity plan, including their CPU/memory, retained home/PVC and cost.
   Complete this review before the global policy makes the ordinary idle fleet
   eligible to pause.

## TC-130 staging verification

Use only isolated staging identities and Sandboxes. Record candidate image
digest, source SHA, environment, time window, sample count, settings/revisions
and raw request/response/activity locations; these instructions are not
execution evidence.

1. Complete the existing-exception review. Read `GET /api/v1/admin/settings`,
   then `PATCH /api/v1/admin/settings` with the current settings revision and
   `{"expected_revision": R, "inactivity": {"auto_pause_enabled": true,
   "pause_after_seconds": 900}}`. Verify the response has true, 900 and a server
   `grace_started_at`; wait the full 900-second grace before timing pause
   behavior.
2. Prepare two idle Active Sandboxes owned by a public test user, A and B, with
   no active/queued work or desktop streams. Read their current revisions
   through `GET /api/v1/sandboxes/{id}`. As that owner, POST A's owner
   always-on route with `{"always_on": true, "expected_revision": R}` and a
   fresh `Idempotency-Key`. Expect 403 `always_on_approval_required` and the
   exact type/detail above. GET A again: preference, revision and updated
   timestamp must be unchanged. Repeat the same request/key and check the same
   refusal. Non-owner requests remain 404; a non-admin request to the admin
   route must return 403 `forbidden`.
3. As admin, POST A's admin always-on route with true and its current revision.
   Expect 200, `always_on: true` and revision R+1. Verify the `configure`
   activity with actor, `sandbox:A`, setting `always_on`, status `true`,
   revision R+1. Verify A appears in the true admin list. As owner, send true
   again with a fresh key: expect the same typed 403 even though A is already
   enabled.
4. Leave A and B idle past 900 seconds from their last activity and past
   enablement grace; allow the 15-second reconciler scan plus lifecycle
   completion. A must remain Active with no inactivity pause event. B must enter
   Pausing then Paused, retaining its IDs/home/history. Observe events at
   `GET /api/v1/admin/sandboxes/{id}/events` and the detail state. An active
   turn control Sandbox must remain running throughout.
5. Re-read A and POST false through its admin route with that revision. Expect
   200, false and revision+1; verify the corresponding `configure` activity with
   `status: "false"` and that A leaves the true list. Revocation does not reset
   the last-activity clock: if already idle for 900 seconds it can pause on the
   next scan; otherwise wait the remaining threshold. Observe Pausing then
   Paused and the inactivity notice/events. No pause-idle-now shortcut qualifies
   this timing.
6. Re-approve a fresh Active test Sandbox and let its owner POST false with the
   correct revision/key; expect 200 false and one revision advance. Owner false
   on a separate unset Sandbox must leave `always_on` absent, revision and
   timestamp unchanged. A stale revision on false returns 409
   `sandbox_invalid_state`. Replay a successful owner disable using its
   original key/body: the original success returns without another revision
   advance. Restore reviewed staging settings only through the administrator
   settings route.

## Compatibility and rollout

Release record `2026-11-11-am-always-on-authorization` follows
[CMA Data Migrations](CMA_Data_Migrations.md). This records #1330's delta;
other changes in a release batch retain their own compatibility requirements.

| Required field | #1330 release record |
| --- | --- |
| Identity, phase and owners | #1330; Agent Manager service owner and release operator. Authorization change only; no format expansion or contraction. Source is this issue's reviewed commit series. |
| Stores and writer triggers | Existing Sandbox CAS commands and administrator activity append. No new worker, startup, import or recovery writer. |
| Format and preservation | No new persisted shape, index, validator or TTL. Activity rows reuse `configure`, the `resource` target, and `setting`/`status`/`revision` detail keys. Existing IDs, revisions, leases and retained history are preserved. |
| Compatible data-reader floor | Unchanged; existing activity and Sandbox readers accept these shapes. No previous release-batch floor is lowered. Deployment image/digest receipts remain required. |
| Writer gate and cohort | Data-writer gate: not applicable, because no new format is emitted. `ControlAnySandbox` is the authorization boundary, not a migration feature flag. |
| Backfill | None; no migration command, transformation or automatic rewrite. Legacy exception review uses the existing CAS commands above. |
| Backup and dry-run | Not affected: no destructive transform or deletion requiring a migration backup. Existing retention and recovery requirements remain. |
| Rollback / forward recovery | The **authorization rollback floor rises to this release**, although the data floor is unchanged. A predecessor API would reopen owner self-approval. All serving and rollback images must enforce administrator approval; preserve retained data and runtime homes. |
| Verification and completion | Core, REST, memory/Mongo and client contracts cover the change. Reader/filter review is below. Live artifact identities, TC-130 evidence and release-operator sign-off must be recorded at deployment; they are not implied by local tests. |

The reader/filter audit covers core `ActivityRecord`/`ActivityDetail`, the
Mongo `ActivityDoc` reader and actor/action filters, and the REST activity
projection. They already accept these keys and string values: `always_on`,
`true`/`false`, and decimal revisions. Both SDK activity decoders accept
`configure`, `resource` and string-valued detail maps. The admin Activity UI
exposes the `configure` filter and renders resource targets; it does not display
detail values. Its settings-only `SettingsAudit` deliberately selects
`platform_settings`/`triggers` targets, so it ignores Sandbox rows rather than
treating them as settings changes. No activity reader, decoder or filter needs
expansion for these rows.

The following notes describe #1341's existing storage boundary; #1330 does not
add a new data-reader requirement.

The persisted fields added by #1341 were audited against predecessor
`d50bbe72d`. New readers accept old records with defaults. Old readers require
neutral values to be omitted; merely defaulting them on read is insufficient.

| Persisted field / record | Neutral write and predecessor reader |
| --- | --- |
| `platform_settings.settings.inactivity.auto_pause_enabled` | `false` is omitted. The predecessor's `InactivitySettings` denies unknown fields. `true` must remain stored. |
| `platform_settings.settings.inactivity.grace_started_at` | `None` is omitted. The settings service clears it while disabled, including threshold edits; enabled policies retain the timestamp. The predecessor rejects any stored timestamp. |
| `SandboxProfileSnapshot.always_on` | `None` is omitted everywhere snapshots are serialized, including Sandbox aggregates, retained warm plans and admissions. Both `Some(true)` and `Some(false)` persist: an explicit preference differs from absence. The predecessor snapshot reader denies unknown fields. |
| `sandboxes.desktop_activity_fence` | Absent until a desktop lease operation increments it. This is outer query metadata, outside the typed `record`; the predecessor reads named metadata fields and ignores this counter. No neutral counter is initialized. |
| `sandbox_sessions` (`_id`, `sandbox_id`, `lease_expires_at`) | New collection and expiry index; the predecessor never reads it. Desktop activity may create these records while auto-pause is off. |

`MongoPlatformSettingsStore` replaces the complete `settings` value using a
literal in its update pipeline. Consequently a disabled save omits both new
keys and removes any previously stored grace/enablement keys; a separate
`$unset` stage is unnecessary. Sandbox creation and replacement likewise
serialize the complete profile snapshot, preserving omission of `None`.
The profile-store extraction changes no draft/revision schema. The new
`SandboxAgentActivity` flags and `SandboxInactivityPauseRequest.auto_pause_enabled`
are transient evidence/request fields, not persisted records. Pause lifecycle
records retain their existing shape.

Deploy with auto-pause off and leave Always on preferences unset. Under those
conditions ordinary settings and Sandbox saves remain readable by the
predecessor at the data level; it is still excluded by the authorization
rollback floor above. API responses still expose
the explicit policy boolean through their existing response projection.
No billing collections, runtime image pins, homes or conversation records are
changed by this compatibility correction.

Enabling auto-pause or writing an explicit Always on preference (including
admin `false` on an unset Sandbox) retains the data-floor restriction introduced
by #1341. Wait until incompatible predecessor pods have drained before either
action. Turning auto-pause off clears its settings
keys, but does not remove explicit Sandbox preferences or establish that all
retained records are predecessor-compatible. Retain a recovery image that reads
these formats and enforces #1330 administrator approval; do not discard
preference/history records to force an older image to read them. There is no
additional data-writer feature gate.

For production verification, the release lead should confirm that the settings
document's key shape is unchanged after an ordinary save with auto-pause off,
and that old pods remain healthy during rolling overlap, without typed-reader
errors. Keep auto-pause off and Always on unset throughout this release's
verification. This checklist is not evidence of an executed production write.

After separate release approval and the exception review above, enable the
900-second threshold and verify the full grace period, an idle existing Sandbox,
an active turn, queued work, an Always on Sandbox, explicit and message resume,
preview, and the confirmed immediate action. Observe decision logs alongside API
latency and Mongo checkout health.

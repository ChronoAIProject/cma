# CMA automatic pause

CMA automatic pause is a deployment-wide operator policy for reclaiming idle
Sandbox runtimes while retaining each Sandbox home, identity and history.
It is disabled by default. A deployment can therefore ship this code without
changing runtime behavior.

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
record: a Sandbox pauses unless its owner explicitly sets the per-Sandbox
`always_on` preference. This makes existing running Sandboxes eligible after the
grace period without rewriting their immutable profile. A legacy Sandbox with
no preference therefore behaves as `always_on = false`; setting `always_on = true`
creates an additive preference on that Sandbox only. Existing readers default a
missing preference to `None`.

## Controls and audit surface

Owners can set `POST /api/v1/sandboxes/{id}/always-on` with
`{ "always_on": boolean, "expected_revision": number }`. The operation is
owner-scoped and revision checked. Admins can inspect and override the value
through `POST /api/v1/admin/sandboxes/{id}/always-on` with the same body, and
can inspect candidates through
`GET /api/v1/admin/auto-pause/preview` and can confirm an immediate scan with
`POST /api/v1/admin/auto-pause/pause-idle-now` and `{ "confirmed": true }`.
The immediate action bypasses enablement grace, but it still rechecks activity,
policy fences and Always on before every durable pause admission.

The settings UI exposes the global toggle and hours/minutes/seconds threshold,
plus the preview and confirmation action. The owner Sandbox Overview exposes
Always on only to the owner. API responses include the additive `always_on`
field when a preference has been explicitly set.

## Compatibility and rollout

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
predecessor during rolling overlap and rollback. API responses still expose
the explicit policy boolean through their existing response projection.
No billing collections, runtime image pins, homes or conversation records are
changed by this compatibility correction.

Enabling auto-pause or writing an explicit Always on preference (including
`false`) raises the rollback floor to this release. Wait until predecessor pods
have drained before either action. Turning auto-pause off clears its settings
keys, but does not remove explicit Sandbox preferences or establish that all
retained records are predecessor-compatible. Retain a recovery image from this
release or later after these writes; do not discard preference/history records
to force an older image to read them. There is no additional feature gate.

For production verification, the release lead should confirm that the settings
document's key shape is unchanged after an ordinary save with auto-pause off,
and that old pods remain healthy during rolling overlap, without typed-reader
errors. Keep auto-pause off and Always on unset throughout this release's
verification. This checklist is not evidence of an executed production write.

After separate owner approval, enable a reviewed threshold and verify the full
grace period, an idle existing Sandbox, an active turn, queued work, an Always
on Sandbox, explicit and message resume, preview, and the confirmed immediate
action. Observe decision logs alongside API latency and Mongo checkout health.

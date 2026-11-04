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

The settings document and Sandbox aggregate use additive fields with defaults;
older records remain readable. The new SDK fields are optional on the TypeScript
reader so clients can preserve unknown fields while a rolling deployment is in
progress. No billing collections, runtime image pins, homes or conversation
records are changed by this feature.

Deploy with the setting off. During production verification, read the settings
revision and maintenance inventory, confirm no automatic decisions occur while
off, then enable a reviewed threshold. Verify the full grace period, an idle
existing Sandbox, an active turn, queued work, an Always on Sandbox, explicit and
message resume, preview, and the confirmed immediate action. Observe decision
logs and metrics alongside API latency and Mongo checkout health. The feature
stays off until the owner explicitly enables it.

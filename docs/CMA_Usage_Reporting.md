# Measured usage visibility contract

Related: #1374, #1332, #1333, #1334, #1352 and #1373.

## Setting and UI rule

`usage_reporting.measured_visible: boolean`, default `false`. Admin label: **Show measured usage**. The ops dashboard and account usage popup show allocated usage by default and render measured usage only when this value is exactly `true`. This controls presentation only. Collection remains independently governed by `CHRONO_USAGE_METRICS_ENABLED`; neither that environment switch nor the #1352 usage report contract changes. Turning visibility on does not promise collected data exists.

## Admin read and writes

All existing `/api/v1/admin/settings` authorization, revision CAS and activity auditing remain in force (`configure_platform` required; non-admin writes return 403).

`GET /api/v1/admin/settings` returns `{ "revision": <integer>, "settings": { ...existing sections, "usage_reporting": { "measured_visible": false } } }`. The section and boolean are always explicit in new-server responses, including defaults.

`PUT /api/v1/admin/settings` accepts `{ "expected_revision": <GET revision>, "settings": { ...complete settings from GET, "usage_reporting": { "measured_visible": true } } }`. It returns the same full response shape with an incremented revision. A missing usage_reporting section in a legacy full replacement defaults to false.

Recommended toggle write: `PATCH /api/v1/admin/settings` with `{ "expected_revision": <GET revision>, "usage_reporting": { "measured_visible": true } }`; set false to hide. Omitted sections remain unchanged. Stale revision returns 409 `settings_conflict`; invalid/null section, non-boolean or unknown keys return 422 `invalid_request`. On 409 refetch before allowing a new save. `POST /api/v1/admin/settings/reset` with `{ "expected_revision": N }` resets visibility to false along with existing defaults.

## Read-only exposure for every authenticated user

The existing `GET /auth/session` success body gains `"usage_reporting": { "measured_visible": false }` alongside `authenticated`, `subject`, `capabilities`, `visitor` and profile fields. Owner, admin and visitor identities all receive the effective saved value. A fresh session read sees current persisted settings (no process-lifetime cache). Anonymous responses stay 401 `{ "authenticated": false }` plus the `"silent_sign_in": false` by default (true only after `CHRONO_SILENT_SIGN_IN_ENABLED` activation) capability described in [API_Overview.md](API_Overview.md); they carry no usage policy.

If configured settings storage cannot be read, the endpoint returns 503 `settings_unavailable`; the UI must keep measured usage hidden when the session/setting is missing or fails. Older servers omit usage_reporting, so consumers must treat absence as false. Refresh the session after a successful admin toggle; other browsers observe changes on their next session refresh.

## SDK names

TypeScript: `client.auth.readSession()` returns `AuthSession`; after `authenticated` narrowing use `session.usage_reporting?.measured_visible === true`. `client.admin.readSettings()`, `replaceSettings(...)`, `patchSettings(...)`, `resetSettings(...)` remain the methods. `PlatformSettings` and `PatchSettingsRequest` gain optional `usage_reporting: { measured_visible: boolean }` for compatibility with older servers and existing clients.

Rust: `client.auth().read_session()` returns `AuthSession::SignedIn(Identity)`, whose optional `usage_reporting: Option<UsageReporting>` contains `measured_visible: bool`. `UsageReporting` lives in `cma_sdk::auth`. Existing `client.admin().read_settings()`, `replace_settings(...)`, `patch_settings(...)`, `reset_settings(...)` remain. `cma_sdk::admin::UsageReportingSettings` is the admin section type; `PlatformSettings.usage_reporting` and `PatchSettingsRequest.usage_reporting` use `Option<Section<UsageReportingSettings>>`.

## Persistence and rollback

Persist no `usage_reporting` key at all while measured_visible is false, including after turning it off or resetting. This preserves the strict d50bbe72d predecessor settings shape. Persist `{ "usage_reporting": { "measured_visible": true } }` only after explicit enablement. True round-trips with the new reader; the strict predecessor cannot read that opt-in shape. Disable and verify the key is absent before restoring that predecessor. No production mutations are part of this work.

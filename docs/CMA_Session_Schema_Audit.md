# Desktop session schema incident and writer audit

Issue: #1390. Incident source: `b31a88603`. CMA source audit baseline: `8d2c8c109`.

## Root cause and trigger

PR #1341, commit `86898e99ab`, introduced the insert at
`api/crates/cma/src/store/sandbox_sessions.rs:24–28` without
`document_schema_version`. An owner opening
`GET /api/v1/agents/{id}/desktop/ws` calls `DesktopService::open`
(`layer1-public/src/rest/desktop.rs:101`,
`layer2-core/src/sandbox/desktop.rs:177`), which records a 90-second desktop
lease after connecting to the runtime. Desktop heartbeat renews that lease;
normal disconnect deletes it. A retained row makes the next startup fail:
`MongoSandboxStore::open_with_storage` includes `sandbox_sessions` in the
collections checked by `store/schema.rs`, whose original predicate rejects
missing versions. Billing admission and revival are not the inserting paths.

## Compatibility boundary

New desktop inserts and renewals persist the existing integer version 1.
Renewal refuses explicit non-v1 or non-integer versions instead of overwriting
them. No new schema version or required payload field is introduced.

Startup recognizes only the exact historical session document: three string
fields `_id`, `sandbox_id`, and `lease_expires_at`, with the same nonempty ID,
128-byte session ID limit, and RFC3339 timestamp checks as the original writer.
Expired leases are also valid historical records. Extra fields, absent payload
fields, incorrect types, invalid timestamps, and explicit unsupported versions
are refused. Missing versions in other collections remain refused because this
incident supplies no evidence that their historical shapes are v1.

The guard streams matching rows and emits one WARN with an aggregate count,
without document contents, identifiers, timestamps, or inherited span fields.
It does not backfill or otherwise mutate rows. New rows remain readable by
`b31a88603`; its unchanged startup guard still requires the operator's targeted
version repair for historical unversioned rows before an old-binary restart.

## Writer audit

Paths below are relative to `api/crates/cma/src/`. The review followed every
production insert, insert-many, replacement, SDK upsert, and raw Mongo update
command to its encoder, and compared the result with the store's startup guard.
Updates without upsert and deletes cannot create an unversioned row. Test
fixtures deliberately containing invalid schemas are excluded.

| Collections / family | Version source and insert/upsert path | Result |
| --- | --- | --- |
| `sandbox_sessions` | `store/sandbox_sessions.rs`, open and renew | Fixed missing insert version; renewal stamps retained leases and fences explicit unknown versions. |
| Sandboxes, Workspaces, Worktrees, Agents | `sandboxes`, `workspaces.rs`, `worktrees.rs`, `agents.rs`; admission, lifecycle, recovery and transaction writers reuse their document encoders | Existing v1 encoders include the version. |
| Agent logs, commands, command orders, authorization audits, Sandbox controls and events | `agent_log.rs`, `agent_commands/{records,reserve,audit}.rs`, `sandbox_controls/records.rs`, `sandbox_events.rs`; queued edits/cancellations and all transactional append paths reuse these encoders | Existing v1 writes include the version. |
| Agent reuse, first messages, access tools/requests, provenance, suggestions, media uploads | `agent_admission_reuse.rs`, `trigger_first_messages.rs`, `agent_access_tools/records.rs`, `agent_access_requests/records.rs`, `agent_provenance.rs`, `suggestions.rs`, `media_uploads.rs` | Existing v1 documents include the version; suggestion `$setOnInsert` uses the complete encoder; tool replacement upserts include it. |
| Resource policies, shares, share commands and Agent artifacts | `resource_access/{records,admission,mutation,artifacts}.rs` | Shared policy/share/receipt/artifact encoders include v1 on all insert and replacement branches. |
| Sandbox billing, usage states/events, Agent channel usage/events | `sandbox_billing.rs`, `sandbox_usage.rs`, `agent_channel_usage/records.rs` | Both insert and replacement branches include v1. |
| Sandbox admissions | `sandbox_admission.rs` owner fence | `$setOnInsert` explicitly includes v1. |
| Profiles and profile revisions (Agent, Workspace, Sandbox), including defaults and marketplace forks | `profile_documents.rs`, `default_profiles.rs`, `marketplace_mutations.rs` | Shared encoders include v1; seeded upserts use the entire versioned document in `$setOnInsert`. |
| Warm Sandboxes and native prepared records/targets | `warm_sandboxes.rs`, `native_prepared.rs`, `native_prepared/planning.rs` | Record encoders include v1; target upsert explicitly sets v1. |
| Provider bindings, conversations, preparations, tool opens and relays | `bindings/documents.rs`, `bindings/reprovision`, `bindings/tools.rs`, `bindings/tools/relay.rs` | Insert/replacement documents include v1, including both tool replacement upserts. |
| Checkpoints, checkpoint objects, archive gates and tombstones | `checkpoints.rs`, `checkpoint_objects/{model,publication,purge,tombstones}.rs` | Encoders and gate initialization `$setOnInsert` include v1. Retirement upsert carries v1 in its equality predicate, which Mongo copies to an inserted row. |
| Sandbox credentials and revocations | `sandbox_credentials.rs`, `sandbox_credentials/{binding,custody}.rs` | Typed documents explicitly carry v1. |
| Auth subjects, login attempts, browser logins, API bearers, actor links and proxy grants | `auth.rs`, `auth_login_documents.rs`, `auth_api_bearers.rs`, `auth_actor_links.rs`, `auth_proxy_grants.rs` | Typed encoders include v1; browser-login replacement upsert uses its versioned encoder. |
| Activity, operation claims, users and platform settings | `activity.rs`, `operation.rs`, `directory.rs`, `settings.rs` | Typed records include v1; directory `$setOnInsert` and settings update pipeline explicitly persist it. |
| Runtime backend routes and initialization marker | `runtime_routing.rs` | Both `$setOnInsert` upserts include integer v1. |
| Request limits, lifecycle operations, message feedback, Sandbox colors | `request_limits.rs`, `agent_lifecycle_operations.rs`, `message_feedback.rs`, `sandbox_colors.rs` | Insert and replacement documents include v1. |
| Bot records, setup records/contexts, Gateway records/interactions, Codex subscriptions | `bots.rs`, `bot_setups.rs`, `bot_setup_contexts.rs`, `gateway_records.rs`, `gateway_interactions.rs`, `codex_subscriptions.rs` | Typed constructors/encoders include v1. |
| Trigger definitions/revisions, launches/events/input protocols, handoffs, visitor day counters | `trigger_definitions/documents.rs`, `trigger_definitions/launches/{documents,input_protocol,visitor_quota}.rs` | Every insert, batch insert and replacement uses versioned documents. |
| Usage report jobs/rows/charts/control, metric bindings and metric/usage rollups | `usage_reports/{admission,work,charts,schema,diagnostics}.rs`, `sandbox_metric_bindings`, `sandbox_metric_rollups`, `sandbox_usage_rollups` | All include v1, including raw bulk replacement/pipeline upserts and control `$setOnInsert`. |
| Document artifacts | `document_artifacts/mongo.rs::encode` | Both inserts and replacements use the existing v1 encoder. |
| Native warm pool targets | `native_warm_pools.rs`, `native_warm_pools/schema.rs`, `warm_pools.rs`, `warm_pools/defaults.rs` | Separate existing guard supports Agent v1 / machine v2. Serialized `PoolTarget` carries that version; unchanged. |
| `sandbox_retained_availability` | `sandbox_maintenance/retained_availability.rs` | Read-only legacy authority plus a non-upsert fence update; no application insert/upsert. |
| Event worker profiles/revisions and retained channel history references | `event_workers.rs`, `legacy_history.rs` | Existing v1 documents include the version even though these use their own read boundaries. Retired Bridge/history collections have no new generic writer to repair. |

The adjacent Event Gateway and Image Manager services own independent
`schema_version` fields and Mongo validators (`cmaeg/src/records/mongo*.rs`,
`cma-image-manager/src/{schema,store}.rs`). Their typed writers already include
their existing versions. They do not use this CMA startup guard and are unchanged.

## Regression coverage

`store/schema_mongo_integration_tests/session_versions.rs` runs through the
repository's disposable Mongo replica-set contract script. It checks a real
writer insert and restart, duplicate and invalid requests, versioned renewal,
refusal to downgrade explicit unknown versions, the exact deployed writer shape
against the exact deployed startup predicate, read-only legacy startup, WARN
redaction, both integer BSON representations, malformed/unknown shapes, and
both runtime-routing `$setOnInsert` paths. A pure test checks the original
writer's ID/timestamp limits. Existing schema rejection, auto-pause compatibility,
and all other Mongo contracts remain part of the same script selection.

## Local validation

Validation uses `CARGO_BUILD_JOBS=4`, debug information disabled for dev/test
profiles, and the shared heavy-step lock. Mongo tests use only the repository
script's owned disposable replica sets; production is not accessed or changed.

- `cargo fmt --all --check`: passed.
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: passed.
- `cargo nextest run -p cma --locked --no-fail-fast --test-threads 4`: 1,249 passed;
  682 tests skipped in this non-ignored run; Mongo contracts run separately.
- Desktop/session selection in `layer2-core` and `layer1-public`: 9 passed.
- `cargo test --doc --workspace --locked`: passed.
- Source-size check: passed; changed Rust files have at most 438 lines.
- Configuration key/documentation and API documentation checks: passed.
- Release/provider executable workflow contracts: passed; mutation suites rejected
  all 108 release and 63 provider unsafe mutations.
- Mongo harness contract tests: 7 passed.
- `.github/scripts/mongo-contract-tests.sh`: all 503 contracts passed across
  13 partitions, including all four new regressions; owned fixtures cleaned up.
- `cargo deny --locked --all-features check advisories licenses bans sources`:
  all four checks passed.
- Gitleaks scan of the final branch diff: passed with no findings.

Final integration merged `origin/develop` at `6750dac2a`. Its additional changes
are limited to frontend code, documentation, and a CI browser-test entry; the
backend source and Mongo harness match the tested tree. Workflow contracts,
mutation regressions, and source-size checks passed again after that merge.

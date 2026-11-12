# Retirement archive census authority

This is the namespace audit for #1726 / #1716. It replaces the generic identity
prefix scan. The catalogue in `store/retirement_archive/census_catalog.rs` names
all 142 collections in `store/isolation.rs` exactly once. Capture refuses a missing
or duplicate policy. Its schema hash binds both the namespace set and these
policies; old manifests cannot be certified under a changed census.

## Scope and deliberate refusal

Capture is for the selected Sandbox's owned records and its exact admitted
configuration snapshots. It is not an account/database backup. Account sign-in
custody, independent profile catalogues, rate limits and deployment controls are
not Sandbox-owned data and are never selected by coincidental `_id` equality.
The complete admitted profile contents and digest live inside each typed
Sandbox/Workspace/Agent snapshot; the independent catalogue's other drafts and
revisions are outside this operation.

**This release is not qualified for every populated production database.** An
`Empty` policy is an unresolved closure, not permission to omit it: the command
checks the entire namespace in the same Mongo snapshot and refuses if any row
exists. This intentionally includes potentially unrelated rows where ownership
cannot be proved. No row from that namespace is archived, and no completion is
issued. The release owner must run the read-only dry-run before scheduling a
source. A refusal needs a reviewed reader/closure extension; do not remove rows,
change the catalogue, or bypass the check to make a source eligible.

The unqualified cases include opaque operation outcomes, encrypted actor/Bot
origins, multi-resource palettes, prepared/warm ownership transfers, indirect
Trigger/provider graphs, retired external checkpoint/media transports, and legacy
Bridge/Channel cleanup namespaces whose writer/decoder was removed. The retained
legacy-history/cleanup readers do not establish complete ownership for all those
names. Global emptiness is the only currently admitted proof of no omitted data.

## Supported closure

Hierarchy discovery reads the actual `sandbox_id` and `record.sandbox_id` fields,
uses the production typed readers, and checks Workspace/Worktree parent membership.
It scans each present Agent's whole log against the authoritative high-water mark
and creation lineage. Quarantine, invalid lineage, missing parents and bounds fail
closed. Retained creation provenance resolves Agent/Workspace/Worktree identities
even after the live resource disappears. Access requester creation, lifecycle/reuse
receipts, and typed creation logs also contribute retained lineage; contradictory
references to an extant Agent in another Sandbox refuse capture. Commands are discovered by their exact
Agent index for dependent authorization audits.

Every supported namespace requires schema version 1 throughout the namespace;
future or unversioned records refuse before a v1 ownership filter can hide them.
The table's `Sandbox` and `Agent` paths compare only their corresponding typed
identity sets. `Activity` selects resource targets by exact raw ID, the actual
`sandbox:<ID>` administrative target, or literal source ID segments in concrete
`/api/v1/` paths. Route templates and neighbouring IDs stay excluded. Encoded
paths refuse because their ownership cannot be proved by literal comparison.
`Command` includes the command key, retained
`record.agent_id`, and `record.resources.sandbox_id` audit lineage.

`Budget` includes `record.agent_id` command/turn reservations,
`record.hold.scope.agent_id` request reservations, and the exact
`request-turn:<Agent ID>:<turn>` key grammar for request-turn accounting.
Unknown budget key families, malformed request-turn keys and missing nested
request ownership refuse capture before selection. The `owner:<subject>` baseline belongs to independent account accounting and is
not copied. Whole BSON records preserve request, consent, receipt and accounting
bytes; document revision objects are separately read back and verified. `Documents`
requires both the indexed and embedded owner to match the source Sandbox owner,
plus the originating/runtime Agent reference. Public creation accepts an origin
annotation, so it cannot authorize copying another account's body.

The full capture/restore regressions create access-request consent and budget
reservations through the production stores. Their expected inventories read the
entire isolated fixture collections, independently of the archive selector, and
compare restored lengths/hashes. Unrelated identity collisions and request-turn
prefix neighbours must stay excluded. Every `Empty` namespace is tested with a
nonempty unresolved row, and unrelated authentication custody remains outside the
archive. Source-independent restore is repeated after the fixture database drops.

## Namespace-by-namespace disposition

Paths below are relative to `api/crates/cma/src/store/` unless prefixed with
`document_artifacts`, `auth`, or `layer2-core`. `Independent` means an independent
account/library/configuration aggregate described above. `Empty` always refuses
a nonempty collection; its source entry identifies the missing closure or decoder.

| Namespace | Policy / exact ownership paths | Schema authority / audit basis |
| --- | --- | --- |
| `model_budget_reservations` | `Budget` | budget/mod.rs; budget/requests.rs; budget/requests/admission.rs |
| `sandbox_metric_bindings` | `Sandbox(&["sandbox_id"])` | sandbox_metric_bindings/mod.rs |
| `sandbox_metric_rollups` | `Sandbox(&["sandbox_id"])` | sandbox_metric_rollups/mod.rs |
| `sandbox_usage_rollups` | `Sandbox(&["sandbox_id"])` | sandbox_usage_rollups/mod.rs |
| `sandbox_usage_reports` | `Empty` | usage_reports/admission.rs |
| `sandbox_usage_report_rows` | `Sandbox(&["sandbox_id"])` | usage_reports/progress.rs |
| `sandbox_usage_report_charts` | `Sandbox(&["sandbox_id"])` | usage_reports/charts.rs |
| `sandbox_usage_report_control` | `Independent` | usage_reports/schema.rs |
| `request_rate_limits` | `Independent` | request_limits.rs |
| `visitor_launch_days` | `Independent` | trigger_definitions/launches/visitor_quota.rs |
| `cma_document_artifacts` | `Documents`: indexed and embedded source owner, plus originating/runtime Agent | document_artifacts/mongo.rs; layer2-core/document_artifacts/types.rs |
| `cma_document_quotas` | `Independent` | Retained rows from the reverted quota policy; no active writer. |
| `native_warm_pool_targets` | `Independent` | native_warm_pools.rs |
| `native_prepared_sandboxes` | `Empty` | native_prepared/model.rs; native_prepared.rs |
| `runtime_backend_routes` | `Sandbox(&["sandbox_id"])` | runtime_routing.rs |
| `activity` | `Activity` | activity.rs; activity/mod.rs; layer2-core/sandbox/admin.rs; rest/admin/runtime_update.rs |
| `agent_artifacts` | `Agent(&["agent_id"])` | resource_access/artifacts.rs |
| `agent_channel_usage` | `Agent(&["_id"])` | agent_channel_usage/records.rs |
| `agent_suggestions` | `Agent(&["agent_id"])` | suggestions.rs |
| `agent_command_orders` | `Agent(&["_id"])` | agent_commands/reserve.rs |
| `agent_commands` | `Agent(&["agent_id"])` | agent_commands/records.rs |
| `agent_creation_provenance` | `Sandbox(&["record.sandbox_id"])` | agent_provenance.rs; layer2-core/sandbox/provenance.rs |
| `agent_access_tools` | `Agent(&["agent_id"])` | agent_access_tools/records.rs |
| `agent_access_requests` | `Sandbox(&["record.requester.sandbox_id"])` | agent_access_requests/records.rs |
| `agent_log` | `Agent(&["agent_id"])` | agent_log.rs |
| `agent_message_feedback` | `Agent(&["agent_id"])` | message_feedback.rs |
| `agent_lifecycle_operations` | `Sandbox(&["sandbox_id"])` | agent_lifecycle_operations.rs |
| `agent_profile_revisions` | `Independent` | profile_documents.rs; layer2-core/profiles snapshots |
| `agent_profiles` | `Independent` | profile_documents.rs; layer2-core/profiles snapshots |
| `agent_reuse_bindings` | `Sandbox(&["sandbox_id"])` | agent_admission_reuse.rs |
| `agents` | `Sandbox(&["sandbox_id"])` | agents.rs |
| `auth_api_bearers` | `Independent` | auth_api_bearers.rs |
| `auth_actor_links` | `Empty` | auth_actor_links.rs; auth/actor_links/model.rs |
| `auth_nyxid_proxy_grants` | `Independent` | auth_proxy_grants.rs; auth/nyxid_proxy.rs |
| `auth_browser_logins` | `Independent` | auth.rs (account sign-in custody) |
| `auth_login_attempts` | `Independent` | auth.rs (account sign-in custody) |
| `auth_subjects` | `Independent` | auth.rs (account sign-in custody) |
| `binding_conversations` | `Empty` | bindings.rs |
| `binding_event_media_uploads` | `Empty` | media_uploads.rs |
| `bot_setups` | `Empty` | bot_setups.rs |
| `bot_setup_contexts` | `Empty` | bot_setup_contexts.rs |
| `bots` | `Empty` | bots.rs |
| `bridge_allocation_heads` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_allocations` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_callback_dispositions` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_callback_inbox` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_callback_platform_messages` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_callback_rates` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_callback_receipts` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_callback_replies` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_channel_capabilities` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_channel_control_aliases` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_channel_control_intents` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_channel_control_references` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_credentials` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_definition_receipts` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_definitions` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_invoke_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_invoke_grants` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_invoke_receipts` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_outbox_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_outbox_heads` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_outbox_invocations` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_outbox_items` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_outbox_recoveries` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_outbox_scans` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_outbox_send_rates` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_partition_command_receipts` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_refused_control_aliases` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_refused_control_intents` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_source_control_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_source_control_policies` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_source_control_receipts` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `bridge_text_attachments` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_child_cleanup_job_admissions_v1` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_child_cleanup_jobs_v1` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_command_revival_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_credential_renewal_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_history_references` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_lifecycle_credential_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_mint_orphan_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_mint_orphans` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_provider_dispatch_journals` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_provider_finality_receipts` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_provider_teardown_receipts` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_retirement_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_retirement_finalizations` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_reuse_cleanup_admission_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_reuse_cleanup_admissions` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_reuse_cleanup_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_reuse_cleanup_removal_audits` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_reuse_cleanup_requests` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_reuse_cleanup_tombstones` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `channel_usage_events` | `Agent(&["agent_id"])` | agent_channel_usage/records.rs |
| `checkpoint_archive_gates` | `Empty` | checkpoint_objects/model.rs |
| `checkpoint_archive_tombstones` | `Empty` | checkpoint_objects/model.rs |
| `checkpoint_objects` | `Empty` | checkpoint_objects/model.rs |
| `checkpoints` | `Empty` | checkpoints.rs |
| `codex_subscription_connections` | `Independent` | codex_subscriptions.rs |
| `event_worker_profiles` | `Independent` | event_workers.rs (independent catalogue) |
| `event_worker_profile_revisions` | `Independent` | event_workers.rs (independent catalogue) |
| `command_authorization_audits` | `Command` | agent_commands/audit.rs; layer2-core/sandbox/agent_io/authorization_audit.rs |
| `operation_claims` | `Empty` | operation.rs |
| `owned_workspace_cleanup_audits_v1` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `owned_workspace_cleanup_consents_v1` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `owned_workspace_cleanup_jobs_v1` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `owned_workspace_cleanup_tombstones_v1` | `Empty` | isolation.rs retained name; legacy_history.rs / lifecycle_execution/cleanup.rs only (no complete decoder) |
| `platform_settings` | `Independent` | settings.rs |
| `provider_bindings` | `Empty` | bindings.rs |
| `provider_binding_preparations` | `Empty` | bindings/reprovision.rs |
| `provider_tool_opens` | `Empty` | bindings.rs |
| `provider_tool_relays` | `Empty` | bindings.rs |
| `resource_policies` | `Sandbox(&["_id"])` | resource_access/records.rs |
| `resource_share_commands` | `Sandbox(&["sandbox_id"])` | resource_access/records.rs |
| `resource_shares` | `Sandbox(&["sandbox_id"])` | resource_access/records.rs |
| `sandbox_admissions` | `Independent` | sandbox_admission.rs |
| `sandbox_billing_state` | `Sandbox(&["_id"])` | sandbox_billing.rs |
| `sandbox_colors` | `Empty` | sandbox_colors.rs |
| `sandbox_controls` | `Sandbox(&["sandbox_id"])` | sandbox_controls/records.rs |
| `sandbox_credential_revocations` | `Sandbox(&["sandbox_id"])` | sandbox_credentials.rs |
| `sandbox_credentials` | `Sandbox(&["sandbox_id"])` | sandbox_credentials.rs |
| `sandbox_events` | `Sandbox(&["sandbox_id"])` | sandbox_events.rs |
| `sandbox_usage` | `Sandbox(&["_id"])` | sandbox_usage.rs |
| `sandbox_usage_events` | `Sandbox(&["sandbox_id"])` | sandbox_usage.rs |
| `sandbox_profile_revisions` | `Independent` | profile_documents.rs; layer2-core/profiles snapshots |
| `sandbox_profiles` | `Independent` | profile_documents.rs; layer2-core/profiles snapshots |
| `sandbox_retained_availability` | `Sandbox(&["_id"])` | sandbox_maintenance/retained_availability.rs |
| `sandbox_sessions` | `Sandbox(&["sandbox_id"])` | sandbox_sessions.rs |
| `sandboxes` | `Sandbox(&["_id"])` | sandboxes.rs |
| `service_connection_handoffs` | `Empty` | trigger_definitions/launches/documents.rs |
| `trigger_definition_revisions` | `Empty` | trigger_definitions/launches/documents.rs; trigger_first_messages.rs (indirect Launch/definition closure) |
| `trigger_definitions` | `Empty` | trigger_definitions/launches/documents.rs; trigger_first_messages.rs (indirect Launch/definition closure) |
| `trigger_first_message_bindings` | `Agent(&["agent_id"])` | trigger_definitions/launches/documents.rs; trigger_first_messages.rs (indirect Launch/definition closure) |
| `trigger_launch_events` | `Empty` | trigger_definitions/launches/documents.rs; trigger_first_messages.rs (indirect Launch/definition closure) |
| `trigger_launch_input_protocols` | `Empty` | trigger_definitions/launches/documents.rs; trigger_first_messages.rs (indirect Launch/definition closure) |
| `trigger_launches` | `Empty` | trigger_definitions/launches/documents.rs; trigger_first_messages.rs (indirect Launch/definition closure) |
| `users` | `Independent` | directory.rs |
| `warm_sandboxes` | `Empty` | warm_sandboxes.rs; layer2-core/sandbox/warmup.rs |
| `workspace_profile_revisions` | `Independent` | profile_documents.rs; layer2-core/profiles snapshots |
| `workspace_profiles` | `Independent` | profile_documents.rs; layer2-core/profiles snapshots |
| `workspaces` | `Sandbox(&["sandbox_id"])` | workspaces.rs |
| `worktrees` | `Sandbox(&["sandbox_id"])` | worktrees.rs |

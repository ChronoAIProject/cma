# Account erasure catalog ownership evidence

This audit names every collection in `store/isolation.rs` and its optional
`isolation/replicas.rs` authority exactly once. Source paths below
are relative to `api/crates/cma/src/store/` unless they name another crate or
`document_artifacts/`. The executable catalog is `store/account_erasure/catalog.rs`.
A selector is indexed only when an existing index has its matching field as a
leading key (or uses the exact `_id`). Partial indexes do not establish support for
historical rows outside the partial predicate. Unindexed rules use the shared page
scan. The erasure executor installs the documented discovery indexes once at
first cohort load; indexed effect classifications remain conservative. See the
second-review migration in CMA_Account_Deletion.md.

## Reading the policies

`Owner` compares exact subject strings. `Linked` compares typed resource IDs from
the live and durable historical closure, unioned with previously inventoried IDs. `SubjectPrefix` denotes
an exact composed key, such as `owner:<subject>`, rather than an unbounded prefix.
`LinkedKey` matches a literal prefix followed by a whole resource ID and `:`.
`VisitorDay` matches the numeric UTC-day prefix and the complete subject suffix.
`RequestLimit` computes each of the three subject bucket keys using the writer's
`sha256("subject:<subject>")` encoding. A reference policy may appear twice when
its indexed projection and unindexed embedded references need separate passes.
For resource shares, the indexed pass covers accepted recipients and the shared
scan also removes unaccepted explicit audience subjects. Personal resource closure
excludes all
Sandboxes whose `resource_policies.owner_kind` is `organization`; their descendants
and policy/share records remain, while personal references are redacted.

`Custody`, `Sealed`, and `ObjectAccounting` are declarations for subsequent steps.
`EraseRecords` skips them. Checkpoint manifests and object tombstones are deferred
alongside document quotas because their exact object keys and cleanup authority
must survive until external object absence is confirmed. Account-deletion records
and events retain the deletion receipt and restore authority under the documented
expiry policy. The reporting control collection mixes deployment-wide scan positions/leases
with `diagnostic:` bindings containing account and Sandbox identity; only the
matching diagnostic rows are removed. The platform settings singleton is a reference policy
because its model budget owner overrides do contain subjects.

## Audited collections

| Collection | Policy | Indexed | Writer and index evidence |
| --- | --- | --- | --- |
| `account_deletions` | `Retained("deletion-receipt")` | yes | account_deletions/{records,events}.rs: deletion audit and restore authority; expiry after completion |
| `account_deletion_events` | `Retained("deletion-receipt")` | yes | account_deletions/{records,events}.rs: deletion audit and restore authority; expiry after completion |
| `model_budget_reservations` | `Owner(&["record.owner"])`; `OwnerKey(SubjectPrefix("owner:"))`; `LinkedKey(Agents, "request-turn:")` | no; yes; yes | budget/mod.rs TurnRecord; budget/requests.rs RequestRecord; no owner index; budget/mod.rs owner_key: exact owner:<subject> baseline; _id index; budget/requests.rs ledger_key: request-turn:<agent>:<turn>; literal delimited prefix on _id |
| `sandbox_metric_bindings` | `Linked(Sandboxes, &["sandbox_id"])` | yes | sandbox_metric_bindings/mod.rs writes sandbox_id; compound index begins sandbox_id |
| `sandbox_metric_rollups` | `Linked(Sandboxes, &["sandbox_id"])` | yes | sandbox_metric_rollups/mod.rs writes sandbox_id; compound index begins sandbox_id |
| `sandbox_usage_rollups` | `Linked(Sandboxes, &["sandbox_id"])` | yes | sandbox_usage_rollups/mod.rs writes sandbox_id; compound index begins sandbox_id |
| `sandbox_usage_allocation_buckets_v1` | `Linked(Sandboxes, &["sandbox_id"])`; `Owner(&["owner"])` | yes; no | allocation_buckets/schema.rs projects sandbox_id and owner; compound sandbox_id/day index; owner fallback covers buckets whose Sandbox has already gone |
| `sandbox_usage_reports` | `Owner(&["principal"])` | yes | usage_reports/admission.rs report.scope.principal projection; schema.rs principal/state/lease index |
| `sandbox_usage_report_rows` | `Linked(Reports, &["report_id"])`; `Owner(&["owner"])` | yes; no | usage_reports/{work,charts}.rs; schema.rs compound index begins report_id; usage_reports/{work,charts}.rs frozen resource owner; indexes begin report_id, not owner |
| `sandbox_usage_report_charts` | `Linked(Reports, &["report_id"])`; `Owner(&["owner"])` | yes; no | usage_reports/{work,charts}.rs; schema.rs compound index begins report_id; usage_reports/{work,charts}.rs frozen resource owner; indexes begin report_id, not owner |
| `sandbox_usage_report_control` | `Reference(DiagnosticBindings)` | no | usage_reports/diagnostics.rs diagnostic: rows contain record.owner_subject and record.sandbox_id; other schema control keys/leases remain global |
| `request_rate_limits` | `OwnerKey(RequestLimit)` | yes | rate_limit/shared.rs default/create/input:<sha256(subject:<subject>)>; exact _id; client/network buckets have no account attribution |
| `visitor_launch_days` | `OwnerKey(VisitorDay)` | no | trigger_definitions/launches/visitor_quota.rs key <UTC day>:<subject>; only expiry index, numeric-day suffix scan |
| `cma_document_artifacts` | `Owner(&["owner_subject"])`; `Reference(Documents)` | yes; no | document_artifacts/mongo.rs encode/ensure_indexes owner_subject; independent owner, not originating Agent; document_artifacts/mongo.rs and layer2-core/document_artifacts/types.rs: recipient projection indexed, embedded shares/comments/receipts not indexed; one shared page handles all |
| `cma_document_quotas` | `ObjectAccounting` | no | document_artifacts/quota.rs owner:<hash>, document:<hash>:*, object:<hash>:*; defer until object finality |
| `native_warm_pool_targets` | `Global` | yes | native_warm_pools.rs target is deployment resource plan; no human owner |
| `cma_agent_manager_members` | `Global` | yes | agent_ownership/{mod,records}.rs process membership and relay endpoints; no personal owner; existing expiry index remains authoritative |
| `cma_agent_manager_owners` | `Retained("agent-ownership-fence")` | yes | agent_ownership/mod.rs requires permanent Agent epoch high watermarks; account erasure must not reset takeover fencing; existing _id index |
| `native_prepared_sandboxes` | `Owner(&["record.ownership.owner"])` | no | native_prepared.rs document + native_prepared/model.rs Ownership; consumed reservation retains explicit owner; index begins kind/state |
| `runtime_backend_routes` | `Linked(Sandboxes, &["sandbox_id"])` | no | runtime_routing.rs setOnInsert sandbox_id; no sandbox_id index (migration control rows remain) |
| `activity` | `Owner(&["record.actor"])`; `Reference(ActivityTargets)`; `Reference(Identities)` | yes; no; no | activity.rs ActivityDoc and actor/recorded_at/_id index; layer2-core/activity.rs ActivityTarget kind/id; target subject has no index |
| `agent_artifacts` | `Linked(Agents, &["agent_id"])`; `Reference(Identities)` | yes; no | resource_access/artifacts.rs document + agent_id/_id index |
| `agent_channel_usage` | `Linked(Agents, &["_id"])`; `Reference(Identities)` | yes; no | agent_channel_usage/records.rs state_document _id = creation.agent_id |
| `agent_suggestions` | `Linked(Agents, &["agent_id"])`; `Owner(&["owner"])` | no; yes | suggestions.rs projection; owner-leading index, no agent_id-leading index; suggestions.rs write projection and owner/agent/answer/version index |
| `agent_command_orders` | `Linked(Agents, &["_id"])` | yes | agent_commands/reserve.rs _id = agent.id |
| `agent_commands` | `Linked(Agents, &["agent_id"])`; `Reference(Identities)` | yes; no | agent_commands/records.rs projection and agent_id/order indexes |
| `agent_creation_provenance` | `Linked(Agents, &["_id"])`; `Reference(Identities)` | yes; no | agent_provenance.rs record_creation uses Agent ID as _id; no secondary index; retained foreign/organization creation actors receive exact CAS identity rewrites |
| `agent_access_tools` | `Linked(Agents, &["agent_id"])` | yes | agent_access_tools/records.rs persist projection; sandbox_mongo.rs agent_id/conversation index |
| `agent_access_requests` | `Linked(Sandboxes, &["record.requester.sandbox_id"])` | yes | agent_access_requests/records.rs and agent_access_requests.rs requester sandbox/phase index |
| `agent_log` | `Linked(Agents, &["agent_id"])` | yes | agent_log_codec.rs log_document; agent_log.rs agent_id/sequence index |
| `agent_log_heads` | `Linked(Agents, &["_id"])` | yes | agent_log_segments/codec.rs Head _id = agent_id |
| `agent_log_segments` | `Linked(Agents, &["agent_id"])` | yes | agent_log_segments/codec.rs Seal; schema.rs agent_id/incarnation/ordinal index |
| `agent_log_publications` | `Linked(Agents, &["agent_id"])` | yes | agent_log_segments/publication.rs Publication; schema.rs agent_id-leading indexes |
| `agent_log_projections` | `Linked(Agents, &["_id"])` | yes | agent_log_segments/projection.rs Projection _id = agent_id |
| `agent_log_evidence` | `Linked(Agents, &["agent_id"])` | yes | agent_log_segments/projection.rs Evidence; schema.rs agent_id-leading indexes |
| `agent_log_audits` | `Linked(Agents, &["agent_id"])` | yes | agent_log_segments/audit.rs Progress; schema.rs agent_id/audit_id index |
| `agent_log_gates` | `Reference(LogGates)` | no | agent_log_segments/writer_gate.rs GateRecord policy.owners and policy.agents; writer_gate_slots.rs slots holder includes agent publication identity; revision/admission_fence/exact-slots CAS because renewal can preserve both counters |
| `agent_log_admissions` | `Linked(Agents, &["agent_id"])` | yes | agent_log_segments/writer_admission.rs Admission; schema.rs agent_id/incarnation/publication_key index |
| `agent_capacity_recovery_work` | `Linked(Agents, &["agent_id"])` | yes | agent_capacity_recovery/records.rs Row; format2.indexes.json agent_id-leading index |
| `agent_message_feedback` | `Owner(&["owner"])` | yes | message_feedback.rs Record + owner/agent/sequence index |
| `agent_lifecycle_operations` | `Owner(&["owner"])`; `Reference(Identities)` | yes; no | agent_lifecycle_operations.rs LifecycleRecord + owner/key index |
| `agent_profile_revisions` | `Reference(Profiles)` | no | profile_documents.rs revision_document and profiles.rs immutable revision has no owner; shared scan retains referenced content and deletes unreferenced revisions after fencing the aggregate |
| `agent_profiles` | `Reference(Profiles)` | no | profile_documents.rs profile_document and profiles.rs owner projection; one shared reference scan retains private retired anonymous aggregates without authoring drafts and exact child pins |
| `agent_reuse_bindings` | `Owner(&["owner"])` | no | agent_admission_reuse.rs insert owner,agent_id,sandbox_id; no owner index |
| `agents` | `Linked(Sandboxes, &["sandbox_id"])` | yes | agents.rs agent_document + sandbox_id index; include retired and late-created Agents |
| `auth_api_bearers` | `Custody(&["subject"])` | no | auth_api_bearers.rs ApiBearerDoc; immutable encrypted access_token, no subject index |
| `auth_actor_links` | `Sealed` | no | auth_actor_links.rs authority envelope contains identity grant; execution belongs to credential cleanup |
| `auth_nyxid_proxy_grants` | `Sealed` | no | auth_proxy_grants.rs grant envelope contains subject; no plaintext owner; execution belongs to credential cleanup |
| `auth_browser_logins` | `Custody(&["subject"])` | no | auth_browser_documents.rs BrowserLoginDoc; expires_at TTL only |
| `auth_login_attempts` | `Sealed` | no | auth_browser_documents.rs and auth_login_documents.rs browser binding plus encrypted actor_link_intent; no subject projection; execution belongs to credential cleanup |
| `auth_subjects` | `Custody(&["_id"])` | yes | auth.rs SubjectDoc _id = subject; owner token needed for final revocation |
| `binding_conversations` | `Linked(Bindings, &["binding_id"])` | yes | bindings/documents.rs conversation_document projection; bindings.rs binding_id/generation/conversation index |
| `binding_event_media_uploads` | `Linked(Agents, &["record.agent_id"])` | no | media_uploads.rs record_bson; cma/src/media_continuations/model.rs MediaUploadRecord agent_id; no selector index |
| `bot_setups` | `Sealed` | no | bot_setups.rs encrypted setup envelope, actor key is not human subject; execution belongs to credential cleanup |
| `bot_setup_contexts` | `Sealed` | no | bot_setup_contexts.rs encrypted payload and digest(binding,conversation,event); no plaintext owner; execution belongs to credential cleanup |
| `bots` | `Custody(&["owner"])`; `Reference(Identities)` | yes; no | bots.rs BotDocument owner projection and owner/creation-key index; Bot cleanup/revocation precedes credential deletion |
| `bridge_allocation_heads` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_allocations` | `ProvenAbsent(LEGACY_ALLOCATION_PATHS)` | no | legacy_history.rs reads record.owner and placement; legacy_reuse_lifecycle.rs/channel_storage_retention.rs retain partial readers, no complete ownership decoder; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_callback_dispositions` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_callback_inbox` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_callback_platform_messages` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_callback_rates` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_callback_receipts` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_callback_replies` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_channel_capabilities` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_channel_control_aliases` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_channel_control_intents` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_channel_control_references` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_credentials` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_definition_receipts` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_definitions` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_invoke_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_invoke_grants` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_invoke_receipts` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_outbox_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_outbox_heads` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_outbox_invocations` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_outbox_items` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_outbox_recoveries` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_outbox_scans` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_outbox_send_rates` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_partition_command_receipts` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_refused_control_aliases` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_refused_control_intents` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_source_control_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_source_control_policies` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_source_control_receipts` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `bridge_text_attachments` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_child_cleanup_job_admissions_v1` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_child_cleanup_jobs_v1` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_command_revival_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_credential_renewal_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_history_references` | `ProvenAbsent(LEGACY_HISTORY_PATHS)` | no | legacy_history.rs still writes allocation_id and record target reference; owner exists only through unresolved bridge allocation, no direct owner field; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_lifecycle_credential_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_mint_orphan_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_mint_orphans` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_provider_dispatch_journals` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_provider_finality_receipts` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_provider_teardown_receipts` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_retirement_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_retirement_finalizations` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_reuse_cleanup_admission_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_reuse_cleanup_admissions` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_reuse_cleanup_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_reuse_cleanup_removal_audits` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_reuse_cleanup_requests` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_reuse_cleanup_tombstones` | `ProvenAbsent(LEGACY_CLEANUP_PATHS)` | no | lifecycle_execution/cleanup.rs reads sandbox_id, record.seal.target.sandbox_id, record.admission.sandbox_id; no complete owner decoder; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `channel_usage_events` | `Linked(Agents, &["agent_id"])`; `Reference(Identities)` | yes; no | agent_channel_usage/records.rs event_document and agent_id/revision index; active typed writer, not unresolved legacy |
| `checkpoint_archive_gates` | `Linked(Sandboxes, &["_id"])` | yes | checkpoint_objects/publication.rs touch_gate uses sandbox id as _id |
| `checkpoint_archive_tombstones` | `ObjectAccounting` | no | checkpoint_objects/tombstones.rs retains object_key without owner/sandbox projection; ownership from cma/checkpoints/<sha256(sandbox)>/ prefix; defer until objects absent |
| `checkpoint_objects` | `ObjectAccounting` | yes | checkpoint_objects/model.rs ManifestDoc sandbox_id and object_key; sandbox_id-leading index; preserve object finality authority |
| `checkpoints` | `Linked(Sandboxes, &["sandbox_id"])` | yes | checkpoints.rs CheckpointDoc + sandbox_id/revision index; external object keys already inventoried |
| `codex_subscription_connections` | `Custody(&["_id"])` | yes | codex_subscriptions.rs SubscriptionDoc _id = owner; retained encrypted subscription credentials |
| `event_worker_profiles` | `Owner(&["owner"])` | yes | event_workers.rs document projection and owner/retired/_id index |
| `event_worker_profile_revisions` | `Owner(&["owner"])` | no | event_workers.rs save copies full projected row changing _id to id:revision; no owner index on revisions |
| `command_authorization_audits` | `Linked(Agents, &["record.agent_id"])`; `Reference(Identities)` | no; no | agent_commands/audit.rs record; layer2-core/sandbox/agent_io/authorization_audit.rs typed agent_id; index begins command_id |
| `operation_claims` | `Owner(&["_id.subject"])` | no | operation.rs OperationIdDoc {subject,operation,key}; no nested-subject index |
| `owned_workspace_cleanup_audits_v1` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `owned_workspace_cleanup_consents_v1` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `owned_workspace_cleanup_jobs_v1` | `ProvenAbsent(LEGACY_OWNER_PATHS)` | no | No current writer/typed decoder remains; only isolation.rs and retirement census register this namespace; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `owned_workspace_cleanup_tombstones_v1` | `ProvenAbsent(LEGACY_CLEANUP_PATHS)` | no | lifecycle_execution/cleanup.rs reads sandbox target markers; original writer/owner decoder removed; candidate owner/owner_subject/subject/record.owner/record.owner.subject/record.owner_subject/record.subject selectors require operator confirmation |
| `platform_settings` | `Reference(PlatformSettings)` | no | settings.rs revision CAS; layer2-core/platform.rs and budget.rs settings.model_budget.owners[].owner personal policy; preserve singleton |
| `provider_bindings` | `Custody(&["record.owner"])`; `Reference(Identities)` | yes; no | bindings/documents.rs ProviderBinding record; bindings.rs record.owner/_id index; encrypted key custody |
| `provider_binding_preparations` | `Owner(&["owner"])` | no | bindings/reprovision.rs receipt_document owner projection; no owner index |
| `provider_tool_opens` | `Linked(Agents, &["record.scope.agent_id"])` | no | bindings/tools.rs OpenRecord and bindings/tools/validation.rs Scope; no matching index |
| `provider_tool_relays` | `Linked(Agents, &["agent_id"])` | no | bindings/tools/relay.rs write projection; no agent_id-leading index |
| `resource_policies` | `Linked(Sandboxes, &["_id"])`; `Reference(Identities)` | yes; no | resource_access/records.rs policy_document; parent closure excludes owner_kind organization; retained organization evidence actors use exact CAS tombstones |
| `resource_share_commands` | `Linked(Sandboxes, &["sandbox_id"])`; `Reference(ShareReceipts)`; `Reference(Identities)` | no; no; no | resource_access/records.rs receipt_document; no sandbox_id-leading receipt index; preserve foreign/organization audit rows and canonically redact personal actors; resource_access/records.rs embeds record.share.link.accepted_subjects; embedded share revision preserved; outer mutation_version increments with exact original-record CAS, requiring absent mutation_version on the first rewrite |
| `resource_shares` | `Linked(Sandboxes, &["sandbox_id"])`; `Reference(Shares)`; `Reference(Shares)`; `Reference(Identities)` | yes; yes; no; no | resource_access/records.rs share_document + resource_access.rs sandbox selector index; accepted_subjects projection has a leading index; unaccepted record.link.audience.subjects has no index and uses the shared scan |
| `sandbox_admissions` | `OwnerKey(Subject)` | yes | sandbox_admission.rs owner fence _id = subject; global::admit key is preserved |
| `sandbox_billing_state` | `Linked(Sandboxes, &["_id"])`; `Owner(&["owner_id"])` | yes; no | sandbox_billing.rs serializes funding/model.rs SandboxBillingState with owner_id and _id = sandbox_id; the erasure migration installs a leading owner_id index for orphan discovery; pending claims and retained windows block before any usage deletion, while a stopped settled current receipt is historical evidence |
| `sandbox_colors` | `OwnerKey(Subject)` | yes | sandbox_colors.rs Palette _id = owner |
| `sandbox_controls` | `Linked(Sandboxes, &["sandbox_id"])`; `Reference(Identities)` | no; no | sandbox_controls/records.rs document; sandbox_id-leading index is partial queued=true, not all historical receipts |
| `sandbox_credential_revocations` | `Custody(&["owner"])` | no | sandbox_credentials.rs RevocationDoc owner; revocation authority needed until key finality; index begins sandbox_id |
| `sandbox_credential_issuance` | `Custody(&["record.owner"])` | no | sandbox_credentials/issuance.rs retains one unresolved issuance barrier per Sandbox; record.owner is the exact subject; preserve custody until credential finality, without adding effects in this slice |
| `sandbox_credentials` | `Custody(&["owner"])` | no | sandbox_credentials.rs CredentialDoc owner; key material needed for credential step; index begins sandbox_id |
| `sandbox_events` | `Linked(Sandboxes, &["sandbox_id"])` | yes | sandbox_events.rs event_document + sandbox_id/seq index |
| `sandbox_usage` | `Owner(&["owner"])` | yes | sandbox_usage.rs state_document owner projection + owner/_id index |
| `sandbox_usage_events` | `Owner(&["owner"])` | yes | sandbox_usage.rs event_document owner projection + owner/ended_at index; delivery guard before delete |
| `sandbox_profile_revisions` | `Reference(Profiles)` | no | profile_documents.rs revision_document and profiles.rs immutable revision has no owner; shared scan retains referenced content and deletes unreferenced revisions after fencing the aggregate |
| `sandbox_profiles` | `Reference(Profiles)` | no | profile_documents.rs profile_document and profiles.rs owner projection; one shared reference scan retains private retired anonymous aggregates without authoring drafts and exact child pins |
| `sandbox_retained_availability` | `Linked(Sandboxes, &["_id"])` | yes | sandbox_maintenance/retained_availability.rs and maintenance record writer bind _id = sandbox id |
| `sandbox_sessions` | `Linked(Sandboxes, &["sandbox_id"])` | yes | sandbox_sessions.rs insert projection + sandbox_id/lease_expires_at index |
| `sandboxes` | `Linked(Sandboxes, &["_id"])` | yes | sandboxes.rs sandbox_document; execution additionally requires retired status, expired/absent lease and exact revision |
| `service_connection_handoffs` | `Owner(&["owner"])` | yes | trigger_definitions/launches/documents.rs handoff_document + launches/indexes.rs owner/_id index |
| `trigger_definition_revisions` | `Reference(TriggerRevisions)` | no | trigger_definitions/documents.rs revision_document owner projection; revision indexes begin trigger_id/profile ids, not owner; definition deletion precedes canonical publisher tombstone rewrite; all intermediate public reads return typed unavailable |
| `trigger_definitions` | `Owner(&["owner"])` | yes | trigger_definitions/documents.rs trigger_document + indexes.rs owner/_id index |
| `trigger_first_message_bindings` | `Linked(Agents, &["agent_id"])` | yes | trigger_first_messages.rs document and unique agent_id index |
| `trigger_launch_events` | `Linked(Launches, &["launch_id"])` | yes | trigger_definitions/launches/documents.rs event_document + indexes.rs launch_id/seq index |
| `trigger_launch_input_protocols` | `Linked(Launches, &["_id"])` | yes | trigger_definitions/launches/input_protocol.rs _id = launch id |
| `trigger_launches` | `Owner(&["owner"])` | yes | trigger_definitions/launches/documents.rs launch_document + indexes.rs owner/_id index; publisher identity does not own visitor Launch |
| `users` | `OwnerKey(Subject)` | yes | directory.rs UserDoc _id = identity subject |
| `warm_sandboxes` | `Linked(Sandboxes, &["_id"])` | yes | warm_sandboxes.rs document and layer2-core/sandbox/warmup.rs claim: reservation id becomes adopted Sandbox id; _id index |
| `workspace_profile_revisions` | `Reference(Profiles)` | no | profile_documents.rs revision_document and profiles.rs immutable revision has no owner; shared scan retains referenced content and deletes unreferenced revisions after fencing the aggregate |
| `workspace_profiles` | `Reference(Profiles)` | no | profile_documents.rs profile_document and profiles.rs owner projection; one shared reference scan retains private retired anonymous aggregates without authoring drafts and exact child pins |
| `workspaces` | `Linked(Sandboxes, &["sandbox_id"])` | yes | workspaces.rs workspace_document + nonpartial sandbox_id index |
| `worktrees` | `Linked(Sandboxes, &["sandbox_id"])` | yes | worktrees.rs worktree_document + sandbox_id index |

## Legacy read-only confirmation

`ProvenAbsent` is a conservative refusal, not a declaration that production has
been counted. No production database was accessed for this change. Operators
must count the candidate subject selectors for each listed namespace before
activation and review any hit. Most names have no remaining writer or complete
decoder in the current tree. In particular, `channel_history_references` retains
an allocation ID rather than a subject, and cleanup tombstones retain Sandbox
targets. The shared scanner also checks known linked lineage before returning
`UnsupportedResidue { collection }`; the candidate paths compare against both
subjects and their known resource IDs, and never authorize deletion. A known
allocation hit blocks before its associated history references can be overlooked. A missing direct subject match cannot certify opaque legacy attribution.

The candidate subject paths are `owner`, `owner_subject`, `subject`, `record.owner`,
`record.owner.subject`, `record.owner_subject`, and `record.subject`. For partial
legacy allocation and cleanup readers, operators must also resolve `allocation_id`,
`record.allocation_id`, `record.target_id`, `sandbox_admission.sandbox_id`,
`record.placement.target.sandbox_id`, `sandbox_id`,
`record.seal.target.sandbox_id`, and `record.admission.sandbox_id` against the
deleting account's live/inventoried closure. New evidence must update this catalog
before it authorizes destructive handling.

## Pending product decisions

Billing and usage default to erasure after all delivery claims settle (pending
product decision). Billing states remain attributable by `owner_id` even when their
Sandbox and inventory entry are absent. Pending reservation requests and retained
settlement windows block usage erasure. A stopped state with no pending request or
retained windows is settled even when `current` still holds the confirmed receipt:
`funding/evidence.rs` removes delivered windows without clearing `current`. Comments authored on another owner's document are removed, and
their receipts retain a `deleted-account` author with an empty body (pending product
decision). Deleted publishers leave redacted Trigger revision tombstones so completed visitor Launches and accepted Launches with an already committed
Agent keep their retained resource path; Draft and Agent-less accepted Launches
return typed retired/unavailable (pending product decision). Organization-owned
resources remain organization property and only personal references are removed
(pending product decision).

## Review-round execution amendments

The history closure uses durable ownership sources, cached for one resumable
cohort, with pages of 500 rows and a per-person 20,000-ID / 512 KiB identity /
128 KiB live Sandbox snapshot ceiling. Projected ownership-query results also have a
1 MiB ceiling. Organization descendants do not consume those
limits. Incoming inventory is checked against current roots and durable sidecar
ownership in pages of 500 before charging personal capacity. Serialized cursors
have a separate 1 MiB ceiling. Resource and Profile state share a 768 KiB
serialized per-person budget, leaving headroom even with the separate live
snapshots. Capacity and corrupt
record blocks include the responsible subject hash. New caller inventory is unioned; current foreign roots and
Launch/input-protocol or binding/conversation sidecars veto stale ownership.
Historical sources survive the ordinary child pass and are returned for durable
inventory acknowledgement before deletion. Indexed effect selection sorts `_id`
only within an exact selector range supported by a full
`{selector_field: 1, _id: 1}` compound index. Versioned Agent log carriers reuse
their existing full native indexes with one bounded tuple position; absent
carriers remain absent, partial explicit migrations remain restartable, and their
frozen DDL inventory is unchanged.
Unindexed scans remain shared across
up to 64 subjects.

`sandbox_usage_report_control` is mixed state: only `diagnostic:` rows with a
matching `record.owner_subject` or known `record.sandbox_id` are deleted. Ordinary
global controls remain intact. `agent_creation_provenance` uses its `_id` Agent
identity index, not an invented secondary owner index.

Organization resources and attributable operator audits retain canonical
`deleted-account` actors, including policy evidence, manager/creation actors,
commands, controls, artifacts, channel usage, Bot custody references and explicit
activity resource targets. Personal feedback and suggestions are deleted.
Generic HTTP activity templates cannot establish a concrete organization target.
Actor attribution never grants personal ownership of a resource target. Activity
target candidates preserve organization audits only after a matching typed
organization policy is found; rootless foreign targets do not enter inventory.
Legacy organization share snapshots receive precise recipient/actor rewrites;
ordinary share creation validation is unchanged. Bot key/debt identifiers survive;
personal delegation debt is handed off before identity rewriting. These rules
are pending product decision; reassignment and remote revocation are not supplied
by the record executor.

Referenced Profiles retain only required immutable revisions and exact transitive
pins. Their aggregate owner becomes `deleted-account`; visibility is private,
retirement is permanent, names/descriptions are empty, and authoring drafts and
fork identity are absent. Existing availability policy remains readable. New
references and forks refuse even though existing exact pins remain available.
This policy is pending product decision.

A surviving Trigger publisher's exact frozen target remains an existing
reference when another Profile author's account is erased. Its accepted Launch
effects wait until the required tree is canonically retained, then may continue
only with transactional proof of the durable Launch, exact live effect, input
receipt, surviving revision and resource lineage. A manual reference or an erased
Trigger publisher cannot use this continuation path.
Trust disclosures use neutral `Sandbox` and `Workspace` labels for canonical
retained metadata while preserving exact commands, digests and confirmation.
This does not restore stored authoring metadata or admit manual references.

The erasure side majority-writes Profile/Trigger fences with server timestamps,
then waits `max(ceil(1.5 * transactionLifetimeLimitSeconds) + 10 seconds,
configured floor)` before checkpointed shared reference discovery. The cleanup
period can extend an expired transaction by half its lifetime; ten seconds cover
scheduling jitter. The configured floor defaults to 120 seconds and remains in
force when the parameter is readable. If it is unreadable, the fallback must
cover that same cleanup bound on every server. Never increase the lifetime during
an active drain. Each call scans at most one ordered page of 500 reference rows;
the cursor stores its stage and last identity. No record effects precede a
complete validated closure. Admission transactions read
fences without writing catalogue counters. Unpublished counter fields are removed.
Reference/history sources are streamed once and cached; refresh uses indexed root
and sidecar lookups instead of repeatedly walking an organization subtree.
Operator audits with a personal `detail.subject` redact that field while retaining
the operator and resource target for every admin target shape (`profiles`,
`profile:<kind>:<id>`, `sandboxes`, Sandbox and event targets), pending product
decision.

Personal-share recipient removal first reserves the matching policy revision
with exact CAS, then changes the share with exact CAS. The full access reader's
policy fence remains valid in every intermediate state. A conflicting share
writer cannot overwrite concurrent recipients; a reservation may safely remain
after conflict. Live Sandbox/policy lineage must agree, while a missing Sandbox
and policy permit the existing orphan-audit rewrite. Policy reservations are
concurrency fences rather than redacted-content counters.

Billing-enabled callers must block on every undelivered usage event, including
unclaimed events. Disabled billing permits unclaimed events to be erased, while
existing undelivered claims and unsettled funding still block. Block errors
carry the responsible subject hash. All partial effects accompany any subsequent
error, and overlapping erasure workers reconcile absence by querying survivors.
Cursors bind catalog digest, collection name, indexed flag, subject set and
billing mode; mismatches restart safely.

Existing durable Sandbox and Workspace snapshots authorize only their exact
declared child pins, including Agents declared by a frozen Trigger Workspace.
The admission transaction validates the parent, snapshot, index, digest and
lineage; manual new references remain refused throughout erasure and retirement.
Organization binding ownership follows `metadata.owner.subject` through the
binding to `Sandbox.owner`; the manager remains a separate audit actor. These
organization resources and their descendants establish surviving Profile roots.
Indexed unchanged pages retain the exact selector and scalar `_id` or native tuple
position; protected rows never trigger a collection-wide fallback. Each indexed page call
probes at most eight empty selectors; the executor reads at most one nonempty
effect page of 500 rows per invocation. Capacity attribution
includes the responsible member's Profile graph and retained-pin contribution;
clock and driver failures remain retryable without account attribution.
Deterministic unassigned corruption is a typed permanent cohort block; invalid
readable lifetime values or unsupported drain configuration require operator
intervention rather than repeated retries. The separate graph-byte counter is
removed; serialized member and cursor budgets remain authoritative.

Pending exact child pins require a surviving parent. Active deletion evidence or
owner-specific Profile fences keep a deleting personal parent's continuation
Pending until canonical retention; a verified organization policy overrides any
subject-string overlap. Canonical retained pins and surviving roots still finish
their precise declarations. See the fourth-review migration in
[CMA Account Deletion](CMA_Account_Deletion.md#fourth-review-selector-and-permanent-refusal-migration)
for the compound-index set, cursor reader floor and permanent cohort-block contract.

# Account erasure catalog ownership evidence (C2)

This audit names every collection in `store/isolation.rs` and its optional
`isolation/replicas.rs` authority exactly once. Source
paths are relative to `api/crates/cma/src/store/` unless another crate is named.
The executable catalog is `store/account_erasure/catalog.rs`. The table records
ownership/reference classification and writer/index evidence; a reference rule
does not imply that every staged transformation is enabled in this slice.

`Owner` compares exact personal subjects. `Linked` uses verified live and durable
historical ownership and acknowledged inventory. Exact composed keys and
delimited Agent-turn keys preserve complete identities. Activity actors never
claim their resource targets. Organization roots and descendants remain
protected independently of whether identity rewriting is available.

An index supports a selector only when its leading keys cover the complete
historical range. Partial indexes do not establish historical coverage. The
executor installs documented discovery/effect indexes idempotently, keeps one
bounded exact selector position, and reuses frozen native Agent log indexes.
Absent native carriers remain absent. Shared unindexed scans cover at most 64
subjects, and one invocation consumes at most 500 nonempty effect rows.

`Custody`, `Sealed`, and `ObjectAccounting` remain declarations for subsequent
credential/object steps and preserve cleanup authority. The deletion receipt and
events remain under their existing retention contract. Mixed global controls
are preserved; only matching diagnostic bindings are removed. Supported foreign
document references and global model-budget overrides use exact revision checks.

C2 implements publisher majority fences, the durable transaction cleanup drain,
checkpointed exact reference discovery, immutable Profile retention, canonical
Trigger publisher tombstones, and their strict readers/admission checks.
Required retained Profile revisions preserve exact transitive pins; unused
publisher data is removed. New manual references refuse fences, while proven
surviving exact declarations may finish after canonical retention. These are
pending product-policy decisions and do not establish a complete deletion job.

Surviving foreign/organization identity, actor, channel, Bot custody and explicit
share-recipient references permanently refuse with
`IdentityRedactionPending { collection }` in this slice. The executor and public
reference facade preserve all affected sources. Generic share/receipt recipient
handling remains blocked through that facade until C3. Organization ownership
protection is active now; canonical audit identity rewriting and personal
delegation debt handoff are delivered together in C3.

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
| `agent_creation_provenance` | `Linked(Agents, &["_id"])`; `Reference(Identities)` | yes; no | agent_provenance.rs record_creation uses Agent ID as _id; no secondary owner index; surviving audit-actor rewriting is deferred to C3 |
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
| `bots` | `Custody(&["owner"])`; `Reference(Identities)` | yes; no | bots.rs BotDocument owner projection and owner/creation-key index; custody remains for credential cleanup; surviving identity/delegation rewriting is deferred to C3 |
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
| `resource_policies` | `Linked(Sandboxes, &["_id"])`; `Reference(Identities)` | yes; no | resource_access/records.rs policy_document; parent closure excludes owner_kind organization; retained policy-actor rewriting is deferred to C3 |
| `resource_share_commands` | `Linked(Sandboxes, &["sandbox_id"])`; `Reference(ShareReceipts)`; `Reference(Identities)` | no; no; no | resource_access/records.rs receipt_document; embedded share recipients and actor attribution; no sandbox_id-leading receipt index; the identity-aware facade permanently refuses affected retained rows until C3 |
| `resource_shares` | `Linked(Sandboxes, &["sandbox_id"])`; `Reference(Shares)`; `Reference(Shares)`; `Reference(Identities)` | yes; yes; no; no | resource_access/records.rs share_document and sandbox selector index; accepted_subjects has a leading index; embedded audience subjects use shared scanning; recipient/actor handling through the facade is deferred to C3 |
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

## Billing and deployment boundary

Unsettled funding and undelivered billing claims block usage erasure. Billing
enabled also requires delivery of unclaimed usage events; disabled billing may
erase unclaimed events. Orphan billing state remains attributable through
`owner_id`; a stopped settled state may retain its historical current receipt.
Partial progress always accompanies a later error. Deterministic unattributable
corruption is a permanent cohort block; clock/driver failure remains retryable.

This catalog is a reviewable persistence implementation, not evidence of a
production census, completed account deletion, remote revocation, or zero
residue. Worker scheduling and the full pipeline remain later deliveries. See
the C2 migration/reader-floor section in
[CMA Account Deletion](CMA_Account_Deletion.md). Preserve exact compatible
executables, indexes, markers, cursors and cleanup authority during recovery.
No downgrade below the recorded C2 floor is certified after its writes.

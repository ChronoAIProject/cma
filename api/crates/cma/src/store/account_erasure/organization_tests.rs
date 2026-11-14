//! Organization and surviving-owner identity references use actual writer shapes.
use super::references::{redact, Redaction};
use super::tests::{count, database, drain, put, sandbox, NOW};
use super::*;
use crate::store::resource_access::records;
use futures_util::TryStreamExt;
use layer2_core::ids::AgentCommandId;
use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use layer2_core::resource_access::*;
use layer2_core::sandbox::{
    agent_io::*,
    binding::*,
    message_feedback::{MessageFeedbackStore, MessageVote},
    suggestions::{SuggestionContext, SuggestionStatus, SuggestionWork, STRATEGY_VERSION},
    AgentCreationOrigin, AgentCreationProvenance, AgentOrigin,
};
use layer2_core::{AgentId, SandboxId, WorkspaceId, WorktreeId};
use mongodb::bson::{self, doc, Document};
use std::collections::BTreeSet;

fn actor(subject: &str) -> ResourceActor {
    ResourceActor::Api {
        subject: SubjectId::new(subject),
        delegation_id: None,
    }
}
fn subjects() -> BTreeSet<String> {
    BTreeSet::from(["erased".into()])
}
fn policy() -> ResourcePolicyRecord {
    ResourcePolicyRecord {
        sandbox_id: SandboxId("sbx_org".into()),
        sandbox_created_at: "2026-09-05T00:00:00Z".into(),
        owner: ResourceOwner::Organization {
            subject: SubjectId::new("organization"),
        },
        organization_evidence: Some(OrganizationOwnershipEvidence {
            issuer: "cma:authenticated-provider-binding-manager".into(),
            organization: SubjectId::new("organization"),
            admitted_by: actor("erased"),
            verified_at: NOW.into(),
        }),
        revision: 1,
    }
}
fn binding() -> ProviderBinding {
    let value = serde_json::json!({
        "schema_version":1,
        "metadata":{"name":"organization-binding","owner":{"kind":"organization","subject":"organization"},"provider":"registered"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"supported","model":"test","thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"},"context":{},
        "environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}
    });
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    let plan = map_profile(
        value,
        &digest,
        AgentProvider::default(),
        "supported",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap();
    let mut binding = ProviderBinding::new(
        "organization-binding".into(),
        SubjectId::new("erased"),
        plan,
        "a".repeat(64),
        NOW.into(),
    )
    .unwrap();
    binding.sandbox_id = SandboxId("sbx_org".into());
    binding
}
fn command() -> AdmittedAgentCommand {
    let command = AgentCommand::Interrupt;
    AdmittedAgentCommand {
        queued_input: None,
        receipt: AdmissionReceipt {
            envelope: AgentCommandEnvelope {
                continuation: None,
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: AgentCommandId("command-other-agent".into()),
                agent_id: AgentId("agt_org".into()),
                owner: ResourceOwner::Organization {
                    subject: SubjectId::new("organization"),
                },
                actor: actor("erased"),
                source: AgentCommandSource {
                    kind: AgentSourceKind::Api,
                    reference: "audit-source".into(),
                },
                correlation_id: "organization-command".into(),
                request_fingerprint: command.fingerprint().unwrap(),
                authorization: AgentAuthorizationRevision {
                    authority_id: "organization-authority".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
                command,
            },
            response_id: None,
            admitted_at: NOW.into(),
            agent_order: 1,
            event_cursor: 0,
        },
        key_sha256: "a".repeat(64),
        queue_deadline: "2026-11-14T00:05:00Z".into(),
        status: AgentResponseStatus::Queued,
        revision: 1,
        target_turn: None,
        work: Default::default(),
    }
}
fn provenance() -> AgentCreationProvenance {
    AgentCreationProvenance {
        schema_version: 1,
        agent_id: AgentId("agt_org".into()),
        agent_created_at: NOW.into(),
        sandbox_id: SandboxId("sbx_org".into()),
        sandbox_created_at: policy().sandbox_created_at,
        workspace_id: WorkspaceId("wsp_org".into()),
        worktree_id: WorktreeId("wtr_org".into()),
        owner: ResourceOwner::Organization {
            subject: SubjectId::new("organization"),
        },
        creation_origin: AgentCreationOrigin::Manual,
        created_by: Some(actor("erased")),
        provisioning_origin: AgentOrigin::Manual,
        creation_fingerprint: "a".repeat(64),
    }
}

async fn row(db: &Database, name: &str, id: &str) -> Document {
    db.collection::<Document>(name)
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn real_organization_rows_and_audits_survive_admin_identity_erasure() {
    let db = database().await;
    sandbox(&db, "organization", "sbx_org").await;
    let original_policy = records::policy_document(&policy()).unwrap();
    put(&db, "resource_policies", original_policy).await;
    let sandbox =
        crate::store::sandboxes::read_sandbox(&row(&db, "sandboxes", "sbx_org").await).unwrap();
    let mut agent = crate::store::sandbox_maintenance::mongo_integration_tests::ready_agent(
        &sandbox, "agt_org",
    );
    let provenance = provenance();
    agent.workspace_id = provenance.workspace_id.clone();
    agent.worktree_id = provenance.worktree_id.clone();
    agent.created_at = provenance.agent_created_at.clone();
    agent.creation_key = "manual:agt_org".into();
    agent.creation_fingerprint = provenance.creation_fingerprint.clone();
    put(
        &db,
        "agents",
        crate::store::agents::agent_document(&agent).unwrap(),
    )
    .await;
    let binding = binding();
    put(&db, "provider_bindings", doc! {
        "_id":&binding.binding_id,"document_schema_version":1_i64,"revision":1_i64,"generation":1_i64,"pending":false,
        "record":bson::serialize_to_bson(&binding).unwrap(),
    }).await;
    put(
        &db,
        "agent_creation_provenance",
        crate::store::agent_provenance::document(&provenance).unwrap(),
    )
    .await;
    put(
        &db,
        "agent_commands",
        crate::store::agent_commands::records::document(&command()).unwrap(),
    )
    .await;
    let audit = CommandAuthorizationAudit::new(
        &command(),
        CommandAuthorizationEffect::Admission,
        vec![ResourceAuthorizationAudit {
            target: ResourceTarget::Agent {
                agent_id: AgentId("agt_org".into()),
            },
            sandbox_id: SandboxId("sbx_org".into()),
            sandbox_created_at: policy().sandbox_created_at,
            owner: policy().owner,
            actor: actor("erased"),
            policy_revision: 1,
            capability: ResourceCapability::OperateAgent,
            captured_at: NOW.into(),
            observations: vec![ResourceAuthorityObservation {
                capabilities: vec![ResourceCapability::OperateAgent],
                basis: ResourceAuthorizationBasis::GlobalOperator,
            }],
        }],
    )
    .unwrap();
    put(&db,"command_authorization_audits",doc! {"_id":&audit.id,"document_schema_version":1_i64,"command_id":audit.command_id.as_str(),"record":bson::serialize_to_bson(&audit).unwrap()}).await;

    // Historical organization audits have no current mutation writer. Keep the
    // immutable event and its owner even when its configuring person is erased.
    put(
        &db,
        "resource_share_commands",
        doc! {
            "_id":"organization-audit","document_schema_version":1_i64,"sandbox_id":"sbx_org",
            "record":{"actor":bson::serialize_to_bson(&actor("erased")).unwrap(),
            "share":{"owner":{"kind":"organization","subject":"organization"},
            "issuer_actor":bson::serialize_to_bson(&actor("erased")).unwrap()},"audit":"keep"},
        },
    )
    .await;
    let feedback = crate::store::message_feedback::MongoMessageFeedbackStore::open(&db)
        .await
        .unwrap();
    for (owner, id, answer) in [
        ("erased", "personal", "personal"),
        ("survivor", "other", "keep"),
    ] {
        feedback
            .set(&SubjectId::new(owner), &agent.id, 2, Some(MessageVote::Up))
            .await
            .unwrap();
        let work = SuggestionWork {
            id: id.into(),
            owner: SubjectId::new(owner),
            agent_id: agent.id.clone(),
            scope: AnswerContext {
                turn_sequence: 1,
                answer_sequence: 2,
            },
            completed_sequence: 3,
            strategy_version: STRATEGY_VERSION.into(),
            authority: "a".repeat(64),
            status: SuggestionStatus::Pending,
            context: SuggestionContext {
                goal: "Review the result".into(),
                answer: answer.into(),
                confirmed_resources: vec![],
            },
            set: None,
            created_at_ms: 1,
            started_at_ms: None,
            deadline_ms: None,
            observation: None,
        };
        put(
            &db,
            "agent_suggestions",
            crate::store::suggestions::document(&work).unwrap(),
        )
        .await;
    }
    let survivor_feedback = db
        .collection::<Document>("agent_message_feedback")
        .find_one(doc! {"owner":"survivor"})
        .await
        .unwrap()
        .unwrap();
    let survivor_suggestion = row(&db, "agent_suggestions", "other").await;
    drain(&db, "erased", &mut ErasureInventory::default()).await;
    for name in [
        "sandboxes",
        "agents",
        "resource_policies",
        "provider_bindings",
        "agent_creation_provenance",
        "agent_commands",
        "resource_share_commands",
    ] {
        assert_eq!(count(&db, name).await, 1, "{name}");
    }
    let stored_policy =
        records::read_policy(&row(&db, "resource_policies", "sbx_org").await).unwrap();
    assert_eq!(stored_policy.owner, policy().owner);
    assert_eq!(
        stored_policy.organization_evidence.unwrap().admitted_by,
        actor("deleted-account")
    );
    assert_eq!(stored_policy.revision, 2);
    let binding_row = row(&db, "provider_bindings", &binding.binding_id).await;
    let stored_binding: ProviderBinding =
        bson::deserialize_from_bson(binding_row.get("record").unwrap().clone()).unwrap();
    stored_binding.validate().unwrap();
    assert_eq!(stored_binding.manager, SubjectId::new("deleted-account"));
    assert_eq!(stored_binding.owner, SubjectId::new("organization"));
    assert_eq!(stored_binding.profile, binding.profile);
    assert_eq!(stored_binding.revision, 2);
    let store = crate::store::sandbox_mongo::MongoSandboxStore::open(db.client(), db.name())
        .await
        .unwrap();
    assert_eq!(
        store.binding(&binding.binding_id).await.unwrap(),
        Some(stored_binding)
    );
    let stored_provenance = crate::store::agent_provenance::read(
        &row(&db, "agent_creation_provenance", "agt_org").await,
    )
    .unwrap();
    assert_eq!(stored_provenance.created_by, Some(actor("deleted-account")));
    assert_eq!(stored_provenance.owner, provenance.owner);
    let stored_command = crate::store::agent_commands::records::read(
        &row(&db, "agent_commands", "command-other-agent").await,
    )
    .unwrap();
    assert_eq!(
        stored_command.receipt.envelope.actor,
        actor("deleted-account")
    );
    assert_eq!(
        stored_command.receipt.envelope.owner,
        command().receipt.envelope.owner
    );
    let audit_row = row(&db, "command_authorization_audits", &audit.id).await;
    let stored_audit: CommandAuthorizationAudit =
        bson::deserialize_from_bson(audit_row.get("record").unwrap().clone()).unwrap();
    stored_audit.validate().unwrap();
    assert_eq!(stored_audit.actor, actor("deleted-account"));
    assert_eq!(stored_audit.resources[0].actor, stored_audit.actor);
    let audit = row(&db, "resource_share_commands", "organization-audit").await;
    assert_eq!(
        super::selectors::strings(&audit, "record.actor.subject"),
        ["deleted-account"]
    );
    assert_eq!(
        super::selectors::strings(&audit, "record.share.issuer_actor.subject"),
        ["deleted-account"]
    );
    assert_eq!(super::selectors::strings(&audit, "record.audit"), ["keep"]);
    for (name, original) in [
        ("agent_message_feedback", survivor_feedback),
        ("agent_suggestions", survivor_suggestion),
    ] {
        assert_eq!(count(&db, name).await, 1, "{name}");
        assert_eq!(
            row(&db, name, original.get_str("_id").unwrap()).await,
            original
        );
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_identity_redaction_compares_the_original_revision_and_record() {
    let db = database().await;
    let stale = records::policy_document(&policy()).unwrap();
    put(&db, "resource_policies", stale.clone()).await;
    db.collection::<Document>("resource_policies").update_one(doc! {"_id":"sbx_org"},doc! {"$set":{"revision":2_i64,"record.revision":2_i64,"record.organization_evidence.issuer":"concurrent-issuer"}}).await.unwrap();
    assert_eq!(
        redact(&db, "resource_policies", &stale, &subjects())
            .await
            .unwrap(),
        Redaction::Conflict
    );
    let current = row(&db, "resource_policies", "sbx_org").await;
    assert_eq!(
        super::selectors::strings(&current, "record.organization_evidence.admitted_by.subject"),
        ["erased"]
    );
    assert_eq!(
        redact(&db, "resource_policies", &current, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = records::read_policy(&row(&db, "resource_policies", "sbx_org").await).unwrap();
    assert_eq!(cleaned.revision, 3);
    assert_eq!(
        cleaned.organization_evidence.unwrap().issuer,
        "concurrent-issuer"
    );
    db.drop().await.unwrap();
}

fn recipient_receipt() -> ShareMutationReceipt {
    ShareMutationReceipt {
        operation_id: "a".repeat(64),
        command_fingerprint: "b".repeat(64),
        sandbox_id: SandboxId("sbx_survivor".into()),
        sandbox_created_at: NOW.into(),
        share: ResourceShare {
            id: "survivor-share".into(),
            owner: ResourceOwner::Person {
                subject: SubjectId::new("survivor"),
            },
            selector: ResourceSelector::Agent {
                agent_id: AgentId("agt_survivor".into()),
            },
            target_org_id: SubjectId::new(""),
            link: Some(AgentShareLink {
                audience: AgentShareAudience::Public,
                token_sha256: "c".repeat(64),
                accepted_subjects: vec![SubjectId::new("erased"), SubjectId::new("other")],
            }),
            mode: ShareMode::View,
            issuer_actor: actor("survivor"),
            created_at: NOW.into(),
            revoked_at: None,
            revision: 1,
        },
        actor: actor("erased"),
        at: NOW.into(),
        policy_revision: 1,
    }
}
#[test]
fn shared_deleted_subject_is_accepted_only_for_canonical_retained_receipts() {
    let mut retained = recipient_receipt();
    retained.actor = actor(layer2_core::DELETED_ACCOUNT_SUBJECT);
    let mut document = records::retained_receipt_document(&retained, true).unwrap();
    document.insert("mutation_version", 1_i64);
    assert_eq!(records::read_receipt(&document).unwrap(), retained);

    retained.actor = ResourceActor::Api {
        subject: SubjectId::new(layer2_core::DELETED_ACCOUNT_SUBJECT),
        delegation_id: Some("retained-grant".into()),
    };
    assert!(records::retained_receipt_document(&retained, true).is_err());
    retained.actor = actor("deleted-account-other");
    assert!(records::retained_receipt_document(&retained, true).is_err());
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn erased_share_recipient_audit_uses_a_canonical_readable_actor_tombstone() {
    let db = database().await;
    let mut retained = recipient_receipt();
    let request = ShareMutation {
        selector: retained.share.selector.clone(),
        owner: retained.share.owner.clone(),
        actor: retained.actor.clone(),
        key_sha256: "d".repeat(64),
        change: ShareChange::RedeemLink {
            share_id: retained.share.id.clone(),
            token_sha256: "c".repeat(64),
            subject: SubjectId::new("erased"),
        },
        expected_share_revision: Some(1),
        policy_fence: ResourcePolicyFence {
            sandbox_id: retained.sandbox_id.clone(),
            sandbox_created_at: retained.sandbox_created_at.clone(),
            policy_revision: 1,
        },
        proposed_id: "unused-share".into(),
        at: NOW.into(),
    };
    request.validate().unwrap();
    retained.operation_id = request.operation_id();
    retained.command_fingerprint = request.fingerprint();
    retained.share.revision = 2;
    retained.policy_revision = 2;
    let original = records::receipt_document(&retained).unwrap();
    put(&db, "resource_share_commands", original.clone()).await;
    let store = crate::store::sandbox_mongo::MongoSandboxStore::open(db.client(), db.name())
        .await
        .unwrap();
    assert_eq!(store.mutate_share(request.clone()).await.unwrap(), retained);
    assert_eq!(
        redact(&db, "resource_share_commands", &original, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = row(&db, "resource_share_commands", &retained.operation_id).await;
    let receipt = records::read_receipt(&cleaned).unwrap();
    assert_eq!(receipt.actor, actor("deleted-account"));
    assert_eq!(receipt.share.owner, recipient_receipt().share.owner);
    assert_eq!(
        receipt.share.link.unwrap().accepted_subjects,
        [SubjectId::new("other")]
    );
    let mut malformed = cleaned.clone();
    malformed.remove("mutation_version");
    assert!(records::read_receipt(&malformed).is_err());
    assert_eq!(
        store.mutate_share(request).await.unwrap_err(),
        ResourceAccessStoreError::IdempotencyConflict
    );
    malformed.get_document_mut("record").unwrap().insert(
        "actor",
        bson::serialize_to_bson(&actor("stranger")).unwrap(),
    );
    malformed.insert("mutation_version", 1_i64);
    assert!(records::read_receipt(&malformed).is_err());
    db.drop().await.unwrap();
}

async fn channel_fixture(
    db: &Database,
) -> (crate::store::sandbox_mongo::MongoSandboxStore, String) {
    use layer2_core::profiles::{AgentProfileSnapshot, WorkspaceProfileSnapshot};
    use layer2_core::sandbox::{channel_usage::*, *};
    let binding = binding();
    let mut parent = crate::store::sandbox_mongo_integration_tests::sandbox("sbx_org");
    parent.owner = SubjectId::new("organization");
    let environment = binding.profile.environment.as_ref().unwrap();
    let workspace = Workspace {
        id: WorkspaceId("wsp_org".into()),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: layer2_core::WorkspaceProfileId("wkp_org".into()),
            revision: 1,
            digest: content_digest(&environment.workspace).unwrap(),
            content: environment.workspace.clone(),
        },
        name: "organization".into(),
        path: "/home/cma/organization".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: None,
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    };
    let worktree = Worktree {
        id: WorktreeId("wtr_org".into()),
        workspace_id: workspace.id.clone(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/organization/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "a".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(AgentId("agt_org".into())),
        revision: 1,
    };
    let agent = Agent {
        id: AgentId("agt_org".into()),
        worktree_id: worktree.id.clone(),
        workspace_id: workspace.id.clone(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        creation_key: "organization-agent".into(),
        creation_fingerprint: "a".repeat(64),
        profile: AgentProfileSnapshot {
            id: layer2_core::AgentProfileId("agp_org".into()),
            revision: 1,
            digest: content_digest(&binding.profile.agent).unwrap(),
            content: binding.profile.agent.clone(),
        },
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        status: AgentStatus::Starting,
        conversation: ConversationState {
            observations: Default::default(),
            settings: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            conversation_id: None,
            runtime_generation: None,
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::AddWorktree, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    };
    let creation = provenance();
    creation.validate_against(&agent, &parent).unwrap();
    put(
        db,
        "sandboxes",
        crate::store::sandboxes::sandbox_document(&parent, 0).unwrap(),
    )
    .await;
    put(
        db,
        "workspaces",
        crate::store::workspaces::workspace_document(&workspace).unwrap(),
    )
    .await;
    put(
        db,
        "worktrees",
        crate::store::worktrees::worktree_document(&worktree).unwrap(),
    )
    .await;
    put(
        db,
        "agents",
        crate::store::agents::agent_document(&agent).unwrap(),
    )
    .await;
    put(
        db,
        "resource_policies",
        records::policy_document(&policy()).unwrap(),
    )
    .await;
    put(
        db,
        "agent_creation_provenance",
        crate::store::agent_provenance::document(&creation).unwrap(),
    )
    .await;
    let before = AgentChannelUsageState::empty(creation.clone());
    let change = ChannelUsageChange::Attach {
        binding: ChannelBindingReference {
            binding_id: "historical-binding".into(),
            bridge_id: "historical-bridge".into(),
            partition_id: "historical-partition".into(),
            generation: 1,
            previous_binding_id: None,
        },
    };
    let reason = ChannelBindingChangeReason::Attachment;
    let mut after = before.clone();
    after.revision = 1;
    after.last_changed_at = Some(NOW.into());
    after.active_bindings.push(ActiveChannelBinding {
        reference: change.introduced_binding().unwrap().clone(),
        attached_at: NOW.into(),
        first_used_at: None,
        last_used_at: None,
    });
    let input = ChannelUsageMutation {
        creation: creation.clone(),
        policy_fence: ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 1,
        },
        expected_revision: 0,
        key_sha256: "d".repeat(64),
        actor: actor("erased"),
        change: change.clone(),
        reason,
        at: NOW.into(),
    };
    let event:ChannelUsageEvent=serde_json::from_value(serde_json::json!({"operation_id":input.operation_id(),"fingerprint":input.fingerprint(),"actor":input.actor,"at":NOW,"detached_at":null,"reason":reason,"change":change,"predecessor":null,"before":before,"after":after})).unwrap();
    event.validate().unwrap();
    let digest = event.digest();
    put(db,"channel_usage_events",doc! {"_id":&event.operation_id,"document_schema_version":1_i64,"agent_id":"agt_org","revision":1_i64,"binding_id":"historical-binding","digest":&digest,"record":bson::serialize_to_bson(&event).unwrap()}).await;
    let before = event.after.clone();
    let mut after = before.clone();
    after.revision = 2;
    after.first_used_at = Some(NOW.into());
    after.last_used_at = Some(NOW.into());
    after.active_bindings[0].first_used_at = Some(NOW.into());
    after.active_bindings[0].last_used_at = Some(NOW.into());
    let input = ChannelUsageMutation {
        creation: creation.clone(),
        policy_fence: ResourcePolicyFence {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            policy_revision: 1,
        },
        expected_revision: 1,
        key_sha256: "e".repeat(64),
        actor: actor("survivor"),
        change: ChannelUsageChange::Use {
            binding_id: "historical-binding".into(),
        },
        reason: ChannelBindingChangeReason::AcceptedCommand,
        at: NOW.into(),
    };
    let second:ChannelUsageEvent=serde_json::from_value(serde_json::json!({"operation_id":input.operation_id(),"fingerprint":input.fingerprint(),"actor":input.actor,"at":NOW,"detached_at":null,"reason":input.reason,"change":input.change,"predecessor":null,"before":before,"after":after})).unwrap();
    second.validate().unwrap();
    put(db,"channel_usage_events",doc! {"_id":&second.operation_id,"document_schema_version":1_i64,"agent_id":"agt_org","revision":2_i64,"digest":second.digest(),"record":bson::serialize_to_bson(&second).unwrap()}).await;
    let stored = StoredAgentChannelUsage {
        state: second.after.clone(),
        latest_event_digest: second.digest(),
    };
    put(db,"agent_channel_usage",doc! {"_id":"agt_org","document_schema_version":1_i64,"revision":2_i64,"active":true,"record":bson::serialize_to_bson(&stored).unwrap()}).await;
    let store = crate::store::sandbox_mongo::MongoSandboxStore::open(db.client(), db.name())
        .await
        .unwrap();
    (store, digest)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn channel_identity_erasure_keeps_current_snapshot_and_history_readable_in_every_phase() {
    use layer2_core::sandbox::store::{AgentChannelUsageStore, PageRequest};
    for order in [
        [
            "channel_usage_events",
            "agent_channel_usage",
            "agent_creation_provenance",
        ],
        [
            "agent_creation_provenance",
            "agent_channel_usage",
            "channel_usage_events",
        ],
    ] {
        let db = database().await;
        let (store, digest) = channel_fixture(&db).await;
        let id = AgentId("agt_org".into());
        store.agent_channel_usage_snapshot(&id).await.unwrap();
        for name in order {
            let originals: Vec<Document> = db
                .collection::<Document>(name)
                .find(doc! {})
                .sort(doc! {"revision":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            for original in originals {
                assert_eq!(
                    redact(&db, name, &original, &subjects()).await.unwrap(),
                    Redaction::Updated,
                    "{name}"
                );
                store.agent_channel_usage_snapshot(&id).await.unwrap();
                let (_, first) = store
                    .agent_channel_usage_history(&id, &PageRequest::new(1, None).unwrap())
                    .await
                    .unwrap();
                store
                    .agent_channel_usage_history(&id, &PageRequest::new(1, first.next).unwrap())
                    .await
                    .unwrap();
            }
        }
        let (snapshot, history) = store
            .agent_channel_usage_history(&id, &PageRequest::new(10, None).unwrap())
            .await
            .unwrap();
        assert_eq!(
            snapshot.state.creation.created_by,
            Some(actor("deleted-account"))
        );
        assert_eq!(history.items[0].actor, actor("deleted-account"));
        assert_eq!(history.items[1].actor, actor("survivor"));
        assert_eq!(history.items[0].digest(), digest);
        let mut mutated = history.items[0].clone();
        mutated.after.creation.creation_fingerprint = "b".repeat(64);
        assert!(mutated.validate().is_err());
        assert_ne!(mutated.digest(), digest);
        let event_row = db
            .collection::<Document>("channel_usage_events")
            .find_one(doc! {"revision":1_i64})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event_row.get_str("digest").unwrap(), digest);
        db.collection::<Document>("channel_usage_events")
            .update_one(
                doc! {"_id":event_row.get("_id").unwrap().clone()},
                doc! {"$set":{"redacted_digest":"a".repeat(64)}},
            )
            .await
            .unwrap();
        assert!(store
            .agent_channel_usage_history(&id, &PageRequest::new(10, None).unwrap())
            .await
            .is_err());
        db.collection::<Document>("channel_usage_events")
            .replace_one(
                doc! {"_id":event_row.get("_id").unwrap().clone()},
                event_row.clone(),
            )
            .await
            .unwrap();
        db.collection::<Document>("channel_usage_events")
            .update_one(
                doc! {"_id":event_row.get("_id").unwrap().clone()},
                doc! {"$unset":{"mutation_version":""}},
            )
            .await
            .unwrap();
        assert!(store
            .agent_channel_usage_history(&id, &PageRequest::new(10, None).unwrap())
            .await
            .is_err());
        db.drop().await.unwrap();
    }
}

fn organization_control() -> layer2_core::sandbox::control::SandboxControlRecord {
    use layer2_core::sandbox::control::*;
    let control = SandboxControl::Pause;
    let audit = ResourceAuthorizationAudit {
        target: ResourceTarget::Sandbox {
            sandbox_id: policy().sandbox_id.clone(),
        },
        sandbox_id: policy().sandbox_id,
        sandbox_created_at: policy().sandbox_created_at,
        owner: policy().owner,
        actor: actor("erased"),
        policy_revision: 1,
        capability: ResourceCapability::ManageSandbox,
        captured_at: NOW.into(),
        observations: vec![ResourceAuthorityObservation {
            capabilities: vec![ResourceCapability::ManageSandbox],
            basis: ResourceAuthorizationBasis::GlobalOperator,
        }],
    };
    let record = SandboxControlRecord {
        schema_version: 1,
        envelope: SandboxControlEnvelope {
            command_id: layer2_core::ids::SandboxCommandId("organization-pause".into()),
            sandbox_id: policy().sandbox_id,
            sandbox_created_at: policy().sandbox_created_at,
            owner: policy().owner,
            actor: actor("erased"),
            authorization: AgentAuthorizationRevision {
                authority_id: "organization-authority".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command: control,
            expected_revision: 1,
            request_fingerprint: control.fingerprint(1),
        },
        key_sha256: "a".repeat(64),
        admitted_at: NOW.into(),
        queue_deadline: "2026-11-14T00:05:00Z".into(),
        status: SandboxControlStatus::Applied {
            at: NOW.into(),
            sandbox_revision: 2,
            disposition: SandboxControlDisposition::Scheduled,
        },
        revision: 2,
        admission_audit: audit.clone(),
        execution_audit: Some(audit),
    };
    record.validate().unwrap();
    record
}

fn control_row(record: &layer2_core::sandbox::control::SandboxControlRecord) -> Document {
    doc! {"_id":record.envelope.command_id.as_str(),"document_schema_version":1_i64,"sandbox_id":record.envelope.sandbox_id.as_str(),
    "scope":record.envelope.scope(),"key_sha256":&record.key_sha256,"queued":record.queued(),"revision":record.revision as i64,
    "queue_deadline":&record.queue_deadline,"record":bson::serialize_to_bson(record).unwrap()}
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_control_keeps_its_terminal_audits_and_rejects_stale_redaction() {
    use layer2_core::sandbox::control::SandboxControlStore;
    let db = database().await;
    sandbox(&db, "organization", "sbx_org").await;
    put(
        &db,
        "resource_policies",
        records::policy_document(&policy()).unwrap(),
    )
    .await;
    let control = organization_control();
    let stale = control_row(&control);
    put(&db, "sandbox_controls", stale.clone()).await;
    db.collection::<Document>("sandbox_controls")
        .update_one(
            doc! {"_id":control.envelope.command_id.as_str()},
            doc! {"$set":{"concurrent_evidence":"preserve"}},
        )
        .await
        .unwrap();
    assert_eq!(
        redact(&db, "sandbox_controls", &stale, &subjects())
            .await
            .unwrap(),
        Redaction::Conflict
    );
    let current = row(
        &db,
        "sandbox_controls",
        control.envelope.command_id.as_str(),
    )
    .await;
    assert_eq!(
        redact(&db, "sandbox_controls", &current, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let stored = row(
        &db,
        "sandbox_controls",
        control.envelope.command_id.as_str(),
    )
    .await;
    assert_eq!(stored.get_str("concurrent_evidence").unwrap(), "preserve");
    let store = crate::store::sandbox_mongo::MongoSandboxStore::open(db.client(), db.name())
        .await
        .unwrap();
    let cleaned = store
        .sandbox_control(&control.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    cleaned.validate().unwrap();
    assert_eq!(cleaned.envelope.owner, control.envelope.owner);
    assert_eq!(cleaned.status, control.status);
    assert_eq!(cleaned.envelope.actor, actor("deleted-account"));
    assert_eq!(cleaned.admission_audit.actor, cleaned.envelope.actor);
    assert_eq!(
        cleaned.execution_audit.unwrap().actor,
        cleaned.envelope.actor
    );
    assert_ne!(cleaned.envelope.scope(), control.envelope.scope());
    assert_eq!(stored.get_str("scope").unwrap(), cleaned.envelope.scope());
    assert_eq!(count(&db, "sandboxes").await, 1);
    db.drop().await.unwrap();
}

fn organization_artifact() -> layer2_core::sandbox::agent_artifacts::StoredAgentArtifact {
    use layer2_core::sandbox::agent_artifacts::*;
    let record = StoredAgentArtifact {
        metadata: AgentArtifactMetadata {
            id: format!("art_{}", "a".repeat(64)),
            agent_id: AgentId("agt_org".into()),
            name: "organization.txt".into(),
            media_type: "text/plain".into(),
            size_bytes: 5,
            sha256: layer2_core::document_artifacts::digest(b"hello"),
            created_at: NOW.into(),
        },
        binding: ArtifactBinding {
            sandbox_id: policy().sandbox_id,
            sandbox_created_at: policy().sandbox_created_at,
            workspace_id: WorkspaceId("wsp_org".into()),
            workspace_created_at: NOW.into(),
            worktree_id: WorktreeId("wtr_org".into()),
            worktree_identity_sha256: "c".repeat(64),
            agent_id: AgentId("agt_org".into()),
            agent_created_at: NOW.into(),
            owner: policy().owner,
        },
        content_base64: "aGVsbG8=".into(),
        authorization: ResourceAuthorizationAudit {
            target: ResourceTarget::Agent {
                agent_id: AgentId("agt_org".into()),
            },
            sandbox_id: policy().sandbox_id,
            sandbox_created_at: policy().sandbox_created_at,
            owner: policy().owner,
            actor: actor("erased"),
            policy_revision: 1,
            capability: ResourceCapability::ManageSharing,
            captured_at: NOW.into(),
            observations: vec![ResourceAuthorityObservation {
                capabilities: vec![ResourceCapability::ManageSharing],
                basis: ResourceAuthorizationBasis::ShareRecipient {
                    subject: SubjectId::new("erased"),
                    share: ResourceGrantObservation {
                        id: "organization-grant".into(),
                        revision: 1,
                        mode: ShareMode::ViewWrite,
                        selector: ResourceSelector::Agent {
                            agent_id: AgentId("agt_org".into()),
                        },
                    },
                },
            }],
        },
    };
    record.content().unwrap();
    record
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_artifact_keeps_bytes_and_binding_and_rejects_stale_redaction() {
    use layer2_core::sandbox::agent_artifacts::AgentArtifactStore;
    let db = database().await;
    let artifact = organization_artifact();
    let stale = doc! {"_id":&artifact.metadata.id,"document_schema_version":1_i64,"agent_id":"agt_org",
    "binding":bson::serialize_to_bson(&artifact.binding).unwrap(),"record":bson::serialize_to_bson(&artifact).unwrap()};
    put(&db, "agent_artifacts", stale.clone()).await;
    db.collection::<Document>("agent_artifacts")
        .update_one(
            doc! {"_id":&artifact.metadata.id},
            doc! {"$set":{"concurrent_evidence":"preserve"}},
        )
        .await
        .unwrap();
    assert_eq!(
        redact(&db, "agent_artifacts", &stale, &subjects())
            .await
            .unwrap(),
        Redaction::Conflict
    );
    let current = row(&db, "agent_artifacts", &artifact.metadata.id).await;
    assert_eq!(
        redact(&db, "agent_artifacts", &current, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let store = crate::store::sandbox_mongo::MongoSandboxStore::open(db.client(), db.name())
        .await
        .unwrap();
    let cleaned = store
        .agent_artifact(&artifact.binding, &artifact.metadata.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleaned.content().unwrap(), b"hello");
    assert_eq!(cleaned.binding, artifact.binding);
    assert_eq!(cleaned.metadata, artifact.metadata);
    assert_eq!(cleaned.authorization.actor, actor("deleted-account"));
    assert!(
        matches!(&cleaned.authorization.observations[0].basis,ResourceAuthorizationBasis::ShareRecipient {subject,..} if subject.as_str()=="deleted-account")
    );
    assert_eq!(
        row(&db, "agent_artifacts", &artifact.metadata.id)
            .await
            .get_str("concurrent_evidence")
            .unwrap(),
        "preserve"
    );
    assert!(store
        .agent_artifact_metadata(&artifact.binding, &artifact.metadata.id)
        .await
        .unwrap()
        .is_some());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_bot_keeps_custody_identifiers_and_rejects_stale_redaction() {
    use layer2_core::bots::{BotOwnership, BotStore};
    let db = database().await;
    let store = crate::store::bots::MongoBotStore::open(db.client(), db.name())
        .await
        .unwrap();
    let mut bot = crate::store::bots_mongo_integration_tests::bot("organization", "a");
    bot.ownership = Some(BotOwnership {
        owner: policy().owner,
        created_by: ResourceActor::Api {
            subject: SubjectId::new("erased"),
            delegation_id: Some("organization-delegation".into()),
        },
        credential_subject: SubjectId::new("erased"),
        credential_incarnation: Some("organization-sign-in".into()),
        authorization_grant_id: Some("organization-delegation".into()),
    });
    bot.resources.credentials.agent_key_id = Some("organization-agent-key".into());
    bot.resources.credentials.route_key_id = Some("organization-route-key".into());
    bot.validate().unwrap();
    store.insert(&bot).await.unwrap();
    let stale = row(&db, "bots", &bot.bot_id).await;
    let mut concurrent = bot.clone();
    concurrent.revision = 2;
    concurrent.updated_at = NOW.into();
    store.replace(1, &concurrent).await.unwrap();
    assert_eq!(
        redact(&db, "bots", &stale, &subjects()).await.unwrap(),
        Redaction::Conflict
    );
    let current = row(&db, "bots", &bot.bot_id).await;
    assert_eq!(
        redact(&db, "bots", &current, &subjects()).await.unwrap(),
        Redaction::Updated
    );
    let cleaned = store.get(&bot.bot_id).await.unwrap().unwrap();
    cleaned.validate().unwrap();
    assert_eq!(cleaned.owner, SubjectId::new("organization"));
    assert_eq!(cleaned.resource_owner(), policy().owner);
    assert_eq!(
        cleaned.ownership.as_ref().unwrap().created_by,
        actor("deleted-account")
    );
    assert_eq!(
        cleaned.credential_subject(),
        &SubjectId::new("deleted-account")
    );
    assert_eq!(cleaned.credential_incarnation(), None);
    assert_eq!(cleaned.authorization_grant_id(), None);
    assert_eq!(cleaned.resources, bot.resources);
    assert_eq!(cleaned.updated_at, NOW);
    assert_eq!(cleaned.revision, 3);
    assert_eq!(cleaned.profiles, bot.profiles);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_resource_activity_survives_while_personal_activity_is_deleted() {
    use layer2_core::{
        ActivityAction, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityStore,
        ActivityTarget, DetailKey,
    };
    let db = database().await;
    sandbox(&db, "organization", "sbx_org").await;
    let mut ownership = policy();
    ownership
        .organization_evidence
        .as_mut()
        .unwrap()
        .admitted_by = actor("survivor");
    put(
        &db,
        "resource_policies",
        records::policy_document(&ownership).unwrap(),
    )
    .await;
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    let mut originals = vec![];
    for target in ["sandbox:sbx_org", "sandbox:sbx_org:events"] {
        let mut record = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
            .by(SubjectId::new("erased"))
            .on(ActivityTarget::Resource(target.into()));
        record.detail.record(DetailKey::Subject, "organization");
        store.append(&record).await.unwrap();
        originals.push(record);
    }
    let personal = ActivityRecord::new(ActivityAction::Delete, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Subject(SubjectId::new("erased")));
    store.append(&personal).await.unwrap();
    let generic = ActivityRecord::new(ActivityAction::Update, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Resource(
            "/v1/sandboxes/{id}/runtime-update".into(),
        ));
    store.append(&generic).await.unwrap();
    sandbox(&db, "foreign-person", "sbx_foreign").await;
    let foreign = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Resource("sandbox:sbx_foreign".into()));
    store.append(&foreign).await.unwrap();
    drain(&db, "erased", &mut ErasureInventory::default()).await;
    let retained = store.list(&ActivityQuery::default()).await.unwrap();
    assert_eq!(retained.items.len(), 2);
    for original in originals {
        let cleaned = retained
            .items
            .iter()
            .find(|record| record.id == original.id)
            .unwrap();
        assert_eq!(cleaned.actor, Some(SubjectId::new("deleted-account")));
        assert_eq!(cleaned.target, original.target);
        assert_eq!(cleaned.detail, original.detail);
        assert_eq!(cleaned.action, original.action);
    }
    assert_eq!(count(&db, "sandboxes").await, 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_resource_activity_compares_exact_evidence_before_redaction() {
    use layer2_core::{
        ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget,
    };
    let db = database().await;
    sandbox(&db, "organization", "sbx_org").await;
    let mut ownership = policy();
    ownership
        .organization_evidence
        .as_mut()
        .unwrap()
        .admitted_by = actor("survivor");
    put(
        &db,
        "resource_policies",
        records::policy_document(&ownership).unwrap(),
    )
    .await;
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    let record = ActivityRecord::new(ActivityAction::Update, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Resource("sandbox:sbx_org".into()));
    store.append(&record).await.unwrap();
    let stale = row(&db, "activity", &record.id.0).await;
    db.collection::<Document>("activity")
        .update_one(
            doc! {"_id":&record.id.0},
            doc! {"$set":{"record.detail.status":"concurrent-status"}},
        )
        .await
        .unwrap();
    assert_eq!(
        redact(&db, "activity", &stale, &subjects()).await.unwrap(),
        Redaction::Conflict
    );
    let current = row(&db, "activity", &record.id.0).await;
    assert_eq!(
        redact(&db, "activity", &current, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = row(&db, "activity", &record.id.0).await;
    assert_eq!(
        super::selectors::strings(&cleaned, "record.actor"),
        ["deleted-account"]
    );
    assert_eq!(
        super::selectors::strings(&cleaned, "record.detail.status"),
        ["concurrent-status"]
    );
    db.drop().await.unwrap();
}

fn historical_organization_share(collection: &str, explicit: bool) -> Document {
    let mut receipt = recipient_receipt();
    if explicit {
        let link = receipt.share.link.as_mut().unwrap();
        link.audience = AgentShareAudience::Users {
            subjects: vec![SubjectId::new("erased")],
        };
        link.accepted_subjects = vec![SubjectId::new("erased")];
    }
    let mut row = if collection == "resource_shares" {
        records::share_document(&receipt.sandbox_id, &receipt.share).unwrap()
    } else {
        records::receipt_document(&receipt).unwrap()
    };
    let record = row.get_document_mut("record").unwrap();
    let share = if collection == "resource_shares" {
        record
    } else {
        record.get_document_mut("share").unwrap()
    };
    share.insert("owner", bson::serialize_to_bson(&policy().owner).unwrap());
    share.insert(
        "issuer_actor",
        bson::serialize_to_bson(&actor("surviving-admin")).unwrap(),
    );
    share.insert("retained_evidence", "keep");
    row
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn historical_organization_share_recipients_are_removed_with_exact_cas() {
    for name in ["resource_shares", "resource_share_commands"] {
        for explicit in [false, true] {
            let db = database().await;
            let stale = historical_organization_share(name, explicit);
            let id = stale.get_str("_id").unwrap();
            put(&db, name, stale.clone()).await;
            db.collection::<Document>(name)
                .update_one(
                    doc! {"_id":id},
                    doc! {"$set":{"concurrent_evidence":"preserve"}},
                )
                .await
                .unwrap();
            assert_eq!(
                redact(&db, name, &stale, &subjects()).await.unwrap(),
                Redaction::Conflict
            );
            let current = row(&db, name, id).await;
            assert_eq!(
                redact(&db, name, &current, &subjects()).await.unwrap(),
                Redaction::Updated
            );
            let cleaned = row(&db, name, id).await;
            let base = if name == "resource_shares" {
                "record"
            } else {
                "record.share"
            };
            assert_eq!(
                super::selectors::strings(&cleaned, &format!("{base}.owner.subject")),
                ["organization"]
            );
            assert_eq!(
                super::selectors::strings(&cleaned, &format!("{base}.issuer_actor.subject")),
                ["surviving-admin"]
            );
            assert_eq!(
                super::selectors::strings(&cleaned, &format!("{base}.retained_evidence")),
                ["keep"]
            );
            assert!(!super::selectors::strings(
                &cleaned,
                &format!("{base}.link.accepted_subjects")
            )
            .contains(&"erased"));
            assert!(!super::selectors::strings(
                &cleaned,
                &format!("{base}.link.audience.subjects")
            )
            .contains(&"erased"));
            if explicit {
                assert!(
                    !super::selectors::strings(&cleaned, &format!("{base}.revoked_at")).is_empty()
                );
            } else {
                assert!(super::selectors::strings(
                    &cleaned,
                    &format!("{base}.link.accepted_subjects")
                )
                .contains(&"other"));
            }
            assert_eq!(cleaned.get_str("concurrent_evidence").unwrap(), "preserve");
            if name == "resource_share_commands" {
                assert_eq!(
                    super::selectors::strings(&cleaned, "record.actor.subject"),
                    ["deleted-account"]
                );
            }
            db.drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_historical_organization_share_is_refused_without_mutation() {
    let db = database().await;
    let mut malformed = historical_organization_share("resource_shares", false);
    malformed
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("link")
        .unwrap()
        .insert(
            "accepted_subjects",
            vec![bson::Bson::String("erased".into()), bson::Bson::Int64(7)],
        );
    put(&db, "resource_shares", malformed.clone()).await;
    assert!(matches!(
        redact(&db, "resource_shares", &malformed, &subjects()).await,
        Err(Error::InvalidRecord("resource_shares"))
    ));
    assert_eq!(
        row(&db, "resource_shares", malformed.get_str("_id").unwrap()).await,
        malformed
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_bot_delegation_is_handed_off_before_personal_custody_is_erased() {
    use layer2_core::account_deletion::DelegationDebt;
    use layer2_core::bots::{BotOwnership, BotStore};
    let db = database().await;
    let store = crate::store::bots::MongoBotStore::open(db.client(), db.name())
        .await
        .unwrap();
    let mut bot = crate::store::bots_mongo_integration_tests::bot("organization", "a");
    bot.ownership = Some(BotOwnership {
        owner: policy().owner,
        created_by: ResourceActor::Api {
            subject: SubjectId::new("erased"),
            delegation_id: Some("organization-delegation".into()),
        },
        credential_subject: SubjectId::new("erased"),
        credential_incarnation: Some("organization-sign-in".into()),
        authorization_grant_id: Some("organization-delegation".into()),
    });
    store.insert(&bot).await.unwrap();
    let original = row(&db, "bots", &bot.bot_id).await;
    let subject = SubjectId::new("erased");
    let mut inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    let batch = erase_records(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
    )
    .await
    .unwrap();
    let debt = DelegationDebt {
        bot_id: bot.bot_id.clone(),
        authorization_grant_id: "organization-delegation".into(),
        credential_incarnation: "organization-sign-in".into(),
    };
    assert_eq!(
        batch.pending_delegations.get("erased"),
        Some(&vec![debt.clone()])
    );
    assert!(batch.waiting);
    assert_eq!(row(&db, "bots", &bot.bot_id).await, original);
    // A second call without durable acknowledgement must return the same debt.
    let retry = erase_records(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
    )
    .await
    .unwrap();
    assert_eq!(
        retry.pending_delegations.get("erased"),
        Some(&vec![debt.clone()])
    );
    assert_eq!(row(&db, "bots", &bot.bot_id).await, original);
    inventory.pending_delegations.push(DelegationDebt {
        credential_incarnation: "different-sign-in".into(),
        ..debt.clone()
    });
    let incorrect = erase_records(
        &db,
        &[Target {
            subject: &subject,
            inventory: &inventory,
        }],
        &mut cursor,
        NOW,
    )
    .await
    .unwrap();
    assert_eq!(
        incorrect.pending_delegations.get("erased"),
        Some(&vec![debt.clone()])
    );
    assert_eq!(row(&db, "bots", &bot.bot_id).await, original);
    inventory.pending_delegations.clear();
    inventory.pending_delegations.push(debt.clone());
    drain(&db, "erased", &mut inventory).await;
    let cleaned = store.get(&bot.bot_id).await.unwrap().unwrap();
    assert_eq!(
        cleaned.credential_subject(),
        &SubjectId::new("deleted-account")
    );
    assert_eq!(inventory.pending_delegations, [debt]);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_activity_never_claims_foreign_orphans_without_ownership_evidence() {
    use layer2_core::sandbox::{SandboxEvent, SandboxEventKind};
    use layer2_core::{
        ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget,
    };
    let db = database().await;
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    let activity = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Resource(
            "sandbox:sbx_foreign_removed:events".into(),
        ));
    store.append(&activity).await.unwrap();
    let event = SandboxEvent {
        sandbox_id: SandboxId("sbx_foreign_removed".into()),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    let original = crate::store::sandbox_events::event_document(&event).unwrap();
    let id = original.get_str("_id").unwrap().to_owned();
    put(&db, "sandbox_events", original.clone()).await;
    let mut inventory = ErasureInventory::default();
    drain(&db, "erased", &mut inventory).await;
    assert_eq!(count(&db, "activity").await, 0);
    assert_eq!(row(&db, "sandbox_events", &id).await, original);
    assert_eq!(
        crate::store::sandbox_events::read_event(&row(&db, "sandbox_events", &id).await).unwrap(),
        event
    );
    assert!(inventory.sandboxes.is_empty());
    assert_eq!(count(&db, "sandboxes").await, 0);
    assert_eq!(count(&db, "resource_policies").await, 0);
    db.drop().await.unwrap();
}

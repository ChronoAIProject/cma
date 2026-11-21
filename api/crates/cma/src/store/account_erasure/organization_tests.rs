//! Organization and surviving-owner identity references use actual writer shapes.
mod activity_tests;
mod audit_tests;
mod bot_tests;
mod channel_tests;
mod share_tests;

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
                    replay_principal: None,
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

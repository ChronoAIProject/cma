use super::*;
use layer2_core::ids::AgentCommandId;
use layer2_core::resource_access::{
    AgentShareAudience, AgentShareLink, ResourceSelector, ResourceShare, ShareMode,
    ShareMutationReceipt,
};
use layer2_core::sandbox::agent_io::*;
use layer2_core::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget, DetailKey,
};
use mongodb::bson::bson;

pub(super) async fn append(
    db: &Database,
    target: &str,
    subject: Option<&str>,
    resource_name: Option<&str>,
) -> Document {
    let mut record = ActivityRecord::new(ActivityAction::Delete, ActivityOutcome::Succeeded)
        .by(SubjectId::new("operator"))
        .on(ActivityTarget::Resource(target.into()));
    record.at = NOW.into();
    if let Some(subject) = subject {
        record.detail.record(DetailKey::Subject, subject);
    }
    if let Some(name) = resource_name {
        record.detail.record(DetailKey::ResourceName, name);
    }
    record.detail.record(DetailKey::Status, "completed");
    record
        .detail
        .record(DetailKey::CorrelationId, "operator-audit");
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    store.append(&record).await.unwrap();
    row(db, "activity", &bson!(record.id.0)).await
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_resource_and_policy_are_preserved_by_a_permanent_identity_refusal() {
    let db = database().await;
    sandbox(&db, "organization", "sbx_org").await;
    let resource = row(&db, "sandboxes", &bson!("sbx_org")).await;
    let original = crate::store::resource_access::records::policy_document(&policy()).unwrap();
    put(&db, "resource_policies", original.clone()).await;
    preserved(&db, "resource_policies", &original).await;
    assert_eq!(row(&db, "sandboxes", &bson!("sbx_org")).await, resource);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_resource_actor_activity_cannot_take_the_personal_delete_path() {
    let db = database().await;
    sandbox(&db, "organization", "sbx_org").await;
    let mut policy = policy();
    policy.organization_evidence.as_mut().unwrap().admitted_by = actor("survivor");
    let policy = crate::store::resource_access::records::policy_document(&policy).unwrap();
    put(&db, "resource_policies", policy.clone()).await;
    let mut record = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
        .by(SubjectId::new("a"))
        .on(ActivityTarget::Resource("sandbox:sbx_org:events".into()));
    record.at = NOW.into();
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    store.append(&record).await.unwrap();
    let original = row(&db, "activity", &bson!(record.id.0)).await;
    preserved(&db, "activity", &original).await;
    assert_eq!(
        row(&db, "resource_policies", &bson!("sbx_org")).await,
        policy
    );
    assert_eq!(count(&db, "sandboxes").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn every_operator_target_with_explicit_subject_remains_pending_and_unchanged() {
    for target in [
        "profiles",
        "profile:agent:agp_removed",
        "profile:workspace:wkp_removed",
        "profile:sandbox:sbp_removed",
        "sandboxes",
        "sandbox:removed",
        "sandbox:updated",
        "sandbox:removed:events",
    ] {
        let db = database().await;
        let original = append(&db, target, Some("a"), Some("preserved-name")).await;
        preserved(&db, "activity", &original).await;
        db.drop().await.unwrap();
    }
}

fn command() -> AdmittedAgentCommand {
    let command = AgentCommand::Interrupt;
    AdmittedAgentCommand {
        queued_input: None,
        receipt: AdmissionReceipt {
            envelope: AgentCommandEnvelope {
                continuation: None,
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: AgentCommandId("foreign-command".into()),
                agent_id: AgentId("agt_foreign".into()),
                owner: ResourceOwner::Person {
                    subject: SubjectId::new("survivor"),
                },
                actor: actor("a"),
                source: AgentCommandSource {
                    kind: AgentSourceKind::Api,
                    reference: "audit-source".into(),
                },
                correlation_id: "foreign-command".into(),
                request_fingerprint: command.fingerprint().unwrap(),
                authorization: AgentAuthorizationRevision {
                    replay_principal: None,
                    authority_id: "foreign-authority".into(),
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn foreign_agent_creation_and_command_actors_remain_pending_without_rewrites() {
    let provenance = crate::store::agent_provenance::document(&provenance("a")).unwrap();
    let command = crate::store::agent_commands::records::document(&command()).unwrap();
    for (name, original) in [
        ("agent_creation_provenance", provenance),
        ("agent_commands", command),
    ] {
        let db = database().await;
        put(&db, name, original.clone()).await;
        preserved(&db, name, &original).await;
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn a_foreign_share_receipt_keeps_its_actor_recipients_and_replay_evidence() {
    let db = database().await;
    let receipt = ShareMutationReceipt {
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
                accepted_subjects: vec![SubjectId::new("a"), SubjectId::new("other")],
            }),
            mode: ShareMode::View,
            issuer_actor: actor("survivor"),
            created_at: NOW.into(),
            revoked_at: None,
            revision: 1,
        },
        actor: actor("a"),
        at: NOW.into(),
        policy_revision: 1,
    };
    receipt.validate().unwrap();
    let original = crate::store::resource_access::records::receipt_document(&receipt).unwrap();
    put(&db, "resource_share_commands", original.clone()).await;
    preserved(&db, "resource_share_commands", &original).await;
    assert_eq!(
        crate::store::resource_access::records::read_receipt(
            &row(
                &db,
                "resource_share_commands",
                &bson!(receipt.operation_id.clone()),
            )
            .await
        )
        .unwrap(),
        receipt
    );
    db.drop().await.unwrap();
}

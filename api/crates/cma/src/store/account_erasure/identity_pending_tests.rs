//! Transitional record effects must preserve unsupported retained attribution.
use super::{references::Redaction, tests::*, *};
use layer2_core::resource_access::{
    OrganizationOwnershipEvidence, ResourceActor, ResourceOwner, ResourcePolicyRecord,
};
use layer2_core::sandbox::{AgentCreationOrigin, AgentCreationProvenance, AgentOrigin};
use layer2_core::{AgentId, SandboxId, WorkspaceId, WorktreeId};
use mongodb::bson::{doc, Document};

mod audits;
mod custody;

fn actor(subject: &str) -> ResourceActor {
    ResourceActor::Api {
        subject: SubjectId::new(subject),
        delegation_id: None,
    }
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
            admitted_by: actor("a"),
            verified_at: NOW.into(),
        }),
        revision: 1,
    }
}

fn provenance(subject: &str) -> AgentCreationProvenance {
    AgentCreationProvenance {
        schema_version: 1,
        agent_id: AgentId("agt_foreign".into()),
        agent_created_at: NOW.into(),
        sandbox_id: SandboxId("sbx_foreign".into()),
        sandbox_created_at: NOW.into(),
        workspace_id: WorkspaceId("wsp_foreign".into()),
        worktree_id: WorktreeId("wtr_foreign".into()),
        owner: ResourceOwner::Person {
            subject: SubjectId::new("survivor"),
        },
        creation_origin: AgentCreationOrigin::Manual,
        created_by: Some(actor(subject)),
        provisioning_origin: AgentOrigin::Manual,
        creation_fingerprint: "a".repeat(64),
    }
}

async fn row(db: &Database, collection: &str, id: &mongodb::bson::Bson) -> Document {
    db.collection::<Document>(collection)
        .find_one(doc! {"_id":id.clone()})
        .await
        .unwrap()
        .unwrap()
}

fn assert_pending(error: Error, collection: &str) {
    match error {
        Error::Interrupted { error, batch } => {
            assert!(batch.waiting && batch.has_more);
            assert_pending(*error, collection);
        }
        Error::Blocked(block) => {
            assert_eq!(
                block.subject_hash,
                layer2_core::document_artifacts::digest(b"a")
            );
            assert_eq!(
                block.reason,
                ErasureBlock::IdentityRedactionPending {
                    collection: collection.into()
                }
            );
        }
        other => panic!("expected a permanent identity refusal, got {other:?}"),
    }
}

async fn executor_refuses(db: &Database, collection: &str, cursor: &mut Cursor) {
    let subject = SubjectId::new("a");
    let mut inventory = ErasureInventory::default();
    for _ in 0..500 {
        match erase_records(
            db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            cursor,
            NOW,
        )
        .await
        {
            Err(error) => {
                assert_pending(error, collection);
                return;
            }
            Ok(batch) => {
                progress::acknowledge(&mut inventory, &batch, "a");
                assert!(batch.has_more, "identity residue must prevent completion");
            }
        }
    }
    panic!("identity residue never reached a permanent refusal");
}

async fn preserved(db: &Database, collection: &str, original: &Document) {
    let subjects = BTreeSet::from(["a".into()]);
    let id = original.get("_id").unwrap();
    for _ in 0..2 {
        assert_pending(
            super::references::redact(db, collection, original, &subjects)
                .await
                .unwrap_err(),
            collection,
        );
        assert_eq!(row(db, collection, id).await, *original);
    }
    let mut cursor = Cursor::default();
    executor_refuses(db, collection, &mut cursor).await;
    assert_eq!(row(db, collection, id).await, *original);
    cursor = serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
    executor_refuses(db, collection, &mut cursor).await;
    assert_eq!(row(db, collection, id).await, *original);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unrelated_retained_attribution_and_free_text_allow_personal_erasure() {
    let db = database().await;
    sandbox(&db, "a", "sbx_personal").await;
    let mut policy = policy();
    policy.organization_evidence.as_mut().unwrap().admitted_by = actor("survivor");
    let policy = crate::store::resource_access::records::policy_document(&policy).unwrap();
    put(&db, "resource_policies", policy.clone()).await;
    let provenance = crate::store::agent_provenance::document(&provenance("survivor")).unwrap();
    put(&db, "agent_creation_provenance", provenance.clone()).await;
    let subjects = BTreeSet::from(["a".into()]);
    for (name, original) in [
        ("resource_policies", policy),
        ("agent_creation_provenance", provenance),
    ] {
        assert_eq!(
            super::references::redact(&db, name, &original, &subjects)
                .await
                .unwrap(),
            Redaction::Unchanged
        );
        assert_eq!(row(&db, name, original.get("_id").unwrap()).await, original);
    }
    let original = audits::append(&db, "profiles", Some("other"), Some("a")).await;
    drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(count(&db, "sandboxes").await, 0);
    assert_eq!(
        row(&db, "activity", original.get("_id").unwrap()).await,
        original
    );
    db.drop().await.unwrap();
}

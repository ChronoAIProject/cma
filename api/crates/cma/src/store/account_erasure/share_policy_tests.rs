//! Real writer and reader contracts for foreign personal share redaction.
use super::references::{redact, Redaction};
use crate::store::{
    resource_access::records, sandbox_mongo::MongoSandboxStore,
    sandbox_mongo_integration_tests::resource_access_tests::fixture,
};
use layer2_core::{resource_access::*, Authorization, Caller, Capability, SubjectId};
use mongodb::{
    bson::{doc, Document},
    Client, Database,
};
use std::{collections::BTreeSet, sync::Arc};

const AT: &str = "2026-11-14T00:00:00Z";

struct NoOrganizations;
#[async_trait::async_trait]
impl OrganizationAuthority for NoOrganizations {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::Unavailable)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Err(MembershipError::Unavailable)
    }
}

fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Retained,
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "https://identity.test".into(),
            subject: SubjectId::new(subject),
            credential_id: format!("fixture-{subject}"),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(subject),
        },
        memberships: vec![],
    }
}

fn mutation(
    snapshot: &ResourceAccessSnapshot,
    key: &str,
    subject: &str,
    change: ShareChange,
    revision: Option<u64>,
) -> ShareMutation {
    ShareMutation {
        selector: ResourceSelector::Agent {
            agent_id: snapshot.lineage.agent.as_ref().unwrap().id.clone(),
        },
        owner: ResourceOwner::Person {
            subject: snapshot.lineage.sandbox.owner.clone(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(subject),
        },
        key_sha256: layer2_core::document_artifacts::digest(key.as_bytes()),
        change,
        expected_share_revision: revision,
        policy_fence: snapshot.fence(),
        proposed_id: format!("share-{key}"),
        at: AT.into(),
    }
}

async fn seed() -> (
    Database,
    Arc<MongoSandboxStore>,
    ResourceTarget,
    ResourceShare,
) {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let db = client.database(&format!(
        "cma_erasure_share_policy_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let (store, target) = fixture(&client, db.name()).await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(snapshot.lineage.sandbox.owner.as_str(), "owner");
    let mut share = store
        .mutate_share(mutation(
            &snapshot,
            "create",
            "owner",
            ShareChange::CreateLink {
                audience: AgentShareAudience::Public,
                mode: ShareMode::View,
                token_sha256: "a".repeat(64),
            },
            None,
        ))
        .await
        .unwrap()
        .share;
    for recipient in ["erased", "remaining"] {
        let snapshot = store.resource_access_snapshot(&target).await.unwrap();
        share = store
            .mutate_share(mutation(
                &snapshot,
                recipient,
                recipient,
                ShareChange::RedeemLink {
                    share_id: share.id.clone(),
                    token_sha256: "a".repeat(64),
                    subject: SubjectId::new(recipient),
                },
                Some(share.revision),
            ))
            .await
            .unwrap()
            .share;
    }
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(share.revision, snapshot.fence().policy_revision);
    (db, Arc::new(store), target, share)
}

async fn row(db: &Database, collection: &str, id: &str) -> Document {
    db.collection::<Document>(collection)
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap()
}

async fn readable(
    store: Arc<MongoSandboxStore>,
    target: &ResourceTarget,
    allowed: &[&str],
    denied: &[&str],
) {
    let service = ResourceAccessService::new(store, Arc::new(NoOrganizations));
    for subject in allowed {
        service
            .authorize(
                &context(subject),
                None,
                target,
                ResourceCapability::ViewHistory,
                AccessPurpose::Read,
            )
            .await
            .unwrap_or_else(|error| panic!("surviving access for {subject}: {error:?}"));
    }
    for subject in denied {
        assert!(matches!(
            service
                .authorize(
                    &context(subject),
                    None,
                    target,
                    ResourceCapability::ViewHistory,
                    AccessPurpose::Read,
                )
                .await,
            Err(ResourceAccessError::Access(AccessError::NotFound))
        ));
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn personal_share_erasure_preserves_policy_fence_and_surviving_access() {
    let (db, store, target, share) = seed().await;
    readable(
        store.clone(),
        &target,
        &["owner", "erased", "remaining"],
        &[],
    )
    .await;
    let original = row(&db, "resource_shares", &share.id).await;
    assert_eq!(
        redact(
            &db,
            "resource_shares",
            &original,
            &BTreeSet::from(["erased".into()])
        )
        .await
        .unwrap(),
        Redaction::Updated
    );
    readable(store.clone(), &target, &["owner", "remaining"], &["erased"]).await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    assert_eq!(snapshot.shares[0].revision, share.revision + 1);
    assert!(snapshot.fence().policy_revision >= snapshot.shares[0].revision);
    assert_eq!(
        snapshot.shares[0].link.as_ref().unwrap().accepted_subjects,
        [SubjectId::new("remaining")]
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn personal_share_erasure_cas_preserves_concurrent_recipient_and_policy() {
    let (db, store, target, share) = seed().await;
    let stale = row(&db, "resource_shares", &share.id).await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let concurrent = store
        .mutate_share(mutation(
            &snapshot,
            "concurrent",
            "new-recipient",
            ShareChange::RedeemLink {
                share_id: share.id.clone(),
                token_sha256: "a".repeat(64),
                subject: SubjectId::new("new-recipient"),
            },
            Some(share.revision),
        ))
        .await
        .unwrap();
    let subjects = BTreeSet::from(["erased".into()]);
    assert_eq!(
        redact(&db, "resource_shares", &stale, &subjects)
            .await
            .unwrap(),
        Redaction::Conflict
    );
    assert_eq!(
        store.resource_share(&share.id).await.unwrap(),
        Some(concurrent.share)
    );
    readable(
        store.clone(),
        &target,
        &["owner", "erased", "remaining", "new-recipient"],
        &[],
    )
    .await;
    let fresh = row(&db, "resource_shares", &share.id).await;
    assert_eq!(
        redact(&db, "resource_shares", &fresh, &subjects)
            .await
            .unwrap(),
        Redaction::Updated
    );
    readable(
        store,
        &target,
        &["owner", "remaining", "new-recipient"],
        &["erased"],
    )
    .await;
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn personal_share_erasure_reserved_policy_keeps_reader_and_writer_fences_valid() {
    let (db, store, target, share) = seed().await;
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let stale_writer = mutation(
        &snapshot,
        "stale-writer",
        "owner",
        ShareChange::SetMode {
            share_id: share.id.clone(),
            mode: ShareMode::View,
        },
        Some(share.revision),
    );
    let id = snapshot.lineage.sandbox.id.as_str();
    let original_policy = row(&db, "resource_policies", id).await;
    let mut policy = records::read_policy(&original_policy).unwrap();
    policy.revision += 1;
    let mut reserved = original_policy.clone();
    reserved.extend(records::policy_document(&policy).unwrap());
    assert_eq!(
        db.collection::<Document>("resource_policies")
            .replace_one(
                doc! {"_id":id,"$expr":{"$eq":["$$ROOT",{"$literal":original_policy}]}},
                reserved,
            )
            .await
            .unwrap()
            .matched_count,
        1
    );
    readable(
        store.clone(),
        &target,
        &["owner", "erased", "remaining"],
        &[],
    )
    .await;
    assert_eq!(
        store.mutate_share(stale_writer).await,
        Err(ResourceAccessStoreError::Conflict)
    );
    let original_share = row(&db, "resource_shares", &share.id).await;
    assert_eq!(
        redact(
            &db,
            "resource_shares",
            &original_share,
            &BTreeSet::from(["erased".into()])
        )
        .await
        .unwrap(),
        Redaction::Updated
    );
    readable(store, &target, &["owner", "remaining"], &["erased"]).await;
    db.drop().await.unwrap();
}

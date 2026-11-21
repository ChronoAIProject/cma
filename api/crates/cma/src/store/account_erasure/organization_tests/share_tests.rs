use super::*;

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

use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn missing_or_invalid_issuance_evidence_cannot_authorize_purge() {
    let f = Fixture::new("purge_bad_intent").await;
    let valid = f.intent(IssuanceState::Accepted);
    f.seed_intent(&valid).await;
    let rows = f.rows("sandbox_credential_issuance");
    let original = rows.find_one(doc! {}).await.unwrap().unwrap();
    for patch in [
        doc! {"document_schema_version":2_i64},
        doc! {"record.state":"future_state"},
        doc! {"record.generation":0_i64},
        doc! {"record.original_fence":0_i64},
        doc! {"record.original_holder":""},
        doc! {"record.operation_id":"not-an-operation"},
        doc! {"record.owner":"different-owner"},
        doc! {"record.sandbox_id":"different-sandbox"},
        doc! {"record.created_at":"not-a-time"},
        doc! {"record.next_check_at":"not-a-time"},
        doc! {"record.identity.provider_identity":"invalid"},
        doc! {"record.identity.request_digest":"invalid"},
        doc! {"record.identity":{"kind":"legacy_unobserved"}},
    ] {
        rows.replace_one(doc! {}, original.clone()).await.unwrap();
        rows.update_one(doc! {}, doc! {"$set":patch.clone()})
            .await
            .unwrap();
        let before = f.snapshot().await;
        assert_eq!(
            f.purge().await,
            Err(SandboxStoreError::Conflict),
            "{patch:?}"
        );
        assert_eq!(f.snapshot().await, before);
    }
    rows.delete_many(doc! {}).await.unwrap();
    let before = f.snapshot().await;
    assert_eq!(f.purge().await, Err(SandboxStoreError::Conflict));
    assert_eq!(
        f.snapshot().await,
        before,
        "a retained marker requires its intent"
    );
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn accepted_issuance_requires_its_valid_parent_marker_and_source() {
    let f = Fixture::new("purge_bad_marker").await;
    f.seed_intent(&f.intent(IssuanceState::Accepted)).await;
    let parents = f.rows("sandboxes");
    for marker in [
        Bson::Null,
        Bson::Int64(0),
        Bson::Int64(-1),
        Bson::String("1".into()),
    ] {
        parents
            .update_one(doc! {}, doc! {"$set":{"credential_issuance_fence":marker}})
            .await
            .unwrap();
        let before = f.snapshot().await;
        assert_eq!(f.purge().await, Err(SandboxStoreError::Conflict));
        assert_eq!(f.snapshot().await, before);
    }
    parents
        .update_one(doc! {}, doc! {"$unset":{"credential_issuance_fence":""}})
        .await
        .unwrap();
    assert_eq!(f.purge().await, Err(SandboxStoreError::Conflict));
    let source = layer2_core::sandbox::SandboxCredentialSource::Provided {
        key_sha256: "c".repeat(64),
    };
    parents
        .update_one(
            doc! {},
            doc! {"$set":{
                "credential_issuance_fence":1_i64,
                "record.credential.source":mongodb::bson::serialize_to_bson(&source).unwrap(),
            }},
        )
        .await
        .unwrap();
    let before = f.snapshot().await;
    assert_eq!(f.purge().await, Err(SandboxStoreError::Conflict));
    assert_eq!(f.snapshot().await, before);
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn accepted_issuance_is_not_revocation_finality_for_known_obligations() {
    for kind in ["current", "pending", "debt", "custody", "queue"] {
        let f = Fixture::new("purge_known_duty").await;
        f.seed_intent(&f.intent(IssuanceState::Accepted)).await;
        let mut credential = f.parent.credential.clone();
        match kind {
            "current" => credential.current = sandbox("known").credential.current,
            "pending" => credential.pending = sandbox("known").credential.current,
            "debt" => credential.queue_revoke("known-key".into(), NOW.into()),
            _ => {
                let collection = if kind == "custody" {
                    "sandbox_credentials"
                } else {
                    "sandbox_credential_revocations"
                };
                f.rows(collection)
                    .insert_one(doc! {
                        "_id":"retained-duty", "sandbox_id":f.parent.id.as_str(),
                        "owner":f.parent.owner.as_str(), "provider_key_id":"known-key",
                    })
                    .await
                    .unwrap();
            }
        }
        credential.delivered = credential.current.is_some();
        assert!(credential.validate(), "{kind}");
        f.rows("sandboxes")
            .update_one(
                doc! {},
                doc! {"$set":{
                    "record.credential":mongodb::bson::serialize_to_bson(&credential).unwrap()
                }},
            )
            .await
            .unwrap();
        let before = f.snapshot().await;
        assert_eq!(f.purge().await, Err(SandboxStoreError::Conflict), "{kind}");
        assert_eq!(f.snapshot().await, before, "{kind}");
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn another_sandbox_obligation_does_not_block_a_settled_parent() {
    let f = Fixture::new("purge_exact_owner").await;
    let other = retired_sandbox("sbx_unrelated");
    f.store.create_sandbox(other.clone()).await.unwrap();
    let mut intent = f.intent(IssuanceState::Unknown);
    intent.sandbox_id = other.id.clone();
    f.rows("sandbox_credential_issuance")
        .insert_one(doc! {
            "_id":other.id.as_str(), "document_schema_version":1_i64,
            "record":mongodb::bson::serialize_to_bson(&intent).unwrap(),
        })
        .await
        .unwrap();
    for collection in ["sandbox_credentials", "sandbox_credential_revocations"] {
        f.rows(collection)
            .insert_one(doc! {"_id":"unrelated", "sandbox_id":other.id.as_str()})
            .await
            .unwrap();
    }
    f.purge().await.unwrap();
    assert!(f.store.get_sandbox(&f.parent.id).await.unwrap().is_none());
    assert_eq!(f.store.get_sandbox(&other.id).await.unwrap(), Some(other));
    assert_eq!(
        f.rows("sandbox_credential_issuance")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.cleanup().await;
}

use super::{policy::*, writer::*, *};
use crate::store::account_deletions::MongoAccountDeletionStore;
use futures_util::TryStreamExt;
use layer2_core::account_deletion::AccountDeletionStore;
use mongodb::Client;

fn release(digit: char, key: &str) -> FrozenRelease {
    FrozenRelease {
        release_key: key.into(),
        image: RuntimeImage {
            uri: format!(
                "registry.example/runtime@sha256:{}",
                digit.to_string().repeat(64)
            ),
            ..Default::default()
        },
        config_digest: format!("sha256:{}", "c".repeat(64)),
        source_revision: "d".repeat(40),
        publication_evidence_sha256: "e".repeat(64),
        capacity: SandboxCapacity::default(),
    }
}

fn issue(
    owner: &SubjectId,
    release: FrozenRelease,
    policy_revision: i64,
    expected_revision: i64,
) -> PrepareCanaryGrant {
    PrepareCanaryGrant {
        owner: owner.clone(),
        expected_revision,
        lane: CanaryLane::Agent,
        release,
        policy_revision,
        catalog_sha256: "a".repeat(64),
        owner_scope_sha256: "b".repeat(64),
        consent_sha256: "f".repeat(64),
        expires_at: DateTime::now().saturating_add_duration(Duration::from_secs(600)),
    }
}

fn redeem(
    issue: &PrepareCanaryGrant,
    grant: &CanaryGrantSlot,
    key: &str,
) -> PreparedCanaryRedemption {
    PreparedCanaryRedemption {
        owner: issue.owner.clone(),
        grant_id: grant.id.clone(),
        release_key: issue.release.release_key.clone(),
        policy_revision: issue.policy_revision,
        key: key.into(),
        fresh: Some(FreshCanaryRedemption {
            lane: issue.lane,
            policy_revision: issue.policy_revision,
            catalog_sha256: issue.catalog_sha256.clone(),
            owner_scope_sha256: issue.owner_scope_sha256.clone(),
            release: issue.release.clone(),
        }),
        input: LifecycleInput {
            behavior: LifecycleBehavior::Legacy,
            managed_agent_instructions: None,
            setup_message: None,
            request: serde_json::from_value(serde_json::json!({"name": "Canary fixture"})).unwrap(),
            actor: None,
            resolved_image: None,
            capacity: None,
            sandbox_availability: Default::default(),
            capabilities: vec![Capability::UseOwnSandboxes, Capability::CreateSandbox],
        },
    }
}

fn operation_id(result: CreateResult) -> String {
    match result {
        CreateResult::Created(id) => id,
        CreateResult::Existing(record) => record.id.clone(),
    }
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn concurrent_canary_redemption_replays_after_slot_replacement_and_closure() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    let reader = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_canary_grant_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    let peer_database = reader.database(&name);
    let owner = SubjectId::new("canary-owner-fixture");
    let store = MongoAgentLifecycleStore::open(&database).await.unwrap();
    let indexes: Vec<_> = store
        .operations
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    for (name, keys, field) in [
        (
            "agent_lifecycle_canary_sandbox_profile",
            doc! { SANDBOX_PROFILE_ID: 1 },
            SANDBOX_PROFILE_ID,
        ),
        (
            "agent_lifecycle_canary_manifest_owner",
            doc! { MANIFEST_DIGEST: 1, "owner": 1 },
            MANIFEST_DIGEST,
        ),
        (
            "agent_lifecycle_canary_owner_request",
            doc! { "owner": 1, REQUEST_KEY_DIGEST: 1 },
            REQUEST_KEY_DIGEST,
        ),
    ] {
        let index = indexes
            .iter()
            .find(|index| {
                index.options.as_ref().and_then(|item| item.name.as_deref()) == Some(name)
            })
            .unwrap();
        let options = index.options.as_ref().unwrap();
        assert_eq!(index.keys, keys);
        assert_eq!(options.unique, Some(true));
        assert_eq!(
            options.partial_filter_expression,
            Some(doc! { (field): { "$exists": true } }),
        );
    }
    let peer = MongoAgentLifecycleStore::open(&peer_database)
        .await
        .unwrap();
    let erasure = MongoAccountDeletionStore::open(&database).await.unwrap();
    let initial = issue(&owner, release('1', "first"), 1, 0);
    CanaryPolicy::configure(
        &database,
        0,
        true,
        initial.catalog_sha256.clone(),
        initial.owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    let grant = store.issue_canary_grant(&database, &initial).await.unwrap();
    let (first, same, different) = tokio::join!(
        store.redeem_canary(&database, redeem(&initial, &grant, "exact")),
        peer.redeem_canary(&peer_database, redeem(&initial, &grant, "exact")),
        peer.redeem_canary(&peer_database, redeem(&initial, &grant, "different")),
    );
    let (key, result) = match different {
        Ok(value) => {
            assert!(matches!(first, Err(CanaryWriteError::Refused)));
            assert!(matches!(same, Err(CanaryWriteError::Refused)));
            ("different", value)
        }
        Err(CanaryWriteError::Refused) => {
            let first = first.unwrap();
            assert_eq!(operation_id(first.clone()), operation_id(same.unwrap()));
            ("exact", first)
        }
        Err(error) => panic!("{error:?}"),
    };
    let id = operation_id(result);
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 1);
    let next = issue(&owner, release('2', "second"), 1, grant.revision + 1);
    let slot = store.issue_canary_grant(&database, &next).await.unwrap();
    assert_ne!(slot.id, grant.id);
    CanaryPolicy::configure(
        &database,
        1,
        false,
        next.catalog_sha256.clone(),
        next.owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    let mut exact = redeem(&initial, &grant, key);
    exact.fresh = None;
    assert_eq!(
        operation_id(peer.redeem_canary(&peer_database, exact).await.unwrap()),
        id
    );
    let mut changed = redeem(&initial, &grant, key);
    changed.fresh = None;
    changed.input.request.name = "Different request".into();
    assert!(matches!(
        store.redeem_canary(&database, changed).await,
        Err(CanaryWriteError::Refused)
    ));
    assert!(matches!(
        store
            .redeem_canary(&database, redeem(&next, &slot, "new"))
            .await,
        Err(CanaryWriteError::Closed)
    ));

    erasure
        .request(&owner, "remove-fixture", &now_rfc3339())
        .await
        .unwrap();
    let mut deleted = redeem(&initial, &grant, key);
    deleted.fresh = None;
    assert!(matches!(
        peer.redeem_canary(&peer_database, deleted).await,
        Err(CanaryWriteError::OwnerUnavailable)
    ));
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 1);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn canary_revocation_and_redemption_share_the_same_owner_fence() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    let peer_client = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_canary_revoke_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    let peer_database = peer_client.database(&name);
    let store = MongoAgentLifecycleStore::open(&database).await.unwrap();
    let peer = MongoAgentLifecycleStore::open(&peer_database)
        .await
        .unwrap();
    let owner = SubjectId::new("canary-owner-fixture");
    let request = issue(&owner, release('1', "candidate"), 1, 0);
    CanaryPolicy::configure(
        &database,
        0,
        true,
        request.catalog_sha256.clone(),
        request.owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    let grant = store.issue_canary_grant(&database, &request).await.unwrap();
    let revocation = RevokeCanaryGrant {
        owner: owner.clone(),
        lane: request.lane,
        grant_id: grant.id.clone(),
        revision: grant.revision,
    };
    let (revoked, redeemed) = tokio::join!(
        store.revoke_canary_grant(&database, &revocation),
        peer.redeem_canary(&peer_database, redeem(&request, &grant, "race-redemption")),
    );
    let rows = store.operations.count_documents(doc! {}).await.unwrap();
    if revoked.is_ok() {
        assert!(matches!(redeemed, Err(CanaryWriteError::Refused)));
        assert_eq!(rows, 0);
    } else {
        let id = operation_id(redeemed.unwrap());
        assert_eq!(rows, 1);
        assert!(matches!(
            store.revoke_canary_grant(&database, &revocation).await,
            Err(CanaryWriteError::Refused)
        ));
        let replay = peer
            .redeem_canary(&peer_database, redeem(&request, &grant, "race-redemption"))
            .await
            .unwrap();
        assert!(matches!(replay, CreateResult::Existing(_)));
        assert_eq!(operation_id(replay), id);
    }
    database.drop().await.unwrap();
}

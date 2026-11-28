use super::{policy::*, writer::*, *};
use crate::store::account_deletions::MongoAccountDeletionStore;
use futures_util::TryStreamExt;
use layer2_core::account_deletion::AccountDeletionStore;
use mongodb::Client;

pub(super) fn release(digit: char, key: &str) -> FrozenRelease {
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

pub(super) fn issue(
    owner: &SubjectId,
    release: FrozenRelease,
    policy_revision: i64,
    expected_revision: i64,
) -> PrepareCanaryGrant {
    PrepareCanaryGrant {
        owner: owner.clone(),
        expected_revision,
        release_key: release.release_key.clone(),
        policy_revision,
        consent_sha256: "f".repeat(64),
        expires_at: DateTime::now().saturating_add_duration(Duration::from_secs(600)),
        fresh: Some(FreshCanaryGrant {
            lane: CanaryLane::Agent,
            release,
            catalog_sha256: "a".repeat(64),
            owner_scope_sha256: "b".repeat(64),
        }),
    }
}

pub(super) fn redeem(
    issue: &PrepareCanaryGrant,
    grant: &CanaryGrantSlot,
    key: &str,
) -> PreparedCanaryRedemption {
    let authority = issue.fresh.as_ref().unwrap();
    PreparedCanaryRedemption {
        owner: issue.owner.clone(),
        grant_id: grant.id.clone(),
        release_key: issue.release_key.clone(),
        policy_revision: issue.policy_revision,
        key: key.into(),
        fresh: Some(FreshCanaryRedemption {
            lane: authority.lane,
            policy_revision: issue.policy_revision,
            catalog_sha256: authority.catalog_sha256.clone(),
            owner_scope_sha256: authority.owner_scope_sha256.clone(),
            release: authority.release.clone(),
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
        initial.fresh.as_ref().unwrap().catalog_sha256.clone(),
        initial.fresh.as_ref().unwrap().owner_scope_sha256.clone(),
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
        next.fresh.as_ref().unwrap().catalog_sha256.clone(),
        next.fresh.as_ref().unwrap().owner_scope_sha256.clone(),
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
        request.fresh.as_ref().unwrap().catalog_sha256.clone(),
        request.fresh.as_ref().unwrap().owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    let grant = store.issue_canary_grant(&database, &request).await.unwrap();
    let revocation = RevokeCanaryGrant {
        owner: owner.clone(),
        release_key: request.release_key.clone(),
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

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn canary_grant_receipt_and_revocation_survive_catalog_reassignment() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    let reader = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_canary_issuance_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    let peer_database = reader.database(&name);
    let store = MongoAgentLifecycleStore::open(&database).await.unwrap();
    let peer = MongoAgentLifecycleStore::open(&peer_database)
        .await
        .unwrap();
    let owner = SubjectId::new("grant-recovery-owner");
    let mut old = issue(&owner, release('1', "reassigned"), 1, 0);
    CanaryPolicy::configure(&database, 0, true, "a".repeat(64), "b".repeat(64))
        .await
        .unwrap();
    let original = store.issue_canary_grant(&database, &old).await.unwrap();
    let consumed_request = issue(&owner, release('3', "consumed"), 1, 0);
    let other = SubjectId::new("grant-consumed-owner");
    let consumed_request = PrepareCanaryGrant {
        owner: other,
        ..consumed_request
    };
    let consumed = store
        .issue_canary_grant(&database, &consumed_request)
        .await
        .unwrap();
    let operation = operation_id(
        store
            .redeem_canary(
                &database,
                redeem(&consumed_request, &consumed, "original-consumed"),
            )
            .await
            .unwrap(),
    );

    CanaryPolicy::configure(&database, 1, true, "c".repeat(64), "b".repeat(64))
        .await
        .unwrap();
    let mut replacement = issue(&owner, release('2', "reassigned"), 2, 0);
    let authority = replacement.fresh.as_mut().unwrap();
    authority.lane = CanaryLane::DebianDesktop;
    authority.catalog_sha256 = "c".repeat(64);
    let replacement_receipt = store
        .issue_canary_grant(&database, &replacement)
        .await
        .unwrap();
    old.fresh = None;
    let replay = peer.issue_canary_grant(&peer_database, &old).await.unwrap();
    assert_eq!(replay.id, original.id);
    assert_eq!(replay.revision, original.revision);
    assert_eq!(replay.policy_revision, original.policy_revision);

    let revoked = peer
        .revoke_canary_grant(
            &peer_database,
            &RevokeCanaryGrant {
                owner,
                release_key: old.release_key,
                grant_id: replay.id.clone(),
                revision: replay.revision,
            },
        )
        .await
        .unwrap();
    assert_eq!(revoked.id, replay.id);
    assert_eq!(revoked.revision, replay.revision + 1);
    assert_eq!(
        store
            .issue_canary_grant(&database, &replacement)
            .await
            .unwrap()
            .id,
        replacement_receipt.id
    );
    let mut exact = redeem(&consumed_request, &consumed, "original-consumed");
    exact.fresh = None;
    assert_eq!(
        operation_id(peer.redeem_canary(&peer_database, exact).await.unwrap()),
        operation
    );
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 1);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn canary_grant_issuance_and_replacement_use_the_database_clock() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("cma_canary_expiry_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    let store = MongoAgentLifecycleStore::open(&database).await.unwrap();
    let owner = SubjectId::new("grant-expiry-owner");
    CanaryPolicy::configure(&database, 0, true, "a".repeat(64), "b".repeat(64))
        .await
        .unwrap();
    let database_now = database.run_command(doc! { "hello": 1 }).await.unwrap();
    let now = *database_now.get_datetime("localTime").unwrap();
    let mut initial = issue(&owner, release('1', "original"), 1, 0);
    initial.expires_at = now;
    assert!(matches!(
        store.issue_canary_grant(&database, &initial).await,
        Err(CanaryWriteError::Refused)
    ));
    let slots = database.collection::<mongodb::bson::Document>("sandbox_admissions");
    let owner_filter = doc! { "_id": owner.as_str() };
    assert!(slots
        .find_one(owner_filter.clone())
        .await
        .unwrap()
        .is_none());

    initial.expires_at = now.saturating_add_duration(Duration::from_secs(600));
    let first = store.issue_canary_grant(&database, &initial).await.unwrap();
    let stored = slots.find_one(owner_filter.clone()).await.unwrap().unwrap();
    let replacement = issue(&owner, release('2', "replacement"), 1, first.revision);
    assert!(matches!(
        store.issue_canary_grant(&database, &replacement).await,
        Err(CanaryWriteError::Refused)
    ));
    assert_eq!(
        slots.find_one(owner_filter.clone()).await.unwrap(),
        Some(stored)
    );
    assert_eq!(
        store
            .issue_canary_grant(&database, &initial)
            .await
            .unwrap()
            .id,
        first.id
    );

    slots
        .update_one(
            owner_filter,
            vec![doc! { "$set": { "runtime_canary_slots.agent.expires_at": "$$NOW" } }],
        )
        .await
        .unwrap();
    let next = store
        .issue_canary_grant(&database, &replacement)
        .await
        .unwrap();
    assert_ne!(next.id, first.id);
    assert_eq!(next.revision, first.revision + 1);
    assert!(matches!(
        store
            .redeem_canary(&database, redeem(&initial, &first, "expired"))
            .await,
        Err(CanaryWriteError::Refused)
    ));
    assert_eq!(store.operations.count_documents(doc! {}).await.unwrap(), 0);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn desktop_canary_on_released_image_does_not_keep_the_ordinary_pool_active() {
    use super::super::released::protects_pool;

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_canary_pool_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let owner = SubjectId::new("canary-pool-fixture");
    let store = MongoAgentLifecycleStore::open(&database).await.unwrap();
    let descriptor = release('1', "desktop");
    let mut issue = issue(&owner, descriptor.clone(), 1, 0);
    let fresh = issue.fresh.as_mut().unwrap();
    fresh.lane = CanaryLane::DebianDesktop;
    CanaryPolicy::configure(
        &database,
        0,
        true,
        fresh.catalog_sha256.clone(),
        fresh.owner_scope_sha256.clone(),
    )
    .await
    .unwrap();
    let grant = store.issue_canary_grant(&database, &issue).await.unwrap();
    let request = redeem(&issue, &grant, "desktop");
    let mut ordinary = request.input.clone();
    ordinary.request.desktop = true;
    ordinary.resolved_image = Some(descriptor.image.uri.clone());

    store.redeem_canary(&database, request).await.unwrap();
    assert!(
        !protects_pool(&database, false, &descriptor.image.uri, None)
            .await
            .unwrap()
    );
    let mut machine = ordinary.clone();
    machine.request.machine_included = true;
    let pinned = layer3_adapter::SandboxCapacity {
        cpu_millis: 1000,
        memory_mib: 4096,
    };
    let obsolete = layer3_adapter::SandboxCapacity {
        cpu_millis: 2000,
        memory_mib: 8192,
    };
    machine.capacity = Some(pinned);
    store
        .create(&owner, "machine-pinned", machine.clone())
        .await
        .unwrap();
    assert!(
        protects_pool(&database, true, &descriptor.image.uri, Some(pinned))
            .await
            .unwrap()
    );
    assert!(
        !protects_pool(&database, true, &descriptor.image.uri, Some(obsolete))
            .await
            .unwrap()
    );
    machine.capacity = None;
    store
        .create(&owner, "machine-legacy", machine)
        .await
        .unwrap();
    assert!(
        protects_pool(&database, true, &descriptor.image.uri, Some(obsolete))
            .await
            .unwrap()
    );
    store.create(&owner, "ordinary", ordinary).await.unwrap();
    assert!(protects_pool(&database, false, &descriptor.image.uri, None)
        .await
        .unwrap());
    database.drop().await.unwrap();
}

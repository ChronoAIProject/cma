use super::*;

fn with_snapshot(mut input: StoredBridgeCredential) -> StoredBridgeCredential {
    let metadata = &input.metadata;
    input.metadata.key_authority_v1 = Some(
        serde_json::from_value(serde_json::json!({
            "id": metadata.connector.key_id,
            "is_active": true,
            "expires_at": null,
            "rotation_predecessor_id": null,
            "scopes": metadata.scopes,
            "allowed_service_ids": metadata.allowed_service_ids,
            "allow_all_services": metadata.allow_all_services,
            "allow_auto_connected_services": false,
            "allowed_node_ids": metadata.allowed_node_ids,
            "allow_all_nodes": metadata.allow_all_nodes,
            "rate_limit_per_second": null,
            "rate_limit_burst": null,
            "platform": "telegram",
            "callback_url": metadata.connector.callback_url,
            "credential_source": {"type": "personal"},
            "purpose": "general",
            "scheduled_write_enabled": false,
            "bindings_count": 0
        }))
        .unwrap(),
    );
    input
}

async fn create_disabled(svc: &BridgeService) -> BridgeId {
    let id = BridgeService::creation_id(&caller(), "create-upgrade").unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            0,
            "create-upgrade",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(config()),
            },
        ),
    )
    .await
    .unwrap();
    id
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn explicit_legacy_reimport_reseals_snapshot_and_grant_without_replacing_custody_epoch() {
    let (client, database, definitions) = fixture().await;
    let svc = service(definitions);
    let id = create_disabled(&svc).await;
    let management = svc.management_lease(&caller(), None, &id, 1).await.unwrap();
    let store =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    let mut legacy = material(&management);
    legacy.metadata.created_at = "2026-01-01T00:00:00Z".into();
    let now = std::time::SystemTime::now();
    let grant = crate::bridge::control_plane::BridgeControlPlaneGrant {
        bridge_id: id.clone(),
        owner: owner(),
        connector: legacy.metadata.connector.clone(),
        subject: caller().caller.subject,
        incarnation: "fixture-snapshot-upgrade-sign-in".into(),
        issued_at: layer2_core::rfc3339_at(now),
        expires_at: layer2_core::rfc3339_at(now + crate::bridge::control_plane::GRANT_LIFETIME),
    };
    legacy.control_plane = Some(grant.clone());
    let reference = legacy.metadata.connector.custody.clone();
    let original = store.admit(&management, legacy).await.unwrap();
    let collection = client
        .database(&database)
        .collection::<Document>("bridge_credentials");
    let raw_legacy = collection.find_one(doc! {}).await.unwrap().unwrap();
    assert!(!raw_legacy
        .get_document("metadata")
        .unwrap()
        .contains_key("key_authority_v1"));

    // Opening old AAD is read-only. No resolver/read may infer fresh consent.
    let reopened =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    assert_eq!(
        reopened.get(&reference).await.unwrap().unwrap().metadata,
        original
    );
    assert_eq!(
        collection.find_one(doc! {}).await.unwrap().unwrap(),
        raw_legacy
    );

    let mut upgrade = with_snapshot(material(&management));
    upgrade.metadata.key_state_version = 7;
    let mut expected = original.clone();
    expected.key_authority_v1 = upgrade.metadata.key_authority_v1.clone();
    assert_eq!(
        reopened.admit(&management, upgrade).await.unwrap(),
        expected
    );
    let retained = reopened.get(&reference).await.unwrap().unwrap();
    assert_eq!(retained.metadata, expected);
    assert_eq!(retained.value.expose(), SECRET);
    assert_eq!(retained.control_plane, Some(grant.clone()));
    assert_eq!(retained.metadata.key_state_version, 3);
    assert_eq!(retained.metadata.revision, 1);
    assert_eq!(retained.metadata.created_at, original.created_at);
    assert_eq!(retained.metadata.created_by, original.created_by);
    let raw_upgraded = collection.find_one(doc! {}).await.unwrap().unwrap();
    assert_ne!(raw_upgraded.get("secret"), raw_legacy.get("secret"));
    assert_ne!(
        raw_upgraded.get("control_plane"),
        raw_legacy.get("control_plane")
    );
    assert!(!format!("{raw_upgraded:?}").contains(SECRET));
    assert!(!format!("{raw_upgraded:?}").contains(&grant.incarnation));

    let mut retry = with_snapshot(material(&management));
    retry.metadata.key_state_version = 11;
    assert_eq!(reopened.admit(&management, retry).await.unwrap(), expected);
    assert_eq!(
        collection.find_one(doc! {}).await.unwrap().unwrap(),
        raw_upgraded
    );

    // Snapshot bytes are authenticated along with both encrypted values.
    collection
        .update_one(
            doc! {"_id": reference.as_str()},
            doc! {"$set": {"metadata.key_authority_v1.rate_limit_burst": 20}},
        )
        .await
        .unwrap();
    assert!(matches!(
        reopened.get(&reference).await,
        Err(BridgeError::Corrupt)
    ));
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn explicit_legacy_reimport_rejects_changed_secret_permissions_and_counter_regression() {
    let (client, database, definitions) = fixture().await;
    let svc = service(definitions);
    let id = create_disabled(&svc).await;
    let management = svc.management_lease(&caller(), None, &id, 1).await.unwrap();
    let store =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    let reference = material(&management).metadata.connector.custody;
    let original = store
        .admit(&management, material(&management))
        .await
        .unwrap();
    for scenario in 0..8 {
        let mut changed = material(&management);
        changed.metadata.key_state_version = 7;
        match scenario {
            0 => changed.value = SecretValue::new("fixture-other-selected-key".into()),
            1 => changed.metadata.key_state_version = 2,
            2 => changed.metadata.scopes = "proxy read".into(),
            3 => changed
                .metadata
                .allowed_service_ids
                .push("service-two".into()),
            4 => changed.metadata.allow_all_services = true,
            5 => changed.metadata.allowed_node_ids.push("node-two".into()),
            6 => changed.metadata.allow_all_nodes = true,
            7 => changed.metadata.connector.key_id = "key-two".into(),
            _ => unreachable!(),
        }
        let result = store.admit(&management, with_snapshot(changed)).await;
        assert!(
            matches!(
                result,
                Err(BridgeError::IdempotencyConflict | BridgeError::Conflict)
            ),
            "scenario {scenario}: {result:?}"
        );
        assert_eq!(
            store.get(&reference).await.unwrap().unwrap().metadata,
            original
        );
    }
    let revoked = store
        .revoke(&management, &reference, 1, &"b".repeat(64))
        .await
        .unwrap();
    assert_eq!(
        store
            .admit(&management, with_snapshot(material(&management)))
            .await,
        Err(BridgeError::IdempotencyConflict)
    );
    assert_eq!(
        store.get(&reference).await.unwrap().unwrap().metadata,
        revoked
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn established_snapshot_cannot_be_replaced_downgraded_or_reimported_after_revocation() {
    let (client, database, definitions) = fixture().await;
    let svc = service(definitions);
    let id = create_disabled(&svc).await;
    let management = svc.management_lease(&caller(), None, &id, 1).await.unwrap();
    let store =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    let reference = material(&management).metadata.connector.custody;
    let original = store
        .admit(&management, with_snapshot(material(&management)))
        .await
        .unwrap();
    for scenario in 0..5 {
        let mut changed = with_snapshot(material(&management));
        changed.metadata.key_state_version = 12;
        match scenario {
            0 => {
                changed
                    .metadata
                    .key_authority_v1
                    .as_mut()
                    .unwrap()
                    .rate_limit_burst = Some(20)
            }
            1 => {
                changed
                    .metadata
                    .key_authority_v1
                    .as_mut()
                    .unwrap()
                    .expires_at = Some("2090-01-01T00:00:00Z".into())
            }
            2 => changed.metadata.key_authority_v1 = None,
            3 => changed.metadata.key_state_version = 2,
            4 => {
                changed
                    .metadata
                    .key_authority_v1
                    .as_mut()
                    .unwrap()
                    .allow_auto_connected_services = true
            }
            _ => unreachable!(),
        }
        assert_eq!(
            store.admit(&management, changed).await,
            Err(BridgeError::IdempotencyConflict)
        );
        assert_eq!(
            store.get(&reference).await.unwrap().unwrap().metadata,
            original
        );
    }
    let revoked = store
        .revoke(&management, &reference, 1, &"a".repeat(64))
        .await
        .unwrap();
    assert_eq!(
        store
            .admit(&management, with_snapshot(material(&management)))
            .await,
        Err(BridgeError::IdempotencyConflict)
    );
    assert_eq!(
        store.get(&reference).await.unwrap().unwrap().metadata,
        revoked
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn legacy_snapshot_upgrade_requires_current_disabled_management_fence() {
    let (client, database, definitions) = fixture().await;
    let svc = service(definitions.clone());
    let id = create_disabled(&svc).await;
    let old = svc.management_lease(&caller(), None, &id, 1).await.unwrap();
    let reference = material(&old).metadata.connector.custody;
    let original = definitions
        .credentials
        .admit(&old, material(&old))
        .await
        .unwrap();
    svc.mutate(
        &caller(),
        None,
        change(&id, 1, "enable", BridgeChange::Enable),
    )
    .await
    .unwrap();
    assert_eq!(
        definitions
            .credentials
            .admit(&old, with_snapshot(material(&old)))
            .await,
        Err(BridgeError::Conflict)
    );
    let active = svc.management_lease(&caller(), None, &id, 2).await.unwrap();
    assert_eq!(
        definitions
            .credentials
            .admit(&active, with_snapshot(material(&active)))
            .await,
        Err(BridgeError::Conflict)
    );
    assert_eq!(
        definitions
            .credentials
            .get(&reference)
            .await
            .unwrap()
            .unwrap()
            .metadata,
        original
    );
    client.database(&database).drop().await.unwrap();
}

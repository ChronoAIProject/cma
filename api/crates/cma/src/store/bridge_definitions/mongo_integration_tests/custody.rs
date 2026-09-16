use super::*;
use crate::auth::persistence::SecretValue;
use crate::bridge::custody::{
    BridgeCredentialStore, BridgeCustodyMetadata, StoredBridgeCredential,
};
use crate::store::{AuthEncryptionKey, MongoBridgeCredentialStore};
use base64::{engine::general_purpose::STANDARD, Engine as _};

const SECRET: &str = "fixture-bridge-control-secret";
fn encryption(byte: u8) -> AuthEncryptionKey {
    AuthEncryptionKey::from_base64(&STANDARD.encode([byte; 32])).unwrap()
}

#[path = "custody_grants_tests.rs"]
mod grants;

pub(super) fn material(
    management: &layer2_core::bridge::BridgeManagementLease,
) -> StoredBridgeCredential {
    StoredBridgeCredential {
        metadata: BridgeCustodyMetadata {
            bridge_id: management.record().id.clone(),
            owner: management.record().owner.clone(),
            connector: management
                .record()
                .current()
                .unwrap()
                .config
                .connector
                .clone(),
            revision: 1,
            key_state_version: 3,
            scopes: "proxy read write".into(),
            allowed_service_ids: vec!["service-one".into()],
            allow_all_services: false,
            allowed_node_ids: vec![],
            allow_all_nodes: false,
            created_at: layer2_core::now_rfc3339(),
            created_by: management.actor().clone(),
            revoked_at: None,
            revoked_by: None,
            revocation_key_digest: None,
        },
        value: SecretValue::new(SECRET.into()),
        control_plane: None,
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn encrypted_custody_reopens_replays_and_revokes_without_touching_selected_nyxid_key() {
    let (client, database, definitions) = fixture().await;
    let svc = service(definitions);
    let id = BridgeService::creation_id(&caller(), "create-custody").unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            0,
            "create-custody",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(config()),
            },
        ),
    )
    .await
    .unwrap();
    let management = svc.management_lease(&caller(), None, &id, 1).await.unwrap();
    let store =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    let input = material(&management);
    let reference = input.metadata.connector.custody.clone();
    let original = store.admit(&management, input).await.unwrap();
    assert_eq!(
        store
            .admit(&management, material(&management))
            .await
            .unwrap(),
        original
    );
    let raw = client
        .database(&database)
        .collection::<Document>("bridge_credentials")
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert!(!format!("{raw:?}").contains(SECRET));
    let reopened =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    let retained = reopened.get(&reference).await.unwrap().unwrap();
    assert_eq!(retained.value.expose(), SECRET);
    assert_eq!(retained.metadata, original);
    let revoked = reopened
        .revoke(&management, &reference, 1, &"a".repeat(64))
        .await
        .unwrap();
    assert_eq!(revoked.revision, 2);
    assert!(revoked.revoked_at.is_some());
    assert_eq!(revoked.created_by, original.created_by);
    assert_eq!(revoked.revoked_by, Some(caller().actor));
    assert_eq!(
        reopened
            .revoke(&management, &reference, 1, &"a".repeat(64))
            .await
            .unwrap(),
        revoked
    );
    assert_eq!(
        reopened.admit(&management, material(&management)).await,
        Err(BridgeError::IdempotencyConflict)
    );
    assert_eq!(
        client
            .database(&database)
            .collection::<Document>("bridge_credentials")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn custody_is_bound_to_metadata_deployment_key_and_current_bridge_revision() {
    let (client, database, definitions) = fixture().await;
    let svc = service(definitions);
    let id = BridgeService::creation_id(&caller(), "create-custody").unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            0,
            "create-custody",
            BridgeChange::Create {
                owner: owner(),
                config: Box::new(config()),
            },
        ),
    )
    .await
    .unwrap();
    let old = svc.management_lease(&caller(), None, &id, 1).await.unwrap();
    let store =
        MongoBridgeCredentialStore::open(&client, &database, "fixture-deployment", &encryption(7))
            .await
            .unwrap();
    svc.mutate(
        &caller(),
        None,
        change(
            &id,
            1,
            "emergency",
            BridgeChange::Emergency {
                controls: BridgeEmergencyControls {
                    ingress_stopped: true,
                    ..Default::default()
                },
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        store.admit(&old, material(&old)).await,
        Err(BridgeError::Conflict)
    );
    let current = svc.management_lease(&caller(), None, &id, 2).await.unwrap();
    let reference = material(&current).metadata.connector.custody;
    store.admit(&current, material(&current)).await.unwrap();
    for (deployment, key) in [("another-deployment", 7), ("fixture-deployment", 8)] {
        let other =
            MongoBridgeCredentialStore::open(&client, &database, deployment, &encryption(key))
                .await
                .unwrap();
        assert!(matches!(
            other.get(&reference).await,
            Err(BridgeError::Corrupt)
        ));
    }
    client
        .database(&database)
        .collection::<Document>("bridge_credentials")
        .update_one(
            doc! {"_id":reference.as_str()},
            doc! {"$set":{"metadata.owner.subject":"other-owner"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.get(&reference).await,
        Err(BridgeError::Corrupt)
    ));
    client.database(&database).drop().await.unwrap();
}

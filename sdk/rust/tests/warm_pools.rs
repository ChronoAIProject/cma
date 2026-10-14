#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;

use cma_sdk::{
    Error, IdempotencyKey,
    admin::{ConfigureWarmPoolBody, ResizeWarmPoolBody, WarmPoolStatus},
};
use serde_json::{Value, json};
use wire::{WireServer, json as response};

fn pool() -> Value {
    json!({"id":"wp_original","revision":1,"image":"registry/runtime@sha256:original",
        "config_sha256":"c".repeat(64),"cpu_millis":1000,"memory_mib":1024,"target_replicas":2,"ready_replicas":null,
        "observed":false,"status":"pending","resource_name":null,"error":null})
}

fn configure() -> ConfigureWarmPoolBody {
    ConfigureWarmPoolBody {
        cpu_millis: 1000,
        memory_mib: 1024,
        target_replicas: 2,
    }
}

#[tokio::test]
async fn original_target_creation_and_disable_use_exact_authority_and_preserve_unknown_readiness() {
    let server = WireServer::new().await;
    let mut unobserved = pool();
    unobserved["status"] = json!("unavailable");
    server.expect(response(
        "GET",
        "/api/v1/admin/warm-pools",
        None,
        200,
        json!({"items":[unobserved],"default_image":"registry/runtime@sha256:next",
            "default_cpu_millis":1000,"default_memory_mib":1024,
            "default_config_sha256":"d".repeat(64),"available":false}),
        true,
    ));
    let inventory = server.client.admin().list_warm_pools().await.unwrap();
    assert!(!inventory.available);
    assert_eq!(inventory.items[0].config_sha256, "c".repeat(64));
    assert_eq!(inventory.default_config_sha256, Some("d".repeat(64)));
    assert_eq!(inventory.items[0].ready_replicas, None);
    assert_eq!(inventory.items[0].status, WarmPoolStatus::Unavailable);
    assert_ne!(inventory.items[0].image, inventory.default_image);

    let mut create = response(
        "POST",
        "/api/v1/admin/warm-pools",
        Some(serde_json::to_value(configure()).unwrap()),
        202,
        pool(),
        true,
    );
    create
        .headers
        .insert("idempotency-key".into(), "original-create".into());
    server.expect(create);
    let created = server
        .client
        .admin()
        .configure_warm_pool(
            &configure(),
            &IdempotencyKey::new("original-create").unwrap(),
        )
        .await
        .unwrap();
    let resize = ResizeWarmPoolBody {
        expected_revision: created.revision,
        target_replicas: 0,
    };
    let mut disabled = pool();
    disabled["revision"] = json!(2);
    disabled["target_replicas"] = json!(0);
    disabled["status"] = json!("disabled");
    server.expect(response(
        "PUT",
        "/api/v1/admin/warm-pools/wp_original",
        Some(serde_json::to_value(&resize).unwrap()),
        202,
        disabled,
        true,
    ));
    assert_eq!(
        server
            .client
            .admin()
            .resize_warm_pool(&created.id, &resize)
            .await
            .unwrap()
            .status,
        WarmPoolStatus::Disabled
    );
    server.assert_drained();
    assert_eq!(server.request_count(), 3);
}

#[tokio::test]
async fn resize_rejects_another_pool_receipt() {
    let server = WireServer::new().await;
    let resize = ResizeWarmPoolBody {
        expected_revision: 1,
        target_replicas: 0,
    };
    let mut wrong = pool();
    wrong["id"] = json!("wp_replacement");
    server.expect(response(
        "PUT",
        "/api/v1/admin/warm-pools/wp_original",
        Some(serde_json::to_value(&resize).unwrap()),
        202,
        wrong,
        true,
    ));
    assert!(matches!(
        server
            .client
            .admin()
            .resize_warm_pool("wp_original", &resize)
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn invalid_targets_revisions_and_oversized_creation_keys_never_emit_http() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("creation").unwrap();
    for body in [
        ConfigureWarmPoolBody {
            cpu_millis: 0,
            ..configure()
        },
        ConfigureWarmPoolBody {
            memory_mib: 1_048_577,
            ..configure()
        },
        ConfigureWarmPoolBody {
            target_replicas: 1001,
            ..configure()
        },
    ] {
        assert!(matches!(
            server.client.admin().configure_warm_pool(&body, &key).await,
            Err(Error::Validation { .. })
        ));
    }
    assert!(matches!(
        server
            .client
            .admin()
            .configure_warm_pool(&configure(), &IdempotencyKey::new("x".repeat(129)).unwrap())
            .await,
        Err(Error::Validation { .. })
    ));
    for body in [
        ResizeWarmPoolBody {
            expected_revision: 0,
            target_replicas: 0,
        },
        ResizeWarmPoolBody {
            expected_revision: 1,
            target_replicas: 1001,
        },
        ResizeWarmPoolBody {
            expected_revision: i64::MAX as u64,
            target_replicas: 0,
        },
        ResizeWarmPoolBody {
            expected_revision: u64::MAX,
            target_replicas: 0,
        },
    ] {
        assert!(matches!(
            server
                .client
                .admin()
                .resize_warm_pool("wp_original", &body)
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
}

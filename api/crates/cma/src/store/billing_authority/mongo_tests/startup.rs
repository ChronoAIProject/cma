use super::*;
use crate::health::ReadinessProbe;
use crate::store::mongo::MongoAccountStores;

async fn backend_config(f: &Fixture) -> crate::config::BackendFreeConfig {
    f.db.run_command(crate::store::isolation::role(f.db.name()))
        .await
        .unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    f.db.run_command(doc! {"createUser":"billing-fixture","pwd":&password,
    "roles":[{"role":"cma_am_runtime","db":f.db.name()}]})
        .await
        .unwrap();
    let mut uri = url::Url::parse(&std::env::var("CHRONO_TEST_MONGODB_URI").unwrap()).unwrap();
    uri.set_username("billing-fixture").unwrap();
    uri.set_password(Some(&password)).unwrap();
    uri.query_pairs_mut().append_pair("authSource", f.db.name());
    let config = crate::config::StartupConfig::from_lookup(|key| {
        Ok(match key {
            "CHRONO_SANDBOX_PROVIDER" => Some("none".into()),
            "CHRONO_ENVIRONMENT" => Some("development".into()),
            "CHRONO_AUTH_MODE" => Some("trusted".into()),
            "CHRONO_DEPLOYMENT_ID" => Some("billing-fixture".into()),
            "CHRONO_MONGODB_URI" => Some(uri.to_string()),
            "CHRONO_MONGODB_DATABASE" => Some(f.db.name().into()),
            "CHRONO_AUTH_ENCRYPTION_KEY" => Some(base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                [0_u8; 32],
            )),
            _ => None,
        })
    })
    .unwrap();
    let crate::config::StartupConfig::BackendFree(config) = config else {
        panic!("backend-free fixture");
    };
    *config
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn full_and_backend_free_startup_and_readiness_refuse_active_or_incomplete_authority() {
    let f = Fixture::new().await;
    let full = f.full_stores().await.unwrap();
    let config = backend_config(&f).await;
    let backend = MongoAccountStores::connect(&config).await.unwrap();
    full.ping().await.unwrap();
    backend.ping().await.unwrap();
    assert!(f
        .db
        .collection::<Document>(SETTINGS)
        .find_one(doc! {"_id":format_v1::STATE_ID})
        .await
        .unwrap()
        .is_none());
    f.seed().await;
    f.full_stores().await.unwrap();
    MongoAccountStores::connect(&config).await.unwrap();
    full.ping().await.unwrap();
    backend.ping().await.unwrap();
    for state in [
        doc! {"mode":"nyxid"},
        doc! {"epoch":1_i64},
        doc! {"extra":true},
    ] {
        f.db.collection::<Document>(SETTINGS)
            .update_one(doc! {"_id":format_v1::STATE_ID}, doc! {"$set":state})
            .await
            .unwrap();
        for error in [
            full.ping().await.unwrap_err(),
            backend.ping().await.unwrap_err(),
            f.full_stores().await.err().unwrap(),
            MongoAccountStores::connect(&config).await.err().unwrap(),
        ] {
            assert!(
                matches!(error,StoreError::Malformed(ref reason) if reason.contains("billing authority")),
                "{error}"
            );
        }
        f.db.collection::<Document>(SETTINGS)
            .replace_one(
                doc! {"_id":format_v1::STATE_ID},
                format_v1::proposed_documents()[1].clone(),
            )
            .await
            .unwrap();
    }
    f.db.collection::<Document>(SETTINGS)
        .delete_one(doc! {"_id":format_v1::STATE_ID})
        .await
        .unwrap();
    assert!(full.ping().await.is_err());
    assert!(backend.ping().await.is_err());
    assert!(f.full_stores().await.is_err());
    assert!(MongoAccountStores::connect(&config).await.is_err());
    f.db.drop().await.unwrap();
}

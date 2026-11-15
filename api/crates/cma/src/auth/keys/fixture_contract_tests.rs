//! Production KeyIssuer against the executable counting dependency fixture.
use super::*;
use serde_json::{json, Value};
use std::sync::Arc;

mod process;
use process::{with_provider, Provider};

fn operation() -> String {
    format!("mint_{}", uuid::Uuid::new_v4().simple())
}

fn dispatch(
    provider: &Provider,
    operation: String,
) -> tokio::task::JoinHandle<Result<MintedKey, KeyIssuerError>> {
    let issuer = Arc::clone(provider.issuer());
    let token = provider.token().clone();
    tokio::spawn(async move {
        issuer
            .mint(
                &operation,
                &layer2_core::SubjectId::new("fixture-owner"),
                token.expose(),
                &[ServiceId::new("chrono-llm-public")],
            )
            .await
    })
}

fn counts(value: &Value, received: u64, accepted: u64) {
    assert_eq!(value["received_mint_posts"], received);
    assert_eq!(value["accepted_calls"], accepted);
    assert_eq!(value["physical_commits"], accepted);
    assert_eq!(value["distinct_effects"], accepted);
    assert_eq!(value["replays"], 0);
    assert!(value["attempted_sends"].is_null());
    assert_eq!(value["failed"], false);
}

#[tokio::test]
#[ignore = "requires Node.js executable counting-provider fixture; no Mongo or external provider"]
async fn executable_provider_accepts_exact_production_grants_without_hiding_repeated_posts() {
    with_provider(|provider| {
        Box::pin(async move {
            let rejected = provider
                .issuer()
                .prepare_mint(
                    &operation(),
                    &layer2_core::SubjectId::new("fixture-owner"),
                    provider.token().expose(),
                    &[ServiceId::new("unconfigured-service")],
                )
                .await;
            assert!(matches!(
                rejected,
                Err(KeyIssuerError::ServiceNotGranted(_))
            ));
            counts(&provider.snapshot().await, 0, 0);

            let operation = operation();
            let first = dispatch(provider, operation.clone())
                .await
                .unwrap()
                .unwrap();
            let second = dispatch(provider, operation.clone())
                .await
                .unwrap()
                .unwrap();
            assert!(first.id != second.id);
            assert!(first.value.expose() != second.value.expose());
            assert!(first.matches_grant(&["fixture-service".into()], &["fixture-node".into()]));
            assert!(second.matches_grant(&["fixture-service".into()], &["fixture-node".into()]));
            let keys = provider
                .issuer()
                .owned_keys(provider.token().expose())
                .await
                .unwrap();
            assert_eq!(keys.len(), 2);
            assert!(keys.iter().all(|key| key.credential_id == operation));
            counts(&provider.snapshot().await, 2, 2);
            provider
                .issuer()
                .revoke(&first.id, provider.token().expose())
                .await
                .unwrap();
            let keys = provider
                .issuer()
                .owned_keys(provider.token().expose())
                .await
                .unwrap();
            assert_eq!(keys.len(), 1);
            assert_eq!(keys[0].id, second.id);
            counts(&provider.snapshot().await, 2, 2);
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires Node.js executable counting-provider fixture; no Mongo or external provider"]
async fn executable_provider_lost_response_keeps_one_effect_through_metadata_observations() {
    with_provider(|provider| {
        Box::pin(async move {
            provider.configure(json!({"mint":"hold_response"})).await;
            let operation = operation();
            let mint = dispatch(provider, operation.clone());
            let request = provider.barrier("response_held").await;
            counts(&provider.snapshot().await, 1, 1);
            provider.release(request, "drop").await;
            assert!(matches!(
                mint.await.unwrap(),
                Err(KeyIssuerError::Unavailable(_))
            ));
            let keys = provider
                .issuer()
                .owned_keys(provider.token().expose())
                .await
                .unwrap();
            assert_eq!(keys.len(), 1);
            assert_eq!(keys[0].credential_id, operation);
            for mode in ["empty", "malformed", "unavailable", "metadata"] {
                provider.configure(json!({"listing":mode})).await;
                let observed = provider
                    .issuer()
                    .owned_keys(provider.token().expose())
                    .await;
                match mode {
                    "empty" => assert!(observed.unwrap().is_empty()),
                    "metadata" => assert_eq!(observed.unwrap(), keys),
                    _ => assert!(observed.is_err()),
                }
                counts(&provider.snapshot().await, 1, 1);
            }
        })
    })
    .await;
}

#[tokio::test]
#[ignore = "requires Node.js executable counting-provider fixture; no Mongo or external provider"]
async fn executable_provider_can_accept_after_client_cancellation_and_an_empty_listing() {
    with_provider(|provider| {
        Box::pin(async move {
            provider.configure(json!({"mint":"hold_acceptance"})).await;
            let operation = operation();
            let mint = dispatch(provider, operation.clone());
            let request = provider.barrier("acceptance_held").await;
            mint.abort();
            assert!(mint.await.unwrap_err().is_cancelled());
            assert!(provider
                .issuer()
                .owned_keys(provider.token().expose())
                .await
                .unwrap()
                .is_empty());
            counts(&provider.snapshot().await, 1, 0);
            provider.release(request, "respond").await;
            let keys = provider
                .issuer()
                .owned_keys(provider.token().expose())
                .await
                .unwrap();
            assert_eq!(keys.len(), 1);
            assert_eq!(keys[0].credential_id, operation);
            counts(&provider.snapshot().await, 1, 1);
        })
    })
    .await;
}

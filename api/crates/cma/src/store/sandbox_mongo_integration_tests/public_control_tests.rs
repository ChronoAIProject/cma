//! Real authenticated handlers, durable idempotency and Mongo control transactions.
use super::*;
use crate::store::agent_ownership::MongoAgentOwnership;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
};
use layer2_core::agent_ownership::OwnershipStore;
use tower::ServiceExt;

mod fixture;
mod identity;
use fixture::Fixture;

fn operation(id: &AgentId, deleting: bool) -> (Method, String, AgentWorkStep) {
    if deleting {
        (
            Method::DELETE,
            format!("/api/v1/agents/{id}"),
            AgentWorkStep::EnsureProcessClosed,
        )
    } else {
        (
            Method::POST,
            format!("/api/v1/agents/{id}/stop"),
            AgentWorkStep::FenceInteraction,
        )
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn public_stop_delete_admit_without_owner_and_replay_once_across_ingresses() {
    let f = Fixture::new().await;
    for deleting in [false, true] {
        for first_ingress in 0..2 {
            let agent = f
                .seed(&format!("ownerless-{deleting}-{first_ingress}"), deleting)
                .await;
            let (method, path, expected_step) = operation(&agent.id, deleting);
            for ingress in [first_ingress, 1 - first_ingress] {
                let response = f
                    .request(
                        ingress,
                        method.clone(),
                        &path,
                        Some(&f.identity.owner),
                        (Some("same-operation"), "{}"),
                        None,
                    )
                    .await;
                assert_eq!(response.status(), StatusCode::ACCEPTED);
                let stored = f.stored(&agent.id).await;
                assert_eq!(stored.work.step, expected_step);
                assert_eq!(stored.revision, agent.revision + 1);
                assert!(stored.work.lease.is_none());
                assert!(f.ownership.owner(&agent.id).await.unwrap().is_none());
            }
            let claims = f
                .client
                .database(&f.name)
                .collection::<Document>("operation_claims");
            let scope = format!(
                "agent:{}:{}",
                if deleting { "delete" } else { "stop" },
                agent.id
            );
            assert_eq!(
                claims
                    .count_documents(doc! {"_id.operation": scope})
                    .await
                    .unwrap(),
                1
            );
        }
    }
    assert_eq!(
        *f.registry.admitted_without_owner.lock().unwrap(),
        vec![true, true]
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn public_stop_delete_admit_with_cancelled_same_epoch_owner_without_replacing_lease() {
    let f = Fixture::new().await;
    for deleting in [false, true] {
        let agent = f
            .seed(
                if deleting {
                    "cancel-delete"
                } else {
                    "cancel-stop"
                },
                deleting,
            )
            .await;
        let (owner, authority) = f.acquire(&agent.id).await;
        authority.revoke();
        let before = f.ownership.owner(&agent.id).await.unwrap().unwrap();
        let (method, path, step) = operation(&agent.id, deleting);
        for ingress in [owner, 1 - owner] {
            assert_eq!(
                f.request(
                    ingress,
                    method.clone(),
                    &path,
                    Some(&f.identity.owner),
                    (Some("cancelled-owner"), "{}"),
                    None
                )
                .await
                .status(),
                StatusCode::ACCEPTED
            );
            assert_eq!(f.stored(&agent.id).await.work.step, step);
            assert_eq!(
                f.ownership.owner(&agent.id).await.unwrap().unwrap().token,
                before.token
            );
            assert!(!authority.is_current());
        }
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn public_stop_delete_admit_at_local_and_remote_ingress_with_a_healthy_owner() {
    let f = Fixture::new().await;
    for deleting in [false, true] {
        for local_first in [false, true] {
            let agent = f
                .seed(&format!("healthy-{deleting}-{local_first}"), deleting)
                .await;
            let (owner, authority) = f.acquire(&agent.id).await;
            let before = f.ownership.owner(&agent.id).await.unwrap().unwrap();
            let first = if local_first { owner } else { 1 - owner };
            let (method, path, step) = operation(&agent.id, deleting);
            for ingress in [first, 1 - first] {
                assert_eq!(
                    f.request(
                        ingress,
                        method.clone(),
                        &path,
                        Some(&f.identity.owner),
                        (Some("healthy-owner"), "{}"),
                        None
                    )
                    .await
                    .status(),
                    StatusCode::ACCEPTED
                );
                let stored = f.stored(&agent.id).await;
                assert_eq!(stored.work.step, step);
                assert_eq!(stored.revision, agent.revision + 1);
                assert_eq!(f.ownership.owner(&agent.id).await.unwrap().unwrap(), before);
                assert!(authority.is_current());
            }
        }
    }
    assert_eq!(
        *f.registry.admitted_without_owner.lock().unwrap(),
        vec![true, true]
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn public_stop_response_survives_owner_cancellation_after_durable_commit() {
    let f = Fixture::new().await;
    let agent = f.seed("post-commit", false).await;
    let (owner, authority) = f.acquire(&agent.id).await;
    *f.registry.cancel.lock().unwrap() = Some(authority.clone());
    let (method, path, _) = operation(&agent.id, false);
    assert_eq!(
        f.request(
            owner,
            method.clone(),
            &path,
            Some(&f.identity.owner),
            (Some("commit-cancel"), "{}"),
            None
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    assert!(!authority.is_current());
    let committed = f.stored(&agent.id).await;
    assert_eq!(committed.revision, agent.revision + 1);
    assert_eq!(committed.work.step, AgentWorkStep::FenceInteraction);
    assert_eq!(
        f.request(
            1 - owner,
            method,
            &path,
            Some(&f.identity.owner),
            (Some("commit-cancel"), "{}"),
            None
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(f.stored(&agent.id).await, committed);
    assert_eq!(
        *f.registry.admitted_without_owner.lock().unwrap(),
        vec![true]
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn public_stop_delete_preserve_authentication_capability_ownership_and_proof_refusals() {
    let f = Fixture::new().await;
    for deleting in [false, true] {
        let agent = f
            .seed(
                if deleting {
                    "denied-delete"
                } else {
                    "denied-stop"
                },
                deleting,
            )
            .await;
        let (method, path, _) = operation(&agent.id, deleting);
        for ingress in 0..2 {
            for (token, expected) in [
                (None, StatusCode::UNAUTHORIZED),
                (Some(f.identity.denied.as_str()), StatusCode::FORBIDDEN),
                (Some(f.identity.foreign.as_str()), StatusCode::NOT_FOUND),
            ] {
                assert_eq!(
                    f.request(
                        ingress,
                        method.clone(),
                        &path,
                        token,
                        (Some("denied"), "{}"),
                        None
                    )
                    .await
                    .status(),
                    expected
                );
                assert_eq!(f.stored(&agent.id).await, agent);
            }
            // A presented proof may not take the new proof-free bypass.
            assert_eq!(
                f.request(
                    ingress,
                    method.clone(),
                    &path,
                    Some(&f.identity.owner),
                    (Some("forged"), "{}"),
                    Some("forged")
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(f.stored(&agent.id).await, agent);
            assert!(f.ownership.owner(&agent.id).await.unwrap().is_none());
        }
    }
    assert!(f.registry.admitted_without_owner.lock().unwrap().is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn public_stop_delete_preserve_validation_domain_conflict_and_request_identity() {
    let f = Fixture::new().await;
    let agent = f.seed("validation", false).await;
    let (method, path, _) = operation(&agent.id, false);
    for ingress in 0..2 {
        for (key, body) in [
            (Some(""), "{}"),
            (Some("invalid"), "{"),
            (Some("invalid"), "{\"unknown\":true}"),
        ] {
            let response = f
                .request(
                    ingress,
                    method.clone(),
                    &path,
                    Some(&f.identity.owner),
                    (key, body),
                    None,
                )
                .await;
            assert!(response.status().is_client_error());
            assert_eq!(f.stored(&agent.id).await, agent);
        }
        let (delete, delete_path, _) = operation(&agent.id, true);
        assert_eq!(
            f.request(
                ingress,
                delete,
                &delete_path,
                Some(&f.identity.owner),
                (Some("live-delete"), ""),
                None
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
        let invalid_path = format!("{path}/extra");
        assert_eq!(
            f.request(
                ingress,
                Method::POST,
                &invalid_path,
                Some(&f.identity.owner),
                (Some("wrong-path"), "{}"),
                None
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(f.stored(&agent.id).await, agent);
    }
    assert_eq!(
        f.request(
            0,
            method.clone(),
            &path,
            Some(&f.identity.owner),
            (Some("body-bound"), "{}"),
            None
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    let committed = f.stored(&agent.id).await;
    let conflict = f
        .request(
            1,
            method,
            &path,
            Some(&f.identity.owner),
            (Some("body-bound"), "{\"reason\":\"owner\"}"),
            None,
        )
        .await;
    assert_eq!(conflict.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(conflict.into_body(), 8192).await.unwrap()).unwrap();
    assert!(value.to_string().contains("idempotency"));
    assert_eq!(f.stored(&agent.id).await, committed);
    f.close().await;
}

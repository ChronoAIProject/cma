//! Real public share/artifact handlers, with per-peer observed mutation stores.
use super::*;
use layer2_core::{
    agent_ownership::{test_support::MemoryOwnershipStore, *},
    resource_access::*,
    sandbox::{agent_artifacts::*, store::InMemorySandboxStore, *},
};
use serde_json::{json, Value};
use std::sync::Mutex;
mod fixture;
use fixture::Cluster;

#[tokio::test]
async fn share_links_redeem_without_prior_access_only_on_the_resolved_owner() {
    let cluster = Cluster::new("recipient").await;
    let share = cluster.link().await;
    let before = cluster
        .inner
        .resource_access_snapshot(&cluster.target())
        .await
        .unwrap();
    let recipient = fixture::caller("recipient");
    let decision = AccessDecision::evaluate(
        &recipient,
        &before.scope(&cluster.target()).unwrap(),
        &before.shares,
        AccessPurpose::Read,
        SystemTime::now(),
    )
    .unwrap();
    assert!(!decision.allows(ResourceCapability::ManageSharing));
    assert!(!decision.allows(ResourceCapability::ViewDetails));
    let (owner, authority) = cluster.owner().await;
    let path = format!("/api/v1/agent-share-links/{}/redeem", share.id);
    let first = cluster
        .request(
            1 - owner,
            Method::POST,
            &path,
            "unused",
            json!({"token":fixture::TOKEN}),
        )
        .await;
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    let replay = cluster
        .request(
            owner,
            Method::POST,
            &path,
            "unused",
            json!({"token":fixture::TOKEN}),
        )
        .await;
    assert_eq!(replay, first);
    assert_eq!(first.1["accepted_count"], 1);
    assert_eq!(cluster.writes[1 - owner].effects.lock().unwrap().len(), 0);
    assert_eq!(
        cluster.writes[owner].effects.lock().unwrap().as_slice(),
        &[authority.token.clone()]
    );
    let retained = cluster
        .inner
        .resource_share(&share.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.revision, share.revision + 1);
}

#[tokio::test]
async fn invalid_and_revoked_redemption_never_claim_or_write() {
    let cluster = Cluster::new("recipient").await;
    let share = cluster.link().await;
    let path = format!("/api/v1/agent-share-links/{}/redeem", share.id);
    for ingress in 0..2 {
        let response = cluster
            .request(
                ingress,
                Method::POST,
                &path,
                "unused",
                json!({"token":"b".repeat(64)}),
            )
            .await;
        assert_eq!(response.0, StatusCode::NOT_FOUND);
    }
    cluster
        .direct
        .change_share(
            &fixture::caller("owner"),
            None,
            ShareCommand {
                selector: ResourceSelector::Agent {
                    agent_id: cluster.id.clone(),
                },
                change: ShareChange::Revoke {
                    share_id: share.id.clone(),
                },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-link".into(),
            },
        )
        .await
        .unwrap();
    for ingress in 0..2 {
        let response = cluster
            .request(
                ingress,
                Method::POST,
                &path,
                "unused",
                json!({"token":fixture::TOKEN}),
            )
            .await;
        assert_eq!(response.0, StatusCode::NOT_FOUND);
    }
    assert!(cluster
        .ownership
        .owner(&cluster.id)
        .await
        .unwrap()
        .is_none());
    assert!(cluster
        .writes
        .iter()
        .all(|store| store.effects.lock().unwrap().is_empty()));
}

#[tokio::test]
async fn real_link_creation_replay_update_stale_revision_and_revoke_keep_the_owner() {
    let cluster = Cluster::new("owner").await;
    let (owner, authority) = cluster.owner().await;
    let path = format!("/api/v1/agents/{}/share-links", cluster.id);
    let body = json!({"audience":{"kind":"public"}, "mode":"view", "token":fixture::TOKEN});
    let created = cluster
        .request(1 - owner, Method::POST, &path, "same-link", body.clone())
        .await;
    assert_eq!(created.0, StatusCode::OK, "{}", created.1);
    assert_eq!(
        cluster
            .request(owner, Method::POST, &path, "same-link", body)
            .await,
        created
    );
    let share_id = created.1["id"].as_str().unwrap();
    let revision = created.1["revision"].as_u64().unwrap();
    let mutation_path = format!("/api/v1/resource-shares/agent/{}/{share_id}", cluster.id);
    // An explicit stale revision remains a refusal, rather than being refreshed.
    let stale = cluster
        .request(
            1 - owner,
            Method::PATCH,
            &mutation_path,
            "stale-mode",
            json!({"mode":"view", "expected_revision":revision+10}),
        )
        .await;
    assert_eq!(stale.0, StatusCode::CONFLICT);
    let update = cluster
        .request(
            1 - owner,
            Method::PATCH,
            &mutation_path,
            "change-mode",
            json!({"mode":"view", "expected_revision":revision}),
        )
        .await;
    assert_eq!(update.0, StatusCode::OK, "{}", update.1);
    let retained = cluster
        .inner
        .resource_share(share_id)
        .await
        .unwrap()
        .unwrap();
    let revoke_path = format!(
        "/api/v1/agents/{}/share-links/{share_id}?expected_revision={}",
        cluster.id, retained.revision
    );
    assert_eq!(
        cluster
            .request(
                1 - owner,
                Method::DELETE,
                &revoke_path,
                "remove-link",
                json!({})
            )
            .await
            .0,
        StatusCode::OK
    );
    assert!(cluster
        .inner
        .resource_share(share_id)
        .await
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert!(cluster.writes[1 - owner].effects.lock().unwrap().is_empty());
    assert!(cluster.writes[owner]
        .effects
        .lock()
        .unwrap()
        .iter()
        .all(|token| token == &authority.token));
}

#[tokio::test]
async fn real_artifact_publication_preserves_receipt_and_rejects_changed_content() {
    let cluster = Cluster::new("owner").await;
    let (owner, authority) = cluster.owner().await;
    let path = format!("/api/v1/agents/{}/artifacts", cluster.id);
    let body =
        json!({"name":"evidence.txt", "media_type":"text/plain", "content_base64":"b3duZXI="});
    let created = cluster
        .request(
            1 - owner,
            Method::POST,
            &path,
            "same-artifact",
            body.clone(),
        )
        .await;
    assert_eq!(created.0, StatusCode::CREATED, "{}", created.1);
    assert_eq!(
        cluster
            .request(owner, Method::POST, &path, "same-artifact", body)
            .await,
        created
    );
    let changed = cluster.request(1-owner, Method::POST, &path, "same-artifact", json!({"name":"evidence.txt", "media_type":"text/plain", "content_base64":"Y2hhbmdlZA=="})).await;
    assert_eq!(changed.0, StatusCode::CONFLICT);
    assert!(cluster.writes[1 - owner].effects.lock().unwrap().is_empty());
    assert_eq!(cluster.writes[owner].effects.lock().unwrap().len(), 3);
    assert!(cluster.writes[owner]
        .effects
        .lock()
        .unwrap()
        .iter()
        .all(|token| token == &authority.token));
}

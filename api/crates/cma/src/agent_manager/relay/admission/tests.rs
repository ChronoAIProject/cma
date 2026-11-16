use super::*;
use std::sync::atomic::Ordering;
pub(super) mod fixture;
use fixture::Cluster;

#[tokio::test]
async fn either_ingress_executes_the_real_admission_only_at_the_current_positive_owner() {
    let cluster = Cluster::new().await;
    let (owner, authority) = cluster.owner().await;
    let first = cluster.admit(1 - owner, "peer-command").await.unwrap();
    let replay = cluster.admit(owner, "peer-command").await.unwrap();
    assert_eq!(first.receipt, replay.receipt);
    assert!(cluster.peers[1 - owner]
        .writes
        .calls
        .lock()
        .unwrap()
        .is_empty());
    assert!(cluster.peers[owner]
        .writes
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|token| token == &authority.token));
    let mut changed = cluster.submission("peer-command");
    changed.command = AgentCommand::message("changed input must refuse");
    let result = cluster
        .evidence()
        .scope(cluster.peers[1 - owner].service.admit(
            &cluster.auth.context,
            "test-access",
            CommandSubmissionSource::Api,
            changed,
        ))
        .await;
    assert!(matches!(
        result,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::IdempotencyConflict
        ))
    ));
}

#[tokio::test]
async fn lost_reply_after_real_peer_commit_never_falls_back_or_retries_automatically() {
    let cluster = Cluster::new().await;
    let (owner, _) = cluster.owner().await;
    cluster.peers[owner]
        .drop_reply
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        cluster.admit(1 - owner, "lost-reply").await,
        Err(CommandAdmissionError::Ledger(
            CommandLedgerError::Unavailable
        ))
    ));
    assert_eq!(cluster.peers[owner].writes.calls.lock().unwrap().len(), 1);
    assert!(cluster.peers[1 - owner]
        .writes
        .calls
        .lock()
        .unwrap()
        .is_empty());
    let recovered = cluster.admit(1 - owner, "lost-reply").await.unwrap();
    let replay = cluster.admit(owner, "lost-reply").await.unwrap();
    assert_eq!(recovered.receipt, replay.receipt);
    assert_eq!(recovered.receipt.agent_order, 1);
}

#[tokio::test]
async fn original_browser_logout_refuses_before_any_peer_admission_effect() {
    use crate::auth::persistence::{AuthStore, SecretDigest};
    let cluster = Cluster::new().await;
    let (owner, _) = cluster.owner().await;
    let login = SecretDigest::identify("browser-login", "test-login");
    cluster
        .auth
        .store
        .delete_browser_login(&login)
        .await
        .unwrap();
    assert!(cluster.admit(1 - owner, "logged-out").await.is_err());
    assert!(cluster
        .peers
        .iter()
        .all(|peer| peer.writes.calls.lock().unwrap().is_empty()));
}

#[tokio::test]
async fn signed_ownerless_invitation_reauthorizes_before_it_can_claim_an_agent() {
    use axum::http::{HeaderMap, Method};
    use layer2_core::agent_ownership::{candidates, OwnershipStore};
    let cluster = Cluster::new().await;
    let membership = cluster.ownership.membership().await.unwrap();
    let selected = candidates(&cluster.id, &membership, None).unwrap()[0]
        .instance_id
        .clone();
    let owner = cluster
        .peers
        .iter()
        .position(|peer| peer.coordinator.instance_id == selected)
        .unwrap();
    let source = &cluster.peers[1 - owner];
    let identity = cluster
        .auth
        .auth
        .capture_owner_admission(&cluster.auth.context, "test-access")
        .await
        .unwrap();
    let call = Call {
        identity,
        credential: "wrong-credential".into(),
        origin: None,
        source: Source::Api,
        intent: OwnerCommandMutation::Admit {
            submission: cluster.submission("forged"),
            binding_policy: None,
        }
        .into(),
    };
    let body = serde_json::to_vec(&call).unwrap();
    let uri: axum::http::Uri = format!("{PREFIX}/control/{}/admission/public", cluster.id)
        .parse()
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse().unwrap());
    let token = OwnershipToken {
        agent_id: cluster.id.clone(),
        instance_id: selected,
        epoch: 0,
        protocol: layer2_core::agent_ownership::PROTOCOL_VERSION,
    };
    let proof = source
        .relay
        .authentication
        .sign(
            &source.coordinator.instance_id,
            &token,
            &Method::POST,
            &uri,
            &headers,
            &body,
        )
        .unwrap();
    let target = &membership
        .members
        .iter()
        .find(|member| member.instance_id == token.instance_id)
        .unwrap()
        .relay_address;
    let response = reqwest::Client::new()
        .post(format!("{target}{uri}"))
        .headers(headers)
        .header(PROOF, proof)
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    assert!(cluster
        .ownership
        .owner(&cluster.id)
        .await
        .unwrap()
        .is_none());
    assert!(cluster
        .peers
        .iter()
        .all(|peer| peer.writes.calls.lock().unwrap().is_empty()));
}

#[tokio::test]
async fn missing_public_request_evidence_cannot_use_retained_custody_as_a_fallback() {
    use layer2_core::agent_ownership::OwnershipStore;
    let cluster = Cluster::new().await;
    let result = cluster.peers[0]
        .service
        .admit(
            &cluster.auth.context,
            "test-access",
            CommandSubmissionSource::Api,
            cluster.submission("missing-origin"),
        )
        .await;
    assert!(matches!(
        result,
        Err(CommandAdmissionError::Authority(
            CommandAuthorizationError::Revoked
        ))
    ));
    assert!(cluster
        .ownership
        .owner(&cluster.id)
        .await
        .unwrap()
        .is_none());
    assert!(cluster
        .peers
        .iter()
        .all(|peer| peer.writes.calls.lock().unwrap().is_empty()));
}

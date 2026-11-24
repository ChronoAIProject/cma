use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn segmented_controls_admit_directly_with_ownership_effect_fencing() {
    for deleting in [false, true] {
        let f = Fixture::with_modes(deleting, true, true, Some("owner")).await;
        TransportAuthority::revoke(f.authority.as_ref());
        for ingress in [f.owner, 1 - f.owner] {
            let result = f
                .request("segmented_cancelled_live_lease", ingress, 202, None)
                .await;
            assert_eq!(result["log_format"], "segmented_v1");
        }
        f.assert_pending_while_cancelled("segmented_cancelled_live_lease")
            .await;
        f.replicas[f.owner]
            .coordinator
            .handoff(&f.authority, f.authority.revoke_transports())
            .await
            .unwrap();
        f.acquire_successor_for_worker().await;
        f.complete().await;
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn real_authentication_authorization_and_forged_proof_cannot_admit_controls() {
    for deleting in [false, true] {
        // Public access rejects attempts to route these direct durable controls
        // before peer-proof validation. Stop's suffix is unsupported (405);
        // Delete has no runtime suffix and fails the path boundary (403).
        for (identity, proof, status) in [
            (None, None, 401),
            (Some("outsider"), None, 404),
            (
                Some("owner"),
                Some("forged"),
                if deleting { 403 } else { 405 },
            ),
        ] {
            let f = Fixture::with_modes(deleting, false, true, identity).await;
            for ingress in [f.owner, 1 - f.owner] {
                let url = format!(
                    "{}/api/v1/agents/{}{}",
                    f.replicas[ingress].address,
                    f.agent.id,
                    if deleting { "" } else { "/stop" }
                );
                let mut request = reqwest::Client::new()
                    .request(
                        if deleting {
                            Method::DELETE
                        } else {
                            Method::POST
                        },
                        url,
                    )
                    .header("content-type", "application/json")
                    .header("idempotency-key", uuid::Uuid::new_v4().to_string())
                    .body("{}");
                if let Some(proof) = proof {
                    request = request.header("x-cma-agent-manager-relay", proof);
                }
                let response = request.send().await.unwrap();
                let actual = response.status().as_u16();
                assert_eq!(actual, status, "{}", response.text().await.unwrap());
                assert_eq!(f.h.stored(&f.agent.id).await.revision, f.agent.revision);
                assert!(f.control_receipt().await.is_none());
            }
            f.close().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn wrong_receiver_ack_keeps_custody_and_late_exact_ack_cannot_release_successor() {
    let f = Fixture::new(false).await;
    let (receiver, ack, server) = receiver(&f.authority).await;
    f.request("wrong_ack_pending", 1 - f.owner, 202, None).await;
    ack.store(true, Ordering::SeqCst);
    receiver.wrong_ack.store(true, Ordering::SeqCst);
    assert!(f.replicas[f.owner]
        .coordinator
        .handoff(&f.authority, f.authority.revoke_transports())
        .await
        .is_err());
    assert_eq!(
        f.ownership.owner(&f.agent.id).await.unwrap().unwrap().token,
        f.authority.token
    );
    f.assert_pending_while_cancelled("wrong_ack_pending").await;
    receiver.wrong_ack.store(false, Ordering::SeqCst);
    f.replicas[f.owner]
        .coordinator
        .handoff(&f.authority, f.authority.revoke_transports())
        .await
        .unwrap();
    f.acquire_successor_for_worker().await;
    let successor = f.ownership.owner(&f.agent.id).await.unwrap().unwrap().token;
    f.replicas[f.owner]
        .coordinator
        .handoff(&f.authority, f.authority.revoke_transports())
        .await
        .unwrap();
    assert_eq!(
        f.ownership.owner(&f.agent.id).await.unwrap().unwrap().token,
        successor
    );
    f.complete().await;
    server.abort();
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn non_owner_stop_admission_does_not_claim_remote_attachment_termination() {
    let f = Fixture::new(false).await;
    let target = Harness::target(&f.agent);
    assert!(f.h.registry.resolve(&target).is_some());
    let before = f.h.backend.requests().await.len();
    f.request("non_owner_stop_remote_attachment", 1 - f.owner, 202, None)
        .await;
    // The public Stop fence ran on the ingress-local empty registry. The
    // remote owner's attachment remains until its valid worker executes.
    assert!(f.h.registry.resolve(&target).is_some());
    assert_eq!(f.h.backend.requests().await.len(), before);
    assert_ne!(f.h.stored(&f.agent.id).await.work.step, AgentWorkStep::None);
    f.complete().await;
    assert!(f.h.registry.resolve(&target).is_none());
    f.close().await;
}

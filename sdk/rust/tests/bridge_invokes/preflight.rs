use super::*;

#[tokio::test]
async fn invalid_ids_limits_targets_keys_and_revisions_do_not_emit_http() {
    let server = WireServer::new().await;
    for id in [
        "".to_owned(),
        "brinv_bad".to_owned(),
        format!("brinv_{}", "C".repeat(64)),
    ] {
        assert!(matches!(
            server.client.bridges().read_invoke_grant(&id).await,
            Err(Error::Validation { .. })
        ));
        assert!(matches!(
            server
                .client
                .bridges()
                .revoke_invoke_grant(&id, &revoke(), &key())
                .await,
            Err(Error::Validation { .. })
        ));
        assert!(matches!(
            server
                .client
                .bridges()
                .list_invoke_grants(
                    "brg-1",
                    &PageOptions {
                        cursor: Some(id),
                        limit: None
                    }
                )
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for limit in [0, 101] {
        assert!(matches!(
            server
                .client
                .bridges()
                .list_invoke_grants(
                    "brg-1",
                    &PageOptions {
                        cursor: None,
                        limit: Some(limit)
                    }
                )
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for revision in [0, u64::MAX] {
        assert!(matches!(
            server
                .client
                .bridges()
                .issue_invoke_grant(
                    "brg-1",
                    &IssueBridgeInvokeGrantRequest {
                        expected_bridge_revision: revision,
                        ..issue()
                    },
                    &key()
                )
                .await,
            Err(Error::Validation { .. })
        ));
        assert!(matches!(
            server
                .client
                .bridges()
                .revoke_invoke_grant(
                    &grant_id(),
                    &RevokeBridgeInvokeGrantRequest {
                        expected_revision: revision,
                        ..revoke()
                    },
                    &key()
                )
                .await,
            Err(Error::Validation { .. })
        ));
    }
    let mut body = issue();
    body.target.agent_id = "bad target".into();
    assert!(matches!(
        server
            .client
            .bridges()
            .issue_invoke_grant("brg-1", &body, &key())
            .await,
        Err(Error::Validation { .. })
    ));
    let oversized = IdempotencyKey::new("k".repeat(201)).unwrap();
    assert!(matches!(
        server
            .client
            .bridges()
            .issue_invoke_grant("brg-1", &issue(), &oversized)
            .await,
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        server
            .client
            .bridges()
            .revoke_invoke_grant(&grant_id(), &revoke(), &oversized)
            .await,
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        server.client.bridges().invoke_readiness("bad bridge").await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 0);
}

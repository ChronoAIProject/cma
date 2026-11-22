use super::*;
use layer2_core::resource_access::*;
use layer2_core::{Caller, SubjectId};

fn context(subject: &str) -> CallerContext {
    let subject = SubjectId::new(subject);
    CallerContext {
        caller: Caller::new(
            subject.clone(),
            crate::auth::AuthState::trusted_authorization(),
        ),
        credential_source: CredentialSource::Retained,
        credential: CredentialContext {
            issuer: "test".into(),
            subject: subject.clone(),
            credential_id: "settings-fixture".into(),
        },
        actor: ResourceActor::Api {
            subject,
            delegation_id: None,
        },
        memberships: vec![],
    }
}

async fn share(access: &ResourceAccessService, mode: ShareMode) -> ResourceShare {
    let token = "b".repeat(64);
    let receipt = access
        .create_agent_share_link(
            &context(OWNER),
            None,
            CreateAgentShareLink {
                agent_id: AgentId(AGENT.into()),
                audience: AgentShareAudience::Public,
                mode,
                token: token.clone(),
                idempotency_key: "settings-share".into(),
            },
        )
        .await
        .unwrap();
    access
        .redeem_agent_share_link(&context("settings-viewer"), None, &receipt.share.id, &token)
        .await
        .unwrap();
    access
        .read_share(
            &context(OWNER),
            None,
            &receipt.share.selector,
            &receipt.share.id,
        )
        .await
        .unwrap()
}

pub(super) async fn revoke(access: &ResourceAccessService, share: &ResourceShare) {
    access
        .change_share(
            &context(OWNER),
            None,
            ShareCommand {
                selector: share.selector.clone(),
                change: ShareChange::Revoke {
                    share_id: share.id.clone(),
                },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-settings-share".into(),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn routed_settings_require_current_write_share_including_cached_success() {
    for mode in [ShareMode::View, ShareMode::ViewWrite] {
        let (mongo, name, durable) =
            crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
        seed(&durable).await;
        let replicas = vec![
            Replica::open_as(&name, true, "settings-viewer").await,
            Replica::open_as(&name, true, "settings-viewer").await,
        ];
        let granted = share(&replicas[0].access, mode).await;
        let (owner, _) = fixture::acquire(&replicas).await;
        let before = durable
            .get_agent(&AgentId(AGENT.into()))
            .await
            .unwrap()
            .unwrap();
        let result = replicas[1 - owner]
            .request(Method::PATCH, Some("shared-settings"), Some(changed()))
            .await;
        if mode == ShareMode::ViewWrite {
            assert_eq!(result.0, StatusCode::OK, "{}", result.1);
            assert_eq!(
                replicas[owner]
                    .request(Method::PATCH, Some("shared-settings"), Some(changed()))
                    .await,
                result
            );
            assert_eq!(replicas[owner].runtime.inputs.lock().unwrap().len(), 1);
        } else {
            assert!(result.0.is_client_error(), "{}", result.1);
            assert_eq!(
                durable.get_agent(&before.id).await.unwrap().unwrap(),
                before
            );
            assert!(replicas
                .iter()
                .all(|replica| replica.scopes.lock().unwrap().is_empty()));
        }
        let admitted = durable.get_agent(&before.id).await.unwrap().unwrap();
        revoke(&replicas[0].access, &granted).await;
        for replica in &replicas {
            let denied = replica
                .request(Method::PATCH, Some("shared-settings"), Some(changed()))
                .await;
            assert!(denied.0.is_client_error(), "{}", denied.1);
        }
        assert_eq!(
            durable.get_agent(&before.id).await.unwrap().unwrap(),
            admitted
        );
        assert_eq!(
            replicas[owner].runtime.inputs.lock().unwrap().len(),
            usize::from(mode == ShareMode::ViewWrite)
        );
        assert!(replicas[1 - owner]
            .runtime
            .inputs
            .lock()
            .unwrap()
            .is_empty());
        for replica in replicas {
            replica.close().await;
        }
        mongo.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn share_revoked_after_owner_body_authorization_cannot_admit_settings() {
    let (mongo, name, durable) =
        crate::store::sandbox_maintenance::mongo_integration_tests::database().await;
    seed(&durable).await;
    let replicas = vec![
        Replica::open_as(&name, true, "settings-viewer").await,
        Replica::open_as(&name, true, "settings-viewer").await,
    ];
    let granted = share(&replicas[0].access, ShareMode::ViewWrite).await;
    let (owner, _) = fixture::acquire(&replicas).await;
    let before = durable
        .get_agent(&AgentId(AGENT.into()))
        .await
        .unwrap()
        .unwrap();
    *replicas[owner].runtime.revocation.lock().unwrap() =
        Some((replicas[owner].access.clone(), granted));
    let (status, problem) = replicas[1 - owner]
        .request(Method::PATCH, Some("revoke-after-body"), Some(changed()))
        .await;
    assert!(status.is_client_error(), "{problem}");
    assert!(replicas[owner].runtime.revocation.lock().unwrap().is_none());
    assert!(!replicas[owner]
        .runtime
        .resolutions
        .lock()
        .unwrap()
        .is_empty());
    assert!(replicas
        .iter()
        .all(|replica| replica.runtime.inputs.lock().unwrap().is_empty()));
    assert_eq!(
        durable.get_agent(&before.id).await.unwrap().unwrap(),
        before
    );
    for replica in replicas {
        replica.close().await;
    }
    mongo.database(&name).drop().await.unwrap();
}

use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn disabled_bridge_can_record_explicit_consent_and_readiness_never_restores_revoked_authority(
) {
    let mut f = Fixture::new().await;
    f.definitions
        .mutate(
            &caller(),
            None,
            BridgeMutation {
                id: f.bridge.id.clone(),
                expected_revision: 2,
                idempotency_key: "disable".into(),
                change: BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::DrainOriginal,
                },
            },
        )
        .await
        .unwrap();
    f.bridge = f.bridges.bridge(&f.bridge.id).await.unwrap().unwrap();
    let management = f
        .definitions
        .management_lease(&caller(), None, &f.bridge.id, 3)
        .await
        .unwrap();
    assert!(f
        .service
        .current_candidate(&management)
        .await
        .unwrap()
        .is_none());
    let grant = f
        .service
        .issue(&caller(), None, f.issue("disabled-consent"))
        .await
        .unwrap()
        .result;
    assert_eq!(
        f.service.current_candidate(&management).await.unwrap(),
        Some(grant.clone())
    );
    assert!(f
        .service
        .authorize(f.request(
            &grant,
            BridgeInvokeEffect::BindExistingTarget {
                allocation_id: "disabled-allocation".into()
            }
        ))
        .await
        .is_err());
    f.service
        .revoke(&caller(), None, f.revoke(&grant, "revoke-disabled"))
        .await
        .unwrap();
    assert!(f
        .service
        .current_candidate(&management)
        .await
        .unwrap()
        .is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn configuration_refresh_keeps_original_grant_and_explicit_drain_never_rebinds_new_generation(
) {
    let mut f = Fixture::new().await;
    let grant = f
        .service
        .issue(&caller(), None, f.issue("consent"))
        .await
        .unwrap()
        .result;
    f.enable("enable").await;
    let mut config = f.bridge.current().unwrap().config.clone();
    config.display_name = "Refreshed".into();
    f.definitions
        .mutate(
            &caller(),
            None,
            BridgeMutation {
                id: f.bridge.id.clone(),
                expected_revision: 3,
                idempotency_key: "refresh".into(),
                change: BridgeChange::Update {
                    config: Box::new(config),
                    previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.store.grant(&grant.id).await.unwrap().unwrap().view(),
        grant
    );
    let proof = f
        .service
        .authorize(f.request(
            &grant,
            BridgeInvokeEffect::AdmitMessage {
                inbox_id: "retained-inbox".into(),
            },
        ))
        .await
        .unwrap();
    f.effect(&proof).await.unwrap();
    assert!(matches!(
        f.service
            .authorize(f.request(
                &grant,
                BridgeInvokeEffect::BindExistingTarget {
                    allocation_id: "new-allocation".into()
                }
            ))
            .await,
        Err(BridgeInvokeError::Suspended)
    ));
    f.definitions
        .mutate(
            &caller(),
            None,
            BridgeMutation {
                id: f.bridge.id.clone(),
                expected_revision: 4,
                idempotency_key: "hold".into(),
                change: BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(f.effect(&proof).await, Err(BridgeInvokeError::Conflict));
    assert!(matches!(
        f.service
            .authorize(f.request(
                &grant,
                BridgeInvokeEffect::AdmitMessage {
                    inbox_id: "retained-inbox".into()
                }
            ))
            .await,
        Err(BridgeInvokeError::Suspended)
    ));
    f.close().await;
}

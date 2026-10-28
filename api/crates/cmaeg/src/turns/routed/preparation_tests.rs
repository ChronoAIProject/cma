use super::{test_support::*, *};
use std::sync::atomic::Ordering;

fn intent(setup: &Setup) -> PreparationIntent {
    PreparationIntent {
        version: 1,
        admission_id: setup.plan.admission_id.clone(),
        target_id: setup.target().id.clone(),
        request_hash: setup.plan.request_hash.clone(),
        binding_id: setup.target().launch.provider.binding_id.to_string(),
        owner: setup.plan.identity.owner.clone(),
        manager: "manager".into(),
        generation: 3,
        binding_revision: 14,
        binding_digest: "a".repeat(64),
        sandbox_digest: "b".repeat(64),
        workspace_digest: "c".repeat(64),
        admitted_at: setup.plan.admitted_at,
        expires_at: setup
            .target()
            .launch
            .event
            .expires_at
            .min(setup.plan.retained_until),
    }
}
fn receipt(intent: &PreparationIntent) -> PreparationReceipt {
    PreparationReceipt {
        version: 1,
        operation_id: intent.operation_id().unwrap(),
        intent_fingerprint: intent.fingerprint().unwrap(),
        generation: intent.generation + 1,
        sandbox_id: "sbx_reserved".into(),
        workspace_id: "wks_reserved".into(),
    }
}

#[tokio::test]
async fn preparation_readers_preserve_legacy_wire_bytes_and_refuse_legacy_or_nonhuman_authority() {
    let mut setup = Setup::new(supported()).await;
    let body = frozen(
        &setup.plan,
        setup.target(),
        &setup.current.channel.lock().unwrap(),
        16,
    );
    let encoded = super::storage::bytes(&body).unwrap();
    assert!(!String::from_utf8_lossy(&encoded).contains("binding_generation"));
    assert!(!String::from_utf8_lossy(&body.provider_body()).contains("binding_generation"));
    let plan = serde_json::to_value(&setup.plan).unwrap();
    assert!(plan.get("binding_preparation_version").is_none());
    let decoded: FrozenTurn = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.provider_body(), body.provider_body());
    let checkpoint = BindingPreparation::Pending {
        intent: intent(&setup),
    };
    assert!(checkpoint.validate(&setup.plan, setup.target()).is_err());
    setup.plan.binding_preparation_version = Some(1);
    checkpoint.validate(&setup.plan, setup.target()).unwrap();
    for actor in [
        cmaeg_core::activity::ActorKind::Bot,
        cmaeg_core::activity::ActorKind::System,
        cmaeg_core::activity::ActorKind::Unknown,
    ] {
        setup.plan.activity.actor.kind = actor;
        assert!(checkpoint.validate(&setup.plan, setup.target()).is_err());
    }
    setup.plan.activity.actor.kind = cmaeg_core::activity::ActorKind::Human;
    setup.plan.targets[0].launch.provider.target =
        cmaeg_core::channel::ProviderTarget::ExistingAgent {
            agent_id: "agt_borrowed".into(),
        };
    assert!(checkpoint.validate(&setup.plan, setup.target()).is_err());
}

#[tokio::test]
async fn pending_preparation_survives_lost_ack_and_cannot_reinspect_new_generation() {
    for lose_read in [false, true] {
        let mut setup = Setup::new(supported()).await;
        setup.plan.binding_preparation_version = Some(1);
        let original = intent(&setup);
        let pending = BindingPreparation::Pending {
            intent: original.clone(),
        };
        let mut lease = setup
            .journal()
            .claim(&setup.plan, setup.target(), "first")
            .await
            .unwrap();
        setup
            .server
            .fault(setup.target().id.clone(), true, lose_read);
        let result = setup
            .journal()
            .pin_binding_preparation(&mut lease, &setup.plan, setup.target(), pending.clone())
            .await;
        if lose_read {
            assert_eq!(result, Err(ExecutionError::Uncertain));
        } else {
            result.unwrap();
        }
        setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
        let mut recovered = setup
            .journal()
            .claim(&setup.plan, setup.target(), "recovered")
            .await
            .unwrap();
        assert_eq!(recovered.state.binding_preparation, Some(pending.clone()));
        setup
            .journal()
            .pin_binding_preparation(&mut recovered, &setup.plan, setup.target(), pending)
            .await
            .unwrap();
        let mut changed = original.clone();
        changed.generation += 1;
        assert_eq!(
            setup
                .journal()
                .pin_binding_preparation(
                    &mut recovered,
                    &setup.plan,
                    setup.target(),
                    BindingPreparation::Pending { intent: changed }
                )
                .await,
            Err(ExecutionError::IdempotencyConflict)
        );
        let mut invalid = receipt(&original);
        invalid.generation += 1;
        assert!(setup
            .journal()
            .record_binding_preparation(&mut recovered, &setup.plan, setup.target(), invalid)
            .await
            .is_err());
        let committed = receipt(&original);
        setup
            .server
            .fault(setup.target().id.clone(), true, lose_read);
        let result = setup
            .journal()
            .record_binding_preparation(
                &mut recovered,
                &setup.plan,
                setup.target(),
                committed.clone(),
            )
            .await;
        if lose_read {
            assert_eq!(result, Err(ExecutionError::Uncertain));
            setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
            recovered = setup
                .journal()
                .claim(&setup.plan, setup.target(), "receipt-recovered")
                .await
                .unwrap();
        } else {
            result.unwrap();
        }
        setup
            .journal()
            .record_binding_preparation(
                &mut recovered,
                &setup.plan,
                setup.target(),
                committed.clone(),
            )
            .await
            .unwrap();
        let mut body = frozen(
            &setup.plan,
            setup.target(),
            &setup.current.channel.lock().unwrap(),
            16,
        );
        assert!(setup
            .journal()
            .freeze(&mut recovered, &setup.plan, setup.target(), &body)
            .await
            .is_err());
        body.binding_generation = Some(committed.generation);
        setup
            .journal()
            .freeze(&mut recovered, &setup.plan, setup.target(), &body)
            .await
            .unwrap();
        let retained = setup.journal().body(&recovered).await.unwrap();
        assert_eq!(retained.binding_generation, Some(committed.generation));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&retained.provider_body()).unwrap()
                ["binding_generation"],
            committed.generation
        );
        assert!(setup.port.starts.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn preparation_receipt_requires_pending_marker_and_cannot_mutate_prepared_turns() {
    let mut setup = Setup::new(supported()).await;
    setup.plan.binding_preparation_version = Some(1);
    let original = intent(&setup);
    let mut lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "first")
        .await
        .unwrap();
    assert_eq!(
        setup
            .journal()
            .record_binding_preparation(&mut lease, &setup.plan, setup.target(), receipt(&original))
            .await,
        Err(ExecutionError::IdempotencyConflict)
    );
    let pinned = BindingPreparation::Pinned {
        version: 1,
        binding_id: original.binding_id,
        generation: original.generation,
    };
    setup
        .journal()
        .pin_binding_preparation(&mut lease, &setup.plan, setup.target(), pinned.clone())
        .await
        .unwrap();
    let mut body = frozen(
        &setup.plan,
        setup.target(),
        &setup.current.channel.lock().unwrap(),
        16,
    );
    body.binding_generation = Some(original.generation);
    setup
        .journal()
        .freeze(&mut lease, &setup.plan, setup.target(), &body)
        .await
        .unwrap();
    assert_eq!(
        setup
            .journal()
            .pin_binding_preparation(&mut lease, &setup.plan, setup.target(), pinned)
            .await,
        Err(ExecutionError::Invalid)
    );
}

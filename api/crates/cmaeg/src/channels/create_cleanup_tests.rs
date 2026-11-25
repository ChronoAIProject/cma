//! A refused create leaves no channel; a refused release is typed and retained.
use super::{
    management::{self, ManagementError},
    mutation_fixture::{owner, wait, Fixture},
};
use crate::providers::ProviderError;
use axum::http::StatusCode;
use cmaeg_core::{channel::ChannelMutationKind, ids::RecordId};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn an_unlisted_provider_is_refused_before_any_channel_is_reserved() {
    for custody in [false, true] {
        let fixture = Fixture::new(false, custody).await;
        let state = fixture.state().await;
        let mut body = fixture.body();
        body["provider"]["slug"] = "unlisted".into();
        body["profile"]["metadata"]["provider"] = "unlisted".into();
        let result = management::create(
            &state,
            &owner("alice", false),
            serde_json::from_value(body).unwrap(),
        )
        .await;
        assert!(
            matches!(
                result,
                Err(ManagementError::Provider(ProviderError::UnknownProvider(_)))
            ),
            "custody={custody}: {result:?}"
        );
        assert!(fixture.peer.rows.lock().unwrap().is_empty(), "no record");
        assert!(fixture.peer.calls.lock().unwrap().is_empty(), "no send");
    }
}

#[tokio::test]
async fn a_refused_release_is_typed_and_retains_the_channel() {
    for custody in [false, true] {
        let fixture = Fixture::new(false, custody).await;
        let state = fixture.state().await;
        let creator = owner("alice", false);
        let created = management::create(
            &state,
            &creator,
            serde_json::from_value(fixture.body()).unwrap(),
        )
        .await
        .unwrap();
        *fixture.peer.refuse_delete.lock().unwrap() = Some((StatusCode::FORBIDDEN, "forbidden"));
        let refused =
            management::delete(&state, &creator, &created.channel_id, created.version).await;
        assert!(
            matches!(
                &refused,
                Err(ManagementError::ReleaseRefused { status: 403, code }) if code == "forbidden"
            ),
            "custody={custody}: {refused:?}"
        );
        assert!(management::load(&state, &creator, &created.channel_id)
            .await
            .is_ok());
        assert_eq!(fixture.peer.bound.lock().unwrap().len(), 1);
        if !custody {
            // Once the provider releases it, the same request completes.
            *fixture.peer.refuse_delete.lock().unwrap() = None;
            management::delete(&state, &creator, &created.channel_id, created.version)
                .await
                .unwrap();
            assert!(matches!(
                management::load(&state, &creator, &created.channel_id).await,
                Err(ManagementError::NotFound)
            ));
        }
    }
}

#[tokio::test]
async fn a_release_reported_absent_or_unavailable_keeps_its_existing_outcome() {
    let fixture = Fixture::new(false, false).await;
    let state = fixture.state().await;
    let creator = owner("alice", false);
    for (status, deleted) in [
        (StatusCode::SERVICE_UNAVAILABLE, false),
        (StatusCode::NOT_FOUND, true),
    ] {
        let created = management::create(
            &state,
            &creator,
            serde_json::from_value(fixture.body()).unwrap(),
        )
        .await
        .unwrap();
        *fixture.peer.refuse_delete.lock().unwrap() = Some((status, "binding_not_found"));
        let result =
            management::delete(&state, &creator, &created.channel_id, created.version).await;
        let retained = management::load(&state, &creator, &created.channel_id)
            .await
            .is_ok();
        if deleted {
            assert!(result.is_ok(), "{result:?}");
            assert!(!retained);
        } else {
            assert!(
                matches!(
                    result,
                    Err(ManagementError::Provider(ProviderError::Unavailable(_)))
                ),
                "{result:?}"
            );
            assert!(retained);
        }
    }
}

#[tokio::test]
async fn retirement_holds_an_incomplete_channel_across_instances_and_lost_ack() {
    for custody in [false, true] {
        let fixture = Fixture::new(false, custody).await;
        let state = fixture.state().await;
        let other = fixture.state().await;
        let creator = owner("alice", false);
        let body = fixture.body();
        let record_id = RecordId::new(body["record_id"].as_str().unwrap()).unwrap();
        *fixture.peer.refuse_bind.lock().unwrap() = Some(StatusCode::BAD_REQUEST);
        assert!(management::create(
            &state,
            &creator,
            serde_json::from_value(body.clone()).unwrap()
        )
        .await
        .is_err());
        let original = super::creation::load_record(&state, &creator, &record_id)
            .await
            .unwrap();
        let version = original.record.version;
        let id = &original.channel_id;
        let original_effect = original
            .definition
            .mutation
            .as_ref()
            .and_then(|mutation| mutation.effect.clone());
        assert!(matches!(
            management::retire_unready(&state, &owner("bob", false), id, version).await,
            Err(ManagementError::NotFound)
        ));
        assert!(matches!(
            management::retire_unready(&state, &creator, id, version + 1).await,
            Err(ManagementError::Conflict { .. })
        ));
        assert_eq!(
            management::load(&other, &creator, id)
                .await
                .unwrap()
                .record
                .version,
            original.record.version
        );

        fixture
            .peer
            .lose_ack
            .store(original.record.version, Ordering::SeqCst);
        management::retire_unready(&state, &creator, id, version)
            .await
            .unwrap();
        let retired = management::load(&other, &creator, id).await.unwrap();
        assert_eq!(
            management::unready_retirement_version(&retired.definition),
            Some(version)
        );
        assert!(!retired.definition.creation.as_ref().unwrap().ready);
        let retained_effect = retired
            .definition
            .mutation
            .as_ref()
            .unwrap()
            .effect
            .as_deref();
        assert_eq!(
            retained_effect,
            original_effect.as_deref().or(Some("unconfirmed_create"))
        );
        assert_eq!(retired.record.version, original.record.version + 1);
        management::retire_unready(&other, &creator, id, version)
            .await
            .unwrap();
        *fixture.peer.refuse_bind.lock().unwrap() = None;
        assert!(matches!(
            management::create(&other, &creator, serde_json::from_value(body).unwrap()).await,
            Err(ManagementError::Conflict { .. })
        ));
        assert_eq!(
            management::load(&state, &creator, id)
                .await
                .unwrap()
                .record
                .version,
            retired.record.version
        );
        assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
        assert!(fixture.peer.bound.lock().unwrap().is_empty());
        assert!(matches!(
            management::delete(&other, &creator, id, version).await,
            Err(ManagementError::Conflict { .. })
        ));
        assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
    }
}

#[tokio::test]
async fn retirement_refuses_a_ready_channel_without_touching_its_binding_or_record() {
    let fixture = Fixture::new(false, false).await;
    let state = fixture.state().await;
    let creator = owner("alice", false);
    let ready = management::create(
        &state,
        &creator,
        serde_json::from_value(fixture.body()).unwrap(),
    )
    .await
    .unwrap();
    let before = management::load(&state, &creator, &ready.channel_id)
        .await
        .unwrap();
    assert!(matches!(
        management::retire_unready(&state, &creator, &ready.channel_id, ready.version).await,
        Err(ManagementError::Invalid(_))
    ));
    let after = management::load(&state, &creator, &ready.channel_id)
        .await
        .unwrap();
    assert_eq!(after.record.version, ready.version);
    assert!(
        after.record.blob == before.record.blob,
        "ready record was rewritten"
    );
    assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
    assert_eq!(fixture.peer.bound.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn incomplete_channel_can_retire_after_its_provider_credential_is_lost() {
    let fixture = Fixture::new(false, false).await;
    let state = fixture.state().await;
    let creator = owner("alice", false);
    let body = fixture.body();
    let record_id = RecordId::new(body["record_id"].as_str().unwrap()).unwrap();
    *fixture.peer.refuse_bind.lock().unwrap() = Some(StatusCode::BAD_REQUEST);
    assert!(
        management::create(&state, &creator, serde_json::from_value(body).unwrap())
            .await
            .is_err()
    );
    let mut pending = super::creation::load_record(&state, &creator, &record_id)
        .await
        .unwrap();
    pending.definition.secrets.provider_credential = None;
    let blob = management::seal_definition(&state.master_keys, &pending.definition).unwrap();
    let version = state
        .records
        .put(&record_id, "alice", &blob, Some(pending.record.version))
        .await
        .unwrap();
    let id = &pending.channel_id;
    assert!(matches!(
        management::delete(&state, &creator, id, version).await,
        Err(ManagementError::Provider(ProviderError::Unavailable(_)))
    ));
    management::retire_unready(&state, &creator, id, version)
        .await
        .unwrap();
    let retired = management::load(&state, &creator, id).await.unwrap();
    assert_eq!(
        management::unready_retirement_version(&retired.definition),
        Some(version)
    );
    assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
}

#[tokio::test]
async fn retirement_preserves_an_incomplete_channel_with_a_maximum_length_prompt() {
    let fixture = Fixture::new(false, false).await;
    let state = fixture.state().await;
    let creator = owner("alice", false);
    let mut body = fixture.body();
    let record_id = RecordId::new(body["record_id"].as_str().unwrap()).unwrap();
    let prompt = "x".repeat(cmaeg_core::profile::MAX_SYSTEM_PROMPT_BYTES);
    body["profile"]["context"]["system_prompt"] = prompt.clone().into();
    *fixture.peer.refuse_bind.lock().unwrap() = Some(StatusCode::BAD_REQUEST);
    assert!(
        management::create(&state, &creator, serde_json::from_value(body).unwrap())
            .await
            .is_err()
    );
    let pending = super::creation::load_record(&state, &creator, &record_id)
        .await
        .unwrap();
    assert_eq!(pending.definition.profile.context.system_prompt, prompt);
    management::retire_unready(
        &state,
        &creator,
        &pending.channel_id,
        pending.record.version,
    )
    .await
    .unwrap();
    let retired = management::load(&state, &creator, &pending.channel_id)
        .await
        .unwrap();
    assert_eq!(retired.definition.profile.context.system_prompt, prompt);
    assert_eq!(
        management::unready_retirement_version(&retired.definition),
        Some(pending.record.version)
    );
    assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
}

#[tokio::test]
async fn bind_started_before_retirement_cannot_publish_readiness() {
    for (custody, mixed_custody) in [(false, false), (true, false), (false, true)] {
        let fixture = Fixture::new(false, custody).await;
        let caller = fixture.state().await;
        let other = fixture
            .state_with_custody(fixture.custody || mixed_custody)
            .await;
        let creator = owner("alice", false);
        let body = fixture.body();
        let record_id = RecordId::new(body["record_id"].as_str().unwrap()).unwrap();
        fixture.peer.pause.store(true, Ordering::SeqCst);
        let request = serde_json::from_value(body).unwrap();
        let create = tokio::spawn(async move {
            management::create(&caller, &owner("alice", false), request).await
        });
        wait(&fixture.peer.started).await;

        let mut pending = super::creation::load_record(&other, &creator, &record_id)
            .await
            .unwrap();
        if mixed_custody {
            let definition = &pending.definition;
            super::mutation::Reservation::acquire(
                &other.records,
                &other.master_keys,
                definition,
                pending.record.version,
                super::mutation::Intent {
                    operation: ChannelMutationKind::Create,
                    expected_version: pending.record.version,
                    fingerprint: definition.creation.as_ref().unwrap().fingerprint.clone(),
                    desired: definition,
                },
            )
            .await
            .unwrap();
            pending = super::creation::load_record(&other, &creator, &record_id)
                .await
                .unwrap();
            let tracked = pending.definition.mutation.as_ref().unwrap();
            assert_eq!(tracked.operation, ChannelMutationKind::Create);
            assert!(tracked.effect.is_none() && tracked.completed.is_empty());
        }
        management::retire_unready(
            &other,
            &creator,
            &pending.channel_id,
            pending.record.version,
        )
        .await
        .unwrap();
        *fixture.peer.refuse_delete.lock().unwrap() =
            Some((StatusCode::NOT_FOUND, "binding_not_found"));
        assert!(matches!(
            management::delete(
                &other,
                &creator,
                &pending.channel_id,
                pending.record.version
            )
            .await,
            Err(ManagementError::Conflict { .. })
        ));
        assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
        fixture.peer.resume.notify_one();
        assert!(create.await.unwrap().is_err());
        let retired = management::load(&other, &creator, &pending.channel_id)
            .await
            .unwrap();
        assert_eq!(
            management::unready_retirement_version(&retired.definition),
            Some(pending.record.version)
        );
        assert!(!retired.definition.creation.as_ref().unwrap().ready);
        assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
        assert_eq!(fixture.peer.bound.lock().unwrap().len(), 1);
    }
}

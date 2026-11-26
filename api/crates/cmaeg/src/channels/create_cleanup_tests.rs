//! A refused create leaves no channel; a refused release is typed and retained.
use super::{
    management::{self, ManagementError},
    mutation_fixture::{owner, wait, Fixture},
};
use crate::providers::ProviderError;
use axum::http::StatusCode;
use cmaeg_core::{
    channel::ChannelMutationKind,
    ids::RecordId,
    seal::{open_blob, open_retained_channel, seal_blob, MAX_BLOB_BYTES},
};
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
        let original_plaintext = open_blob(&fixture.keys, &original.record.blob)
            .unwrap()
            .payload;
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
        let (retired, retirement) = management::inspect(&other, &creator, id).await.unwrap();
        assert_eq!(retirement, Some(version));
        assert!(!retired.definition.creation.as_ref().unwrap().ready);
        assert_eq!(
            open_retained_channel(
                &fixture.keys,
                record_id.as_str(),
                retired.record.version,
                &retired.record.blob
            )
            .unwrap()
            .0
            .payload,
            original_plaintext
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
            management::inspect(&state, &creator, id)
                .await
                .unwrap()
                .0
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
    let (_, retirement) = management::inspect(&state, &creator, id).await.unwrap();
    assert_eq!(retirement, Some(version));
    assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
}

#[tokio::test]
async fn retirement_at_the_record_limit_fences_the_legacy_attachment_audit() {
    check_maximum_record_retirement(false).await;
}

#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo"]
async fn retirement_at_the_record_limit_preserves_tracked_worker_credentials() {
    check_maximum_record_retirement(true).await;
}

async fn check_maximum_record_retirement(native: bool) {
    use crate::worker_credentials::{tests::Memory, WorkerCredentials, WorkerHistory};
    use serde_json::json;
    use std::sync::Arc;

    let fixture = Fixture::new(native, false).await;
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
    let pending = super::creation::load_record(&state, &creator, &record_id)
        .await
        .unwrap();
    assert!(pending.definition.mutation.is_none());
    let mut value = pending.definition.to_blob_json();
    value["sources"] = json!([{
        "type": "custom",
        "auth": {"type": "hmac", "key_id": "route-key"},
        "attachments": {"origins": ["https://media.example"]}
    }]);
    value["secrets"]["hmac_secrets"] = json!([["route-key", "attachment-fixture"]]);
    value["partition"]["key"] = json!("per_event");
    value["profile"]["environment"] = json!({"padding": ""});
    let mut definition: cmaeg_core::channel::ChannelDefinition =
        serde_json::from_value(value).unwrap();
    definition.profile_digest = definition.profile.digest();
    // Pad a valid environment value and include legacy JSON whitespace.
    let spare = MAX_BLOB_BYTES
        - serde_json::to_vec(&definition.to_blob_json())
            .unwrap()
            .len();
    definition.profile.environment = Some(json!({"padding": "x".repeat(spare - 1)}));
    definition.profile_digest = definition.profile.digest();
    definition.validate().unwrap();
    let mut plaintext = serde_json::to_vec(&definition.to_blob_json()).unwrap();
    plaintext.push(b'\n');
    assert_eq!(plaintext.len(), MAX_BLOB_BYTES);
    let original = seal_blob(&fixture.keys, &plaintext).unwrap();
    let version = state
        .records
        .put(
            &record_id,
            &creator.subject,
            &original,
            Some(pending.record.version),
        )
        .await
        .unwrap();

    let history = Arc::new(Memory::default());
    let retiring = if native {
        fixture
            .state_with_tracking(WorkerCredentials::new(
                history.clone(),
                fixture.keys.clone(),
            ))
            .await
    } else {
        fixture.state().await
    };
    let id = &pending.channel_id;
    management::retire_unready(&retiring, &creator, id, version)
        .await
        .unwrap();
    let (retired, receipt) = management::inspect(&state, &creator, id).await.unwrap();
    assert_eq!(receipt, Some(version));
    assert_eq!(retired.record.version, version + 1);
    assert_eq!(retired.record.blob.len(), original.len());
    assert!(open_blob(&fixture.keys, &retired.record.blob).is_err());
    assert!(open_retained_channel(
        &fixture.keys,
        record_id.as_str(),
        version,
        &retired.record.blob
    )
    .is_err());
    assert!(open_retained_channel(
        &fixture.keys,
        "another-record",
        version + 1,
        &retired.record.blob
    )
    .is_err());
    assert_eq!(
        open_retained_channel(
            &fixture.keys,
            record_id.as_str(),
            version + 1,
            &retired.record.blob
        )
        .unwrap()
        .0
        .payload,
        plaintext
    );
    if native {
        let hash = super::definition::hash_secret("fixture-agent-key");
        assert!(history
            .was_worker(hash.strip_prefix("sha256:").unwrap_or(&hash))
            .await
            .unwrap());
    } else {
        // The rollback-era attachment audit uses the ordinary blob reader.
        let audit = super::audit::audit_custom_attachments(&state.records, &fixture.keys, true)
            .await
            .unwrap();
        assert_eq!((audit.scanned, audit.unreadable, audit.cleared), (1, 1, 0));
        assert!(!audit.complete());
    }
    management::retire_unready(&retiring, &creator, id, version)
        .await
        .unwrap();
    let (replayed, receipt) = management::inspect(&state, &creator, id).await.unwrap();
    assert_eq!(receipt, Some(version));
    assert_eq!(replayed.record, retired.record);
    assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
    fixture.cleanup().await;
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
        let (retired, receipt) = management::inspect(&other, &creator, &pending.channel_id)
            .await
            .unwrap();
        assert_eq!(receipt, Some(pending.record.version));
        assert!(!retired.definition.creation.as_ref().unwrap().ready);
        assert_eq!(fixture.peer.calls.lock().unwrap().as_slice(), ["PUT"]);
        assert_eq!(fixture.peer.bound.lock().unwrap().len(), 1);
    }
}

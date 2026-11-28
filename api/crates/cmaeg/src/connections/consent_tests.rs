//! Managed connection consent passes through the real owned HTTP record reader.
use super::*;
use crate::channels::{definition::build_update, write_gate::WriteFlags, ChannelUpdateRequest};

async fn managed(app: &Native) -> ChannelDefinition {
    let (mut current, _) = app.reserve_child().await;
    current.secrets.provider_credential =
        Some(cmaeg_core::channel::Secret::new("fixture-managed-provider"));
    current.secrets.agent_key_hash =
        crate::channels::definition::hash_secret("fixture-managed-provider");
    current
}

fn update(current: &ChannelDefinition) -> ChannelUpdateRequest {
    serde_json::from_value(json!({
        "schema_version": current.schema_version, "name": "Renamed managed channel",
        "profile": current.profile,
        "provider": {"slug": current.provider.slug, "target": current.provider.target},
        "sources": current.sources, "connection_ref": current.connection_ref,
        "partition": current.partition, "reply": current.reply, "expected_version": 1
    }))
    .unwrap()
}

fn flags() -> WriteFlags {
    WriteFlags {
        telegram_message_events: true,
        telegram_topic_lifecycle: true,
        telegram_topic_creation: true,
        dynamic_sources: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn managed_topic_creation_requires_separate_consent_and_survives_unrelated_saves_and_revocation(
) {
    let app = Native::new().await;
    let current = managed(&app).await;
    let mut request = update(&current);
    let crate::channels::SourceRequest::Telegram { topic_creation, .. } = &mut request.sources[0]
    else {
        panic!()
    };
    *topic_creation = true;
    expand(&app, &current, &mut request).await;
    assert!(build_update(&request, &current, WriteFlags::default()).is_err());
    let opted = build_update(&request, &current, flags()).unwrap();
    let mut request = update(&opted);
    request.sources.clear();
    expand(&app, &opted, &mut request).await;
    assert_eq!(
        build_update(&request, &opted, WriteFlags::default())
            .unwrap()
            .sources,
        opted.sources
    );
    let crate::channels::SourceRequest::Telegram { topic_creation, .. } = &mut request.sources[0]
    else {
        panic!()
    };
    *topic_creation = false;
    expand(&app, &opted, &mut request).await;
    let revoked = build_update(&request, &opted, WriteFlags::default()).unwrap();
    assert_eq!(revoked.sources, current.sources);
    assert!(build_update(&update(&opted), &revoked, WriteFlags::default()).is_err());
}

async fn expand(app: &Native, current: &ChannelDefinition, request: &mut ChannelUpdateRequest) {
    binding::expand_update(
        &app.service.management,
        &owner(&app.record.owner, false),
        current,
        request,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn managed_empty_source_preserves_saved_consent_and_optional_admission() {
    let app = Native::new().await;
    let mut current = managed(&app).await;
    let Source::Telegram {
        topic_lifecycle,
        message_events,
        channel_posts,
        ..
    } = &mut current.sources[0]
    else {
        panic!()
    };
    *topic_lifecycle = true;
    *message_events = true;
    *channel_posts = true;
    let before_calls = app.peer.calls.lock().unwrap().len();
    let before_version = app.stored().await.1;
    let mut request = update(&current);
    request.sources.clear();
    expand(&app, &current, &mut request).await;
    let next = build_update(&request, &current, WriteFlags::default()).unwrap();
    assert_eq!(next.sources, current.sources);
    assert_eq!(
        next.secrets.telegram_bot_token,
        current.secrets.telegram_bot_token
    );
    assert_eq!(
        next.secrets.telegram_webhook_secret,
        current.secrets.telegram_webhook_secret
    );
    assert_eq!(next.connection_ref, current.connection_ref);
    assert_eq!(next.name, "Renamed managed channel");
    request.sources.clear();
    request.connection_admission = Some(
        serde_json::from_value(json!({
            "type":"scoped", "senders":{"type":"open"},
            "chats":{"type":"allowlist","ids":["-10042"]}, "groups":"all"
        }))
        .unwrap(),
    );
    expand(&app, &current, &mut request).await;
    let next = build_update(&request, &current, flags()).unwrap();
    let Source::Telegram {
        admission,
        topic_lifecycle,
        message_events,
        channel_posts,
        ..
    } = &next.sources[0]
    else {
        panic!()
    };
    assert_eq!(Some(admission), request.connection_admission.as_ref());
    assert!(*topic_lifecycle && *message_events && *channel_posts);
    assert_eq!(app.peer.calls.lock().unwrap().len(), before_calls);
    assert_eq!(app.stored().await.1, before_version);
}

#[tokio::test]
async fn managed_explicit_consent_obeys_write_gates_and_allows_opt_out() {
    let app = Native::new().await;
    let current = managed(&app).await;
    let before_calls = app.peer.calls.lock().unwrap().len();
    let mut request = update(&current);
    let crate::channels::SourceRequest::Telegram {
        topic_lifecycle,
        message_events,
        channel_posts,
        ..
    } = &mut request.sources[0]
    else {
        panic!()
    };
    *topic_lifecycle = true;
    *message_events = true;
    *channel_posts = true;
    expand(&app, &current, &mut request).await;
    assert!(build_update(&request, &current, WriteFlags::default()).is_err());
    let opted = build_update(&request, &current, flags()).unwrap();
    assert_eq!(
        serde_json::to_value(&opted.sources[0]).unwrap()["channel_posts"],
        true
    );
    for events in [false, true] {
        let mut reduced = update(&opted);
        let crate::channels::SourceRequest::Telegram {
            message_events,
            channel_posts,
            ..
        } = &mut reduced.sources[0]
        else {
            panic!()
        };
        *message_events = events;
        *channel_posts = false;
        expand(&app, &opted, &mut reduced).await;
        let retained = build_update(&reduced, &opted, WriteFlags::default()).unwrap();
        assert!(matches!(
            retained.sources[0],
            Source::Telegram {
                topic_lifecycle: true,
                ..
            }
        ));
        assert!(build_update(&update(&opted), &retained, WriteFlags::default()).is_err());
        let crate::channels::SourceRequest::Telegram { admission, .. } = &mut reduced.sources[0]
        else {
            panic!()
        };
        *admission = cmaeg_core::channel::AdmissionPolicy::Allowlist {
            sender_ids: vec!["42".into()],
        };
        assert!(build_update(&reduced, &opted, WriteFlags::default()).is_err());
    }
    let mut request = update(&opted);
    let crate::channels::SourceRequest::Telegram {
        topic_lifecycle,
        message_events,
        channel_posts,
        ..
    } = &mut request.sources[0]
    else {
        panic!()
    };
    *topic_lifecycle = false;
    *message_events = false;
    *channel_posts = false;
    expand(&app, &opted, &mut request).await;
    let next = build_update(&request, &opted, WriteFlags::default()).unwrap();
    assert_eq!(next.sources, current.sources);
    let crate::channels::SourceRequest::Telegram { channel_posts, .. } = &mut request.sources[0]
    else {
        panic!()
    };
    *channel_posts = true;
    expand(&app, &opted, &mut request).await;
    assert!(build_update(&request, &opted, flags()).is_err());
    assert_eq!(app.peer.calls.lock().unwrap().len(), before_calls);
}

#[tokio::test]
async fn managed_consent_refuses_identity_credentials_extra_sources_and_wrong_owner() {
    let app = Native::new().await;
    let current = managed(&app).await;
    let original = serde_json::to_value(&current.sources[0]).unwrap();
    let mut other_bot = original.clone();
    other_bot["bot_id"] = "99".into();
    let mut other_name = original.clone();
    other_name["bot_username"] = "other_bot".into();
    let cases = [
        json!([other_bot]),
        json!([other_name]),
        json!([original.clone(), original]),
        json!([{"type":"custom","auth":{"type":"hmac","key_id":"fixture"}}]),
    ];
    let before_calls = app.peer.calls.lock().unwrap().len();
    for sources in cases {
        let mut request = update(&current);
        request.sources = serde_json::from_value(sources).unwrap();
        assert!(binding::expand_update(
            &app.service.management,
            &owner(&app.record.owner, false),
            &current,
            &mut request
        )
        .await
        .is_err());
    }
    for credential in [
        "telegram_bot_token",
        "telegram_webhook_secret",
        "discord_bot_token",
    ] {
        let mut request = update(&current);
        request.credentials =
            Some(serde_json::from_value(json!({credential:"fixture-override"})).unwrap());
        assert!(binding::expand_update(
            &app.service.management,
            &owner(&app.record.owner, false),
            &current,
            &mut request
        )
        .await
        .is_err());
    }
    let mut request = update(&current);
    assert!(binding::expand_update(
        &app.service.management,
        &owner("other-owner", false),
        &current,
        &mut request
    )
    .await
    .is_err());
    request.connection_ref = Some("another-connection".into());
    assert!(binding::expand_update(
        &app.service.management,
        &owner(&app.record.owner, false),
        &current,
        &mut request
    )
    .await
    .is_err());
    assert_eq!(app.peer.calls.lock().unwrap().len(), before_calls);
}

#[tokio::test]
async fn quarantined_channel_create_preserves_connection_bytes_and_version() {
    use crate::{channels::management, records::RecordError};

    let mut app = Native::new().await;
    app.begin(false).await;
    app.service
        .observe_creation(&app.channel, &app.receipt(42, now(), 107))
        .await
        .unwrap();
    let (connection, _) = app.stored().await;
    assert_eq!(connection.status, Status::Ready);
    assert!(connection.channel_record.is_none());

    let records = app.fixture.records().connections();
    let url = records.record_url(&connection.id).unwrap();
    let before = records.get(url.as_str()).await.unwrap();
    let mut template = app.channel.definition.clone();
    template.profile.metadata.owner.subject = connection.owner.clone();
    let request = app.replacement_request(&template);
    app.service.management.records = Arc::new(app.fixture.records().quarantine_channel_creates());

    let result = management::create(
        &app.service.management,
        &owner(&connection.owner, connection.organization),
        request,
    )
    .await;
    assert!(matches!(
        result,
        Err(management::ManagementError::Record(
            RecordError::Unavailable(_)
        ))
    ));
    let after = records.get(url.as_str()).await.unwrap();
    assert_eq!(after.version, before.version);
    assert!(after.blob == before.blob);
}

#[tokio::test]
async fn setup_gate_refuses_new_binding_but_preserves_bound_retry_and_update() {
    let mut app = Native::new().await;
    let current = managed(&app).await;
    app.service.management.managed_telegram_setup_enabled = false;
    let before_calls = app.peer.calls.lock().unwrap().len();
    let mut retry = app.replacement_request(&current);
    retry.record_id = Some(current.record_id.clone());
    binding::expand_create(
        &app.service.management,
        &owner(&app.record.owner, false),
        &mut retry,
    )
    .await
    .unwrap();
    expand(&app, &current, &mut update(&current)).await;
    let (mut connection, version) = app.stored().await;
    connection.channel_record = None;
    app.fixture
        .store()
        .save_connection(&connection, Some(version))
        .await
        .unwrap();
    let before = app.stored().await;
    let mut fresh = app.replacement_request(&current);
    assert!(binding::expand_create(
        &app.service.management,
        &owner(&app.record.owner, false),
        &mut fresh
    )
    .await
    .is_err());
    assert_eq!(app.stored().await.1, before.1);
    assert_eq!(app.peer.calls.lock().unwrap().len(), before_calls);
}

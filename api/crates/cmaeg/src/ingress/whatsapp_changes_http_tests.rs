use super::*;
use crate::whatsapp::message_changes_tests::{edited_text, revoke};
#[tokio::test]
async fn native_whatsapp_changes_signed_batch_is_visible_before_provider_work_and_survives_lost_ack(
) {
    let mut setup = Setup::at(time::OffsetDateTime::now_utc().unix_timestamp()).await;
    setup.changes();
    let activity = Arc::new(crate::activity::ActivityStore::new(
        setup.server.records.clone(),
        Arc::new(fixture::keys()),
    ));
    setup.state = setup.state.clone().with_activity_observer(Arc::new(
        crate::activity::ObservedActivity::new(
            activity.clone(),
            Arc::new(crate::turns::observer::NoopObserver),
        ),
    ));
    let app = setup.http();
    let change = current(edited_text(
        "wamid.edit",
        "wamid.original",
        "@other updated",
    ));
    let body = envelope(vec![
        change.clone(),
        current(text("wamid.original", "@default original")),
    ]);
    assert_eq!(
        call(app.clone(), setup.callback(body.clone(), false))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    // Store commits the change but its success response is lost; ingress verifies it by reading back.
    setup
        .server
        .fault(setup.identity("wamid.edit").admission_id(), true, false);
    let response = call(app.clone(), setup.callback(body.clone(), true)).await;
    assert_eq!(
        json_response(response).await,
        json!({"accepted":2,"refused":0})
    );
    // This fixture intentionally has not started the provider scheduler.
    let page = activity
        .latest(
            &setup.channel.definition.owner.subject,
            setup.channel.definition.record_id.as_str(),
        )
        .await
        .unwrap();
    let observed = page
        .items
        .iter()
        .find(|item| item.event_id == setup.identity("wamid.edit").event_id.as_str())
        .unwrap();
    assert_eq!(
        observed.event_type.as_deref(),
        Some("cma.message.updated.v1")
    );
    assert!(observed
        .consumers
        .iter()
        .all(|consumer| consumer.completed_at.is_none()));
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    let response = call(app, setup.callback(envelope(vec![change]), true)).await;
    assert_eq!(
        json_response(response).await,
        json!({"accepted":1,"refused":0})
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    let saved = setup
        .server
        .store()
        .load(&setup.identity("wamid.edit"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.source_body, body);
}
#[tokio::test]
async fn native_whatsapp_changes_signed_callback_refuses_sender_groups_and_wrong_account_phone() {
    let mut setup = Setup::at(time::OffsetDateTime::now_utc().unix_timestamp()).await;
    setup.changes();
    let app = setup.http();
    admitted(
        setup
            .launch(setup.verified(current(text("wamid.original", "hello")), vec![]))
            .await,
    );
    let mut wrong_sender = current(revoke("wamid.revoke", "wamid.original"));
    wrong_sender["from"] = "447700900999".into();
    assert_eq!(
        json_response(
            call(
                app.clone(),
                setup.callback(envelope(vec![wrong_sender]), true)
            )
            .await
        )
        .await,
        json!({"accepted":0,"refused":1})
    );
    for path in [
        "/entry/0/id",
        "/entry/0/changes/0/value/metadata/phone_number_id",
    ] {
        let mut body: Value = serde_json::from_slice(&envelope(vec![current(revoke(
            "wamid.revoke",
            "wamid.original",
        ))]))
        .unwrap();
        *body.pointer_mut(path).unwrap() = "99999".into();
        assert_eq!(
            json_response(
                call(
                    app.clone(),
                    setup.callback(serde_json::to_vec(&body).unwrap(), true)
                )
                .await
            )
            .await,
            json!({"accepted":0,"refused":0})
        );
    }
    let mut group = current(revoke("wamid.revoke", "wamid.original"));
    group["group_id"] = "group_ABC".into();
    assert_eq!(
        call(app.clone(), setup.callback(envelope(vec![group]), true))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let missing = current(revoke("wamid.missing", "wamid.never-incoming"));
    assert_eq!(
        call(app, setup.callback(envelope(vec![missing]), true))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

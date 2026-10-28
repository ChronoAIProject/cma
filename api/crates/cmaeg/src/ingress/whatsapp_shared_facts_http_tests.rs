use super::*;
use crate::whatsapp::shared_facts::tests::{contact, place};

#[tokio::test]
async fn native_whatsapp_shared_facts_http_verify_signature_account_phone_before_admission() {
    let mut setup = Setup::new().await;
    let app = setup.http();
    let contact = current(contact("wamid.http-contact"));
    let location = current(place("wamid.http-location"));
    let body = envelope(vec![
        contact.clone(),
        location.clone(),
        current(text("wamid.text", "hello")),
    ]);
    let response = call(app.clone(), setup.callback(body.clone(), false)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    for pointer in [
        "/entry/0/id",
        "/entry/0/changes/0/value/metadata/phone_number_id",
    ] {
        let mut wrong: Value = serde_json::from_slice(&body).unwrap();
        *wrong.pointer_mut(pointer).unwrap() = "999999".into();
        let response = call(
            app.clone(),
            setup.callback(serde_json::to_vec(&wrong).unwrap(), true),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_response(response).await,
            json!({"accepted":0,"refused":0})
        );
        assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    }
    let response = call(app.clone(), setup.callback(body.clone(), true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_response(response).await,
        json!({"accepted":3,"refused":0})
    );
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    let response = call(app.clone(), setup.callback(body, true)).await;
    assert_eq!(
        json_response(response).await,
        json!({"accepted":3,"refused":0})
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    let mut malformed = location;
    malformed["id"] = "wamid.invalid".into();
    malformed["location"]["latitude"] = 91.into();
    let body = envelope(vec![
        current(text("wamid.should-not-admit", "before malformed")),
        malformed,
    ]);
    let response = call(app, setup.callback(body, true)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
}

#[tokio::test]
async fn native_whatsapp_shared_contact_identity_never_satisfies_sender_admission() {
    let mut setup = Setup::new().await;
    let channel = Arc::make_mut(&mut setup.channel);
    let cmaeg_core::channel::Source::Whatsapp { admission, .. } =
        &mut channel.definition.sources[0]
    else {
        panic!()
    };
    *admission =
        serde_json::from_value(json!({"type":"allowlist","sender_ids":["999999"]})).unwrap();
    let app = setup.http();
    let body = envelope(vec![current(contact("wamid.not-the-sender"))]);
    let response = call(app, setup.callback(body, true)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_response(response).await,
        json!({"accepted":0,"refused":1})
    );
    assert!(setup
        .server
        .store()
        .load(&setup.identity("wamid.not-the-sender"))
        .await
        .unwrap()
        .is_none());
}

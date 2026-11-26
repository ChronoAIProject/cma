use super::*;
use cmaeg::{
    http::callbacks,
    ingress::{launch::UnavailableLauncher, IngressState},
};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

#[tokio::test]
async fn second_gateway_observes_the_pending_creation_before_provider_acknowledgement() {
    let pause = Arc::new(fake_provider::BindingPause {
        accepted: Notify::new(),
        release: Semaphore::new(0),
    });
    let provider = FakeProvider::start(FakeProviderConfig {
        binding_pause: Some(pause.clone()),
        ..Default::default()
    })
    .await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (first, stop_first, first_task) = app(&provider, &cma, &nyxid).await;
    let second_state = management_state(&provider, &cma, &nyxid, key_ring(1));
    let ingress = IngressState::new(
        second_state.resolver.clone(),
        second_state.master_keys.clone(),
        Default::default(),
        Arc::new(UnavailableLauncher),
    )
    .unwrap();
    let (second, stop_second, second_task) =
        serve_router(router_with_state(second_state).merge(callbacks::router(ingress))).await;

    let client = Client::new();
    let body = reserved_body();
    let record_id = body["record_id"].as_str().unwrap();
    let binding_id = format!("bnd_{}", record_id.replace('-', ""));
    let create = client
        .post(format!("{first}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&body);
    let pending_create = tokio::spawn(create.send());

    tokio::time::timeout(Duration::from_secs(5), pause.accepted.notified())
        .await
        .expect("provider must receive the bind");
    assert!(!pending_create.is_finished());
    let read_url = format!("{second}/v1/channel-records/{record_id}");
    let owner_read = client
        .get(&read_url)
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(owner_read.status(), 200);
    let pending: Value = owner_read.json().await.unwrap();
    assert_eq!(pending["record_id"], record_id);
    assert_eq!(pending["provider"]["binding_id"], binding_id);
    assert_eq!(pending["definition"]["creation"]["ready"], false);
    let channel = pending["channel_id"].as_str().unwrap();

    let foreign = client
        .get(&read_url)
        .bearer_auth("token-bob")
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), 404);
    let absent = client
        .get(format!(
            "{second}/v1/channel-records/{}",
            uuid::Uuid::new_v4()
        ))
        .bearer_auth("token-bob")
        .send()
        .await
        .unwrap();
    assert_eq!(absent.status(), 404);
    assert_eq!(
        foreign.json::<Value>().await.unwrap(),
        absent.json::<Value>().await.unwrap()
    );

    // Callback intake resolves the channel before it inspects the relay token.
    let callback = client
        .post(format!("{second}/v1/callbacks/{channel}"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(callback.status(), 409);
    let refusal: Value = callback.json().await.unwrap();
    assert_eq!(refusal["code"], "conflict");
    assert!(refusal["detail"]
        .as_str()
        .unwrap()
        .starts_with("channel_creation_pending:"));
    assert!(!pending_create.is_finished());

    pause.release.add_permits(1);
    let created = pending_create.await.unwrap().unwrap();
    assert_eq!(created.status(), 201);
    let created: Value = created.json().await.unwrap();
    let ready = client
        .get(&read_url)
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200);
    let ready: Value = ready.json().await.unwrap();
    for view in [&created, &ready] {
        assert_eq!(view["record_id"], record_id);
        assert_eq!(view["channel_id"], channel);
        assert_eq!(view["provider"]["binding_id"], binding_id);
        assert_eq!(view["definition"]["creation"]["ready"], true);
    }
    assert_eq!(cma.records().len(), 1);
    let mutations = provider.requests();
    assert_eq!(mutations.len(), 1);
    assert_eq!(mutations[0].method, "PUT");
    assert_eq!(
        mutations[0].path,
        format!("/api/v1/cmaeg/bindings/{binding_id}")
    );
    stop_first.send(()).unwrap();
    stop_second.send(()).unwrap();
    first_task.await.unwrap();
    second_task.await.unwrap();
}

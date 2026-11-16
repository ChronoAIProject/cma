use super::{
    definition::build_update,
    management,
    mutation_fixture::{owner, Fixture},
};
#[tokio::test]
async fn full_reserved_intent_must_fit_before_any_provider_send() {
    let fixture = Fixture::new(false, true).await;
    let state = fixture.state().await;
    let creator = owner("alice", false);
    let body = fixture.body();
    let created = management::create(
        &state,
        &creator,
        serde_json::from_value(body.clone()).unwrap(),
    )
    .await
    .unwrap();
    let mut request = fixture.update(&body, created.version, "Large");
    request.credentials.as_mut().unwrap().agent_key =
        Some(cmaeg_core::channel::Secret::new("x".repeat(30000)));
    let desired = build_update(&request, &created.definition, state.write_flags()).unwrap();
    assert!(
        management::seal_definition(&fixture.keys, &desired)
            .unwrap()
            .len()
            < 65536
    );
    assert!(
        management::update(&state, &creator, &created.channel_id, request)
            .await
            .is_err()
    );
    assert_eq!(fixture.peer.calls.lock().unwrap().len(), 1);
    assert!(management::load(&state, &creator, &created.channel_id)
        .await
        .unwrap()
        .definition
        .mutation
        .is_none());
    fixture.cleanup().await;
}

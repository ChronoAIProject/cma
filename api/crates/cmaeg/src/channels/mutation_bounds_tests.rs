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
    // Both fields are valid individually; the nested sealed intent exceeds
    // the record bound even though the intended channel alone still fits.
    request.credentials.as_mut().unwrap().agent_key =
        Some(cmaeg_core::channel::Secret::new("x".repeat(16000)));
    request.profile.context.system_prompt = "x".repeat(14000);
    let desired = build_update(&request, &created.definition, state.write_flags()).unwrap();
    assert!(
        management::seal_definition(&fixture.keys, &desired)
            .unwrap()
            .len()
            < 65536
    );
    let error = management::update(&state, &creator, &created.channel_id, request)
        .await
        .unwrap_err();
    assert!(matches!(error, management::ManagementError::Invalid(error)
        if error.message == "sealed channel exceeds the record size limit"));
    assert_eq!(fixture.peer.calls.lock().unwrap().len(), 1);
    let retained = management::load(&state, &creator, &created.channel_id)
        .await
        .unwrap();
    assert_eq!(retained.record.version, created.version);
    assert!(retained.definition.mutation.is_none());
    fixture.cleanup().await;
}

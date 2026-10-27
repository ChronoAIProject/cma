use super::*;
#[tokio::test]
async fn native_whatsapp_groups_require_owner_gate_and_approved_phone_before_any_write() {
    for case in 0..3 {
        let provider = FakeProvider::start(FakeProviderConfig::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let (mut state, _) = state(&provider, &cma, &nyxid, true);
        state.native_whatsapp_groups_writes_enabled = case != 0;
        let graph = Arc::new(Graph {
            valid: true,
            oba: case != 1,
            calls: AtomicUsize::new(0),
            methods: Mutex::default(),
        });
        state.whatsapp = Some(Arc::new(
            GraphClient::production()
                .unwrap()
                .with_transport(graph.clone()),
        ));
        let (base, stop, task) = serve_app(state).await;
        let mut input = body();
        input["sources"][0]["group_ids"] = json!(["group-fixture"]);
        let response = post(&base, &input).await;
        assert_eq!(
            response.status(),
            if case == 2 { 201 } else { 400 },
            "case {case}: {}",
            response.text().await.unwrap()
        );
        assert_eq!(
            graph.calls.load(Ordering::SeqCst),
            match case {
                0 => 0,
                1 => 3,
                _ => 5,
            }
        );
        assert!(graph
            .methods
            .lock()
            .unwrap()
            .iter()
            .all(|method| *method == Method::GET));
        if case != 2 {
            assert!(cma.records().is_empty());
            assert!(provider.requests().is_empty());
        }
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}
#[tokio::test]
async fn native_whatsapp_group_removal_with_authoring_off_needs_no_meta_availability() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let (mut enabled, _) = state(&provider, &cma, &nyxid, true);
    enabled.native_whatsapp_groups_writes_enabled = true;
    let (base, stop, task) = serve_app(enabled).await;
    let mut input = body();
    input["sources"][0]["group_ids"] = json!(["group-fixture"]);
    let response = post(&base, &input).await;
    assert_eq!(response.status(), 201);
    let created: Value = response.json().await.unwrap();
    stop.send(()).unwrap();
    task.await.unwrap();
    let (mut disabled, graph) = state(&provider, &cma, &nyxid, false);
    disabled.native_whatsapp_writes_enabled = false;
    let (base, stop, task) = serve_app(disabled).await;
    input.as_object_mut().unwrap().remove("record_id");
    input["credentials"]
        .as_object_mut()
        .unwrap()
        .remove("whatsapp");
    input["sources"][0]
        .as_object_mut()
        .unwrap()
        .remove("group_ids");
    input["expected_version"] = created["version"].clone();
    let response = Client::new()
        .put(format!(
            "{base}/v1/channels/{}",
            created["channel_id"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .json(&input)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    assert_eq!(graph.calls.load(Ordering::SeqCst), 0);
    stop.send(()).unwrap();
    task.await.unwrap();
}

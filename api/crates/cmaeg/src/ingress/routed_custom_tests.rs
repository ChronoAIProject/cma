//! Automatic selection is scoped to immutable owner consent and real Custom ingress.
use super::*;

fn custom_request(opted: bool) -> LaunchRequest {
    fixture::request(
        |definition| {
            definition["durable_custom_execution"] = opted.into();
            definition["routing"]["default_targets"] = serde_json::json!(["default"]);
        },
        fixture::custom(None),
    )
}

#[tokio::test]
async fn self_service_custom_selects_without_operator_record_allowlist_and_replays_once() {
    let request = custom_request(true);
    let mut setup = Setup::new(&request, false).await;
    assert!(setup.state.routed.as_ref().unwrap().records.is_empty());
    let first = admitted(setup.launch(request, b"immutable Custom body", true).await);
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .admission_enabled = false;
    let replay = admitted(
        setup
            .launch(custom_request(true), b"immutable Custom body", true)
            .await,
    );
    assert_eq!(first, replay);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
    let page = setup
        .server
        .store()
        .recover_all_page(None, 32)
        .await
        .unwrap();
    assert_eq!(page.plans.len(), 1);
    assert_eq!(page.plans[0].source_body, b"immutable Custom body");
}

#[tokio::test]
async fn marker_does_not_upgrade_other_sources_or_unmarked_routing_records() {
    let mut telegram = request();
    Arc::get_mut(&mut telegram.channel)
        .unwrap()
        .definition
        .durable_custom_execution = true;
    for request in [custom_request(false), telegram] {
        let setup = Setup::new(&request, false).await;
        setup
            .server
            .memory
            .unavailable_reads
            .store(1, Ordering::SeqCst);
        assert!(setup
            .state
            .capture_routed(&request, b"legacy body", true)
            .is_none());
        assert!(matches!(
            setup.launch(request, b"legacy body", true).await,
            LaunchOutcome::Accepted { .. }
        ));
        assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 1);
        assert_eq!(
            setup.server.memory.unavailable_reads.load(Ordering::SeqCst),
            1
        );
        assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn opted_custom_never_falls_back_when_store_replay_routing_or_admission_are_unavailable() {
    for condition in ["store", "replay", "routing", "paused"] {
        let request = custom_request(true);
        let mut setup = Setup::new(&request, false).await;
        match condition {
            "store" => setup.state.routed = None,
            "replay" => Arc::get_mut(setup.state.routed.as_mut().unwrap())
                .unwrap()
                .replay_providers
                .clear(),
            "routing" => {}
            "paused" => {
                Arc::get_mut(setup.state.routed.as_mut().unwrap())
                    .unwrap()
                    .admission_enabled = false
            }
            _ => unreachable!(),
        }
        let mut request = request;
        if condition == "routing" {
            Arc::get_mut(&mut request.channel)
                .unwrap()
                .definition
                .routing = None;
        }
        assert!(
            matches!(
                setup.launch(request, b"Custom body", true).await,
                LaunchOutcome::Refused(Problem { status: 503, .. })
            ),
            "{condition}"
        );
        assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0, "{condition}");
        assert_eq!(
            setup.server.memory.writes.load(Ordering::SeqCst),
            0,
            "{condition}"
        );
    }
}

#[tokio::test]
async fn immutable_custom_opt_in_keeps_unsigned_target_override_refusal() {
    let request = fixture::request(
        |value| value["durable_custom_execution"] = true.into(),
        fixture::custom(Some(fixture::REVIEWER)),
    );
    // The opt-in is serving policy, never authority over unsigned event headers.
    request.activity.validate().unwrap();
    let setup = Setup::new(&request, false).await;
    let outcome = setup.launch(request, b"binary data", false).await;
    assert!(matches!(outcome, LaunchOutcome::Refused(_)));
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn owner_custom_opt_in_can_select_replay_provider_outside_default_nonhuman_set() {
    let request = fixture::request(
        |value| {
            value["durable_custom_execution"] = true.into();
            value["provider"]["slug"] = "nyxbot".into();
            value["profile"]["metadata"]["provider"] = "nyxbot".into();
            value["profile_digest"] =
                crate::channels::routing_fixture::digest_of(&value["profile"]).into();
            value["routing"] = serde_json::json!({
                "version":1,"default_consumer":{"name":"default","non_human":true},
                "default_targets":["default"]
            });
            value["secrets"]["consumer_keys"] = serde_json::json!([]);
        },
        fixture::custom(None),
    );
    let setup = Setup::new(&request, false).await;
    assert!(!setup
        .state
        .routed
        .as_ref()
        .unwrap()
        .non_human_providers
        .contains("nyxbot"));
    admitted(
        setup
            .launch(request, b"owner opted Custom body", true)
            .await,
    );
    let plans = setup
        .server
        .store()
        .recover_all_page(None, 32)
        .await
        .unwrap()
        .plans;
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].targets.len(), 1);
    assert_eq!(plans[0].targets[0].launch.consumer.as_str(), "default");
    assert_eq!(plans[0].targets[0].launch.provider.slug.as_str(), "nyxbot");
    assert_eq!(
        plans[0].targets[0].launch.provider.binding_id.as_str(),
        "binding-1"
    );
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

fn existing_agent_media_request() -> LaunchRequest {
    let mut activity = serde_json::to_value(fixture::custom(None)).unwrap();
    activity["kind"]["ce"]["data"] = serde_json::json!({"attachments":[{
        "url":"data:text/plain;base64,Zml4dHVyZQ==","filename":"fixture.txt"
    }]});
    activity["kind"]["attachments"] = serde_json::json!([{
        "reference":"custom:fixture", "kind":"file", "filename":"fixture.txt",
        "mime_type":"text/plain", "source":{"type":"inline","pointer":"/attachments/0/url"}
    }]);
    fixture::request(
        |value| {
            value["durable_custom_execution"] = true.into();
            value["provider"]["target"] =
                serde_json::json!({"type":"existing_agent","agent_id":"agt_media_fixture"});
            value["sources"][2]["attachments"] =
                serde_json::json!({"pointer":"/attachments","origins":[]});
        },
        serde_json::from_value(activity).unwrap(),
    )
}

#[tokio::test]
async fn fresh_existing_agent_media_requires_continuation_but_accepted_replay_survives_writer_pause(
) {
    let request = existing_agent_media_request();
    let mut setup = Setup::new(&request, false).await;
    let result = setup.launch(request, b"media body", true).await;
    assert!(
        matches!(result, LaunchOutcome::Refused(Problem { status: 503, ref detail, .. })
        if detail.as_deref() == Some("custom_existing_agent_media_unavailable"))
    );
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), 0);
    Arc::get_mut(setup.state.routed.as_mut().unwrap())
        .unwrap()
        .custom_media_providers
        .insert("cma_codex".into());
    let first = admitted(
        setup
            .launch(existing_agent_media_request(), b"media body", true)
            .await,
    );
    let writes = setup.server.memory.writes.load(Ordering::SeqCst);
    let routing = Arc::get_mut(setup.state.routed.as_mut().unwrap()).unwrap();
    routing.custom_media_providers.clear();
    routing.admission_enabled = false;
    let replay = admitted(
        setup
            .launch(existing_agent_media_request(), b"media body", true)
            .await,
    );
    assert_eq!(first, replay);
    assert_eq!(setup.server.memory.writes.load(Ordering::SeqCst), writes);
    assert_eq!(setup.legacy.0.load(Ordering::SeqCst), 0);
}

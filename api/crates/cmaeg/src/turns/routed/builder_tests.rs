use super::{adapter_test_server::HttpServer, builder_test_media, test_support::*, *};
use crate::{
    ingress::routing_store::StoredTarget,
    turns::{
        preparation,
        render_media::{Audience, RenderMedia},
    },
    MediaToolProviders,
};
use cmaeg_core::{activity::ActivityKind, channel::PartitionKey, ids::ConsumerId};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

fn builder(http: &HttpServer) -> ProductionFrozenTurnBuilder {
    ProductionFrozenTurnBuilder::new(
        http.client(true, Duration::from_secs(1)),
        Arc::new(fixture::keys()),
    )
}
async fn prepare(
    setup: &Setup,
    builder: &ProductionFrozenTurnBuilder,
    target: &StoredTarget,
) -> Result<FrozenTurn, ExecutionError> {
    let current = setup.current.channel.lock().unwrap().clone();
    builder.prepare(&setup.plan, target, &current).await
}
fn input(body: &FrozenTurn) -> &str {
    body.input[0]["content"][0]["text"].as_str().unwrap()
}

#[tokio::test]
async fn default_and_named_targets_use_exact_admitted_conversation_binding_and_credential() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let builder = builder(&http);
    for target in &setup.plan.targets[..2] {
        let body = prepare(&setup, &builder, target).await.unwrap();
        assert_eq!(input(&body), "hello");
        assert_eq!(body.conversation_id, target.launch.event.conversation_id);
        assert_eq!(body.idempotency_key, target.launch.idempotency_key);
        assert_eq!(body.delivery.consumer, target.launch.consumer);
        let event = body
            .event_context
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload;
        assert_eq!(event.consumer, target.launch.event.consumer);
        assert_eq!(event.lineage, target.launch.event.lineage);
        assert_eq!(event.expires_at, target.launch.event.expires_at);
        let seen = http.seen();
        let request = seen.last().unwrap();
        assert_eq!(request.method, "PUT");
        assert!(request
            .uri
            .contains(target.launch.provider.binding_id.as_str()));
        assert!(request
            .uri
            .ends_with(target.launch.event.conversation_id.as_str()));
        assert_eq!(
            request.bearer,
            format!("Bearer nyx_fixture_{}", target.launch.consumer)
        );
        assert!(!String::from_utf8(body.provider_body())
            .unwrap()
            .contains("nyx_fixture_"));
    }
    assert_eq!(http.memory.lock().unwrap().conversations.len(), 2);
    assert!(http.seen().iter().all(|request| request.method == "PUT"));
}

#[tokio::test]
async fn existing_conversation_is_reused_and_frozen_targets_do_not_prepare_again() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let providers = http.client(true, Duration::from_secs(1));
    let builder = Arc::new(ProductionFrozenTurnBuilder::new(
        providers.clone(),
        Arc::new(fixture::keys()),
    ));
    prepare(&setup, &builder, setup.target()).await.unwrap();
    let port = Arc::new(HttpExecutionPort::new(
        providers,
        builder,
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
    ));
    let dispatcher = port.dispatcher();
    let DispatchOutcome::Replay {
        mut lease, body, ..
    } = dispatcher
        .dispatch(&setup.plan.identity, &setup.target().id, "builder-worker")
        .await
        .unwrap()
    else {
        panic!("accepted")
    };
    assert_eq!(http.memory.lock().unwrap().conversations.len(), 1);
    assert_eq!(
        http.seen()
            .iter()
            .filter(|request| request.method == "PUT")
            .count(),
        2
    );
    assert_eq!(
        http.seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        1
    );
    let bytes = body.provider_body();
    setup.journal().release(&mut lease).await.unwrap();
    let DispatchOutcome::Replay { body, .. } = dispatcher
        .dispatch(&setup.plan.identity, &setup.target().id, "builder-worker")
        .await
        .unwrap()
    else {
        panic!("replay")
    };
    assert_eq!(body.provider_body(), bytes);
    assert_eq!(http.seen().len(), 3);
}

#[tokio::test]
async fn default_address_is_pinned_before_provider_conversation_preparation() {
    let setup = Setup::with_request(
        supported(),
        fixture::request(
            |value| {
                fixture::targets(value);
                value["routing"]["consumers"][0]["reply"] =
                    serde_json::json!({"destination":"default"});
                value["routing"]["default_reply"] = serde_json::json!({
                    "type":"telegram_chat","source_id":"telegram",
                    "chat_id":"-200","thread_id":"7"
                });
            },
            fixture::message("hello"),
        ),
    )
    .await;
    let http = HttpServer::start().await;
    let builder = builder(&http);
    let body = prepare(&setup, &builder, setup.target()).await.unwrap();
    assert!(matches!(body.delivery.default_destination,
        Some(DefaultDestination::Telegram { chat_id, thread_id, .. })
        if chat_id == "-200" && thread_id.as_deref() == Some("7")));
    let mut current = (**setup.current.channel.lock().unwrap()).clone();
    current.definition.routing.as_mut().unwrap().default_reply = None;
    assert!(builder
        .prepare(&setup.plan, setup.target(), &current)
        .await
        .is_err());
    assert_eq!(http.seen().len(), 1);
    assert_eq!(http.seen()[0].method, "PUT");
}

#[tokio::test]
async fn rich_media_reuses_real_preparation_and_keeps_locators_out_of_input() {
    let mut activity = fixture::message("review the attached notes");
    if let ActivityKind::Message {
        attachments,
        reply_to,
        ..
    } = &mut activity.kind
    {
        attachments.push(builder_test_media::attachment());
        *reply_to = Some("prior-message".parse().unwrap());
    }
    let setup =
        Setup::with_request(supported(), fixture::request(fixture::targets, activity)).await;
    let http = HttpServer::start().await;
    let (media, source) = builder_test_media::service();
    let builder = builder(&http)
        .with_media(Some(media), crate::media::prepare::PrepareConfig::default())
        .with_media_tool_providers(Arc::new(MediaToolProviders::from_iter(["cma_codex"
            .parse()
            .unwrap()])));
    let body = prepare(&setup, &builder, setup.target()).await.unwrap();
    let text = input(&body);
    assert!(text.contains("[replying to an earlier message]"));
    assert!(text.contains("[Attachments: 1."));
    assert!(text.contains("notes.txt"));
    assert!(text.contains("shared media excerpt for the routed consumer"));
    assert!(text.contains("<media-"));
    assert!(text.contains("readEventAttachment"));
    assert!(!text.contains("private_file_locator"));
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(http.seen().len(), 1);
    assert_eq!(http.seen()[0].method, "PUT");
}

#[tokio::test]
async fn conversation_failure_or_cancellation_never_starts_a_response() {
    for status in [401, 403, 503] {
        let setup = Setup::new(supported()).await;
        let http = HttpServer::start().await;
        http.memory.lock().unwrap().config.conversation_status = status;
        let builder = builder(&http);
        assert!(prepare(&setup, &builder, setup.target()).await.is_err());
        assert_eq!(http.seen().len(), 1);
        assert_eq!(http.seen()[0].method, "PUT");
        assert!(http.memory.lock().unwrap().conversations.is_empty());
    }
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.conversation_delay = Duration::from_secs(1);
    let builder = builder(&http);
    assert!(tokio::time::timeout(
        Duration::from_millis(80),
        prepare(&setup, &builder, setup.target())
    )
    .await
    .is_err());
    assert_eq!(http.seen().len(), 1);
    assert_eq!(http.seen()[0].method, "PUT");
}

#[tokio::test]
async fn invalid_or_changed_authority_is_refused_before_conversation_creation() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let builder = builder(&http);
    let mut foreign = setup.target().clone();
    foreign.id = "foreign".into();
    assert!(matches!(
        prepare(&setup, &builder, &foreign).await,
        Err(ExecutionError::Invalid)
    ));
    let current = setup.current.channel.lock().unwrap().clone();
    for field in ["owner", "consumer", "provider", "source"] {
        let mut changed = (*current).clone();
        match field {
            "owner" => changed.definition.owner.subject = "other".into(),
            "consumer" => changed
                .definition
                .routing
                .as_mut()
                .unwrap()
                .consumers
                .clear(),
            "provider" => {
                changed.definition.routing.as_mut().unwrap().consumers[0]
                    .provider
                    .binding_id = "other-binding".parse().unwrap()
            }
            "source" => changed.definition.sources.clear(),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                builder.prepare(&setup.plan, setup.target(), &changed).await,
                Err(ExecutionError::AuthorityChanged)
            ),
            "{field}"
        );
    }
    assert!(http.seen().is_empty());
}

#[tokio::test]
async fn admitted_partition_and_reference_stay_pinned_when_channel_partition_changes() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let builder = builder(&http);
    let before = prepare(&setup, &builder, setup.target()).await.unwrap();
    let current = setup.current.channel.lock().unwrap().clone();
    let mut changed = (*current).clone();
    changed.definition.partition.key = PartitionKey::ConversationAndSender;
    let after = builder
        .prepare(&setup.plan, setup.target(), &changed)
        .await
        .unwrap();
    assert_eq!(
        before.metadata["cmaeg.partition"],
        after.metadata["cmaeg.partition"]
    );
    assert_eq!(before.conversation_id, after.conversation_id);
    assert_eq!(
        before
            .event_context
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload,
        after
            .event_context
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload
    );
}

#[tokio::test]
async fn routed_plain_input_matches_shared_live_renderer() {
    let request = fixture::request(fixture::targets, fixture::message("same prompt"));
    let original = preparation::render(
        &request,
        &RenderMedia::empty(None),
        Audience {
            tools: false,
            downloads: false,
        },
        None,
    );
    let setup = Setup::with_request(supported(), request).await;
    let http = HttpServer::start().await;
    let body = prepare(&setup, &builder(&http), &setup.plan.targets[1])
        .await
        .unwrap();
    assert_eq!(body.input, original.input);
    assert_eq!(
        body.metadata["cmaeg.actor"],
        original.metadata["cmaeg.actor"]
    );
    assert_eq!(body.delivery.consumer, ConsumerId::default_consumer());
}

#[tokio::test]
async fn slow_preparation_renews_lease_before_start_acceptance() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let advance = || {
        let clock = setup.server.memory.clock.clone();
        Arc::new(move || {
            clock.fetch_add(20, Ordering::SeqCst);
        }) as Arc<dyn Fn() + Send + Sync>
    };
    {
        let mut memory = http.memory.lock().unwrap();
        memory.config.on_conversation = Some(advance());
        memory.config.on_post = Some(advance());
    }
    let providers = http.client(true, Duration::from_secs(1));
    let builder = Arc::new(ProductionFrozenTurnBuilder::new(
        providers.clone(),
        Arc::new(fixture::keys()),
    ));
    let dispatcher = Arc::new(HttpExecutionPort::new(
        providers,
        builder,
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
    ))
    .dispatcher();
    let DispatchOutcome::Replay { lease, .. } = dispatcher
        .dispatch(&setup.plan.identity, &setup.target().id, "slow-preparation")
        .await
        .unwrap()
    else {
        panic!("accepted with renewed lease")
    };
    setup.journal().guard(&lease).await.unwrap();
    assert_eq!(http.seen().len(), 2);
}

#[tokio::test]
async fn existing_agent_target_keeps_excerpt_without_advertising_uninstalled_tools() {
    let mut activity = fixture::message("read my notes");
    if let ActivityKind::Message { attachments, .. } = &mut activity.kind {
        attachments.push(builder_test_media::attachment());
    }
    let request = fixture::request(
        |value| {
            fixture::targets(value);
            value["routing"]["consumers"][0]["provider"]["target"] =
                serde_json::json!({"type":"existing_agent","agent_id":"agt_existing"});
        },
        activity,
    );
    let setup = Setup::with_request(supported(), request).await;
    let http = HttpServer::start().await;
    let (media, source) = builder_test_media::service();
    let builder = builder(&http)
        .with_media(Some(media), crate::media::prepare::PrepareConfig::default())
        .with_media_tool_providers(Arc::new(MediaToolProviders::from_iter(["cma_codex"
            .parse()
            .unwrap()])));
    let body = prepare(&setup, &builder, setup.target()).await.unwrap();
    assert!(input(&body).contains("shared media excerpt for the routed consumer"));
    assert!(!input(&body).contains("readEventAttachment"));
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert!(http.seen().iter().all(|request| request.method == "PUT"));
}

#[tokio::test]
async fn expired_or_unconfigured_recovery_targets_have_no_preparation_effects() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    let current = setup.current.channel.lock().unwrap().clone();
    let mut plan = setup.plan.clone();
    plan.targets[0].launch.event.expires_at = 1;
    assert!(matches!(
        builder(&http)
            .prepare(&plan, &plan.targets[0], &current)
            .await,
        Err(ExecutionError::Expired)
    ));
    let builder = ProductionFrozenTurnBuilder::new(
        http.client(false, Duration::from_secs(1)),
        Arc::new(fixture::keys()),
    );
    assert!(matches!(
        prepare(&setup, &builder, setup.target()).await,
        Err(ExecutionError::Unsupported)
    ));
    assert!(http.seen().is_empty());
}

#[tokio::test]
async fn failed_preparation_remains_pending_until_an_idempotent_retry_succeeds() {
    let setup = Setup::new(supported()).await;
    let http = HttpServer::start().await;
    http.memory.lock().unwrap().config.conversation_status = 503;
    let providers = http.client(true, Duration::from_secs(1));
    let builder = Arc::new(ProductionFrozenTurnBuilder::new(
        providers.clone(),
        Arc::new(fixture::keys()),
    ));
    let dispatcher = Arc::new(HttpExecutionPort::new(
        providers,
        builder,
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
    ))
    .dispatcher();
    assert!(matches!(
        dispatcher
            .dispatch(&setup.plan.identity, &setup.target().id, "prepare-failure")
            .await,
        Err(ExecutionError::Unavailable)
    ));
    assert_eq!(http.seen().len(), 1);
    assert_eq!(http.seen()[0].method, "PUT");
    let mut lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "inspect")
        .await
        .unwrap();
    assert!(matches!(lease.state().phase, Phase::Pending));
    setup.journal().release(&mut lease).await.unwrap();
    http.memory.lock().unwrap().config.conversation_status = 201;
    assert!(matches!(
        dispatcher
            .dispatch(&setup.plan.identity, &setup.target().id, "prepare-retry")
            .await
            .unwrap(),
        DispatchOutcome::Replay { .. }
    ));
    assert_eq!(
        http.seen()
            .iter()
            .map(|seen| seen.method.as_str())
            .collect::<Vec<_>>(),
        ["PUT", "PUT", "POST"]
    );
}

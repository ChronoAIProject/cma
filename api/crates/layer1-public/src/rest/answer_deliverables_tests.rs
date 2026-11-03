use super::{agents_tests, shared_agent_tests};
use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::{
    agent_artifacts::AgentArtifactService, answer_deliverables::AnswerDeliverableService,
    store::AgentLogStore, AgentLogKind, AgentStatus,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt as _;

const ROOT: &str = "/api/v1/agents/agt_delivery";

#[tokio::test]
async fn answer_deliverables_report_bounded_turn_reads_as_unavailable() {
    let fixture = agents_tests::fixture().await;
    let mut kinds = vec![AgentLogKind::TurnStarted {
        model: None,
        turn_sequence: 3,
        origin_key: None,
    }];
    for _ in 0..514 {
        kinds.push(AgentLogKind::OwnerMessage {
            content: "Follow-up".into(),
        });
    }
    kinds.push(AgentLogKind::AgentMessage {
        content: "Final answer".into(),
    });
    kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
    agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent("agt_delivery", AgentStatus::Ready, 519, None),
        kinds,
    )
    .await;
    let access = Arc::new(ResourceAccessService::new(
        fixture.store.clone(),
        Arc::new(shared_agent_tests::Authority(Mutex::new(
            shared_agent_tests::Reply::Member,
        ))),
    ));
    let service = AnswerDeliverableService::new(
        fixture.reads(),
        AgentArtifactService::new(fixture.store.clone(), access),
    );
    let router = super::answer_deliverables_router(service);
    let (status, value) = agents_tests::json_response(
        &router,
        agents_tests::request(
            Method::GET,
            &format!("{ROOT}/answers/event:518/deliverables"),
            "owner-a",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{value}");
}

async fn read(router: &Router, path: &str) -> Value {
    let (status, value) = agents_tests::json_response(
        router,
        agents_tests::request(Method::GET, path, "owner-a", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value
}

#[tokio::test]
async fn answer_deliverables_keep_explicit_repeats_but_reject_examples_and_nonfinal_messages() {
    let fixture = agents_tests::fixture().await;
    let agent = agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent("agt_delivery", AgentStatus::Ready, 41, None),
        vec![],
    )
    .await;
    let access = Arc::new(ResourceAccessService::new(
        fixture.store.clone(),
        Arc::new(shared_agent_tests::Authority(Mutex::new(
            shared_agent_tests::Reply::Member,
        ))),
    ));
    let artifacts = AgentArtifactService::new(fixture.store.clone(), access);
    let service = AnswerDeliverableService::new(fixture.reads(), artifacts.clone());
    let router =
        super::answer_deliverables_router(service).merge(super::agent_artifacts_router(artifacts));
    let (status, published) = agents_tests::json_response(
        &router,
        agents_tests::request(
            Method::POST,
            &format!("{ROOT}/artifacts"),
            "owner-a",
            Some("explicit-result"),
            Some(json!({"name":"result.txt","media_type":"text/plain","content_base64":"YWJj"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, described) = agents_tests::json_response(
        &router,
        agents_tests::request(
            Method::POST,
            &format!("{ROOT}/deliverable-references"),
            "owner-a",
            None,
            Some(json!({"kind":"agent_artifact","id":published["id"]})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let marker = described["marker"].as_str().unwrap();
    let start = |turn_sequence| AgentLogKind::TurnStarted {
        model: None,
        turn_sequence,
        origin_key: None,
    };
    let owner = || AgentLogKind::OwnerMessage {
        content: "A question".into(),
    };
    let reply = |content: String| AgentLogKind::AgentMessage { content };
    for kind in [
        start(3),
        owner(),
        reply(marker.into()),
        reply(format!("Final\n{marker}\n{marker}")),
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
        start(8),
        owner(),
        reply(format!(
            "An ordinary `result.txt` and a quoted example:\n```html\n{marker}\n```"
        )),
        AgentLogKind::TurnCompleted { turn_sequence: 8 },
        start(12),
        owner(),
        reply(marker.into()),
        owner(),
        AgentLogKind::TurnCompleted { turn_sequence: 12 },
        start(17),
        owner(),
        reply(marker.into()),
        AgentLogKind::TurnInterrupted { turn_sequence: 17 },
        start(21),
        owner(),
        reply(marker.into()),
        start(24),
        owner(),
        reply(marker.into()),
        AgentLogKind::TurnCompleted { turn_sequence: 24 },
        start(28),
        owner(),
        AgentLogKind::ConversationGap { frames: 1 },
        reply(marker.into()),
        AgentLogKind::TurnCompleted { turn_sequence: 28 },
        start(33),
        owner(),
        reply(marker.into()),
        AgentLogKind::ConversationGap { frames: 1 },
        AgentLogKind::TurnCompleted { turn_sequence: 33 },
        start(38),
        owner(),
        reply(marker.into()),
        AgentLogKind::TurnCompleted { turn_sequence: 38 },
    ] {
        fixture
            .store
            .append_agent_log(&agent.id, "2026-09-05T00:00:00Z".into(), kind)
            .await
            .unwrap();
    }
    let first = read(&router, &format!("{ROOT}/answers/event:6/deliverables")).await;
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["turn_sequence"], 3);
    assert!(
        read(&router, &format!("{ROOT}/answers/event:10/deliverables")).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for sequence in [5, 14, 19, 23] {
        let (status, _) = agents_tests::json_response(
            &router,
            agents_tests::request(
                Method::GET,
                &format!("{ROOT}/answers/event:{sequence}/deliverables"),
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "nonfinal message {sequence}");
    }
    let repeated = read(&router, &format!("{ROOT}/answers/event:26/deliverables")).await;
    assert_eq!(repeated["turn_sequence"], 24);
    assert_eq!(repeated["items"], first["items"]);
    let delivery_id = first["items"][0]["id"].as_str().unwrap();
    for sequence in [31, 35] {
        for suffix in [
            String::new(),
            format!("/{delivery_id}"),
            format!("/{delivery_id}?verify=true"),
            format!("/{delivery_id}/download"),
        ] {
            let (status, _) = agents_tests::json_response(
                &router,
                agents_tests::request(
                    Method::GET,
                    &format!("{ROOT}/answers/event:{sequence}/deliverables{suffix}"),
                    "owner-a",
                    None,
                    None,
                ),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "gapped answer {sequence}{suffix}"
            );
        }
    }
    let intact = read(&router, &format!("{ROOT}/answers/event:40/deliverables")).await;
    assert_eq!(intact["turn_sequence"], 38);
    assert_eq!(intact["items"], first["items"]);
}

#[tokio::test]
async fn answer_deliverables_keep_two_published_results_separate_after_reconnect() {
    let fixture = agents_tests::fixture().await;
    let agent = agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent("agt_delivery", AgentStatus::Ready, 10, None),
        vec![],
    )
    .await;
    let access = Arc::new(ResourceAccessService::new(
        fixture.store.clone(),
        Arc::new(shared_agent_tests::Authority(Mutex::new(
            shared_agent_tests::Reply::Member,
        ))),
    ));
    let artifacts = AgentArtifactService::new(fixture.store.clone(), access);
    let build = || {
        super::answer_deliverables::answer_deliverables_router(AnswerDeliverableService::new(
            fixture.state.reads.clone(),
            artifacts.clone(),
        ))
        .merge(super::agent_artifacts_router(artifacts.clone()))
    };
    let router = build();
    let mut markers = Vec::new();
    let mut artifact_ids = Vec::new();
    for (name, content) in [("A.txt", "Answer A\n"), ("B.txt", "Answer B\n")] {
        let (status, published) = agents_tests::json_response(&router, agents_tests::request(
            Method::POST, &format!("{ROOT}/artifacts"), "owner-a", Some(name),
            Some(json!({"name": name, "media_type": "text/plain", "content_base64": STANDARD.encode(content)})),
        )).await;
        assert_eq!(status, StatusCode::CREATED, "{published}");
        let (status, reference) = agents_tests::json_response(
            &router,
            agents_tests::request(
                Method::POST,
                &format!("{ROOT}/deliverable-references"),
                "owner-a",
                None,
                Some(json!({"kind":"agent_artifact", "id": published["id"]})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{reference}");
        markers.push(reference["marker"].as_str().unwrap().to_owned());
        artifact_ids.push(published["id"].clone());
    }
    for (turn_sequence, marker) in [(3, &markers[0]), (7, &markers[1])] {
        for kind in [
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Deliver the requested file".into(),
            },
            AgentLogKind::AgentMessage {
                content: format!("Delivered.\n{marker}"),
            },
            AgentLogKind::TurnCompleted { turn_sequence },
        ] {
            fixture
                .store
                .append_agent_log(&agent.id, "2026-11-03T00:00:00Z".into(), kind)
                .await
                .unwrap();
        }
    }
    let first_path = format!("{ROOT}/answers/event:5/deliverables");
    let first = read(&router, &first_path).await;
    assert_eq!(first["agent_id"], "agt_delivery");
    assert_eq!(first["answer_id"], "event:5");
    assert_eq!(first["turn_sequence"], 3);
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        first["items"][0]["reference"]["resource"]["id"],
        artifact_ids[0]
    );
    assert_eq!(first["items"][0]["status"], "ready");
    let single_path = format!("{first_path}/{}", first["items"][0]["id"].as_str().unwrap());
    let single = read(&router, &single_path).await;
    assert_eq!(single["answer_id"], "event:5");
    assert_eq!(single["turn_sequence"], 3);
    assert_eq!(single["item"], first["items"][0]);
    assert_eq!(
        read(&router, &format!("{single_path}?verify=true")).await,
        single
    );
    let second = read(&router, &format!("{ROOT}/answers/event:9/deliverables")).await;
    assert_eq!(
        second["items"][0]["reference"]["resource"]["id"],
        artifact_ids[1]
    );
    let reconnected = build();
    assert_eq!(read(&reconnected, &first_path).await, first);
    let response = reconnected
        .oneshot(agents_tests::request(
            Method::GET,
            &format!(
                "{first_path}/{}/download",
                first["items"][0]["id"].as_str().unwrap()
            ),
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "Answer A\n"
    );
}

#[tokio::test]
async fn answer_deliverables_download_the_original_document_revision_after_an_edit() {
    use layer2_core::document_artifacts::{
        memory::{MemoryDocumentBlobs, MemoryDocuments},
        DocumentService,
    };
    let fixture = agents_tests::fixture().await;
    let agent = agents_tests::insert_agent(
        &fixture.store,
        agents_tests::seeded_agent("agt_delivery", AgentStatus::Ready, 6, None),
        vec![],
    )
    .await;
    let access = Arc::new(ResourceAccessService::new(
        fixture.store.clone(),
        Arc::new(shared_agent_tests::Authority(Mutex::new(
            shared_agent_tests::Reply::Member,
        ))),
    ));
    let documents = DocumentService::new(
        Arc::new(MemoryDocuments::default()),
        Arc::new(MemoryDocumentBlobs::default()),
    );
    let service = AnswerDeliverableService::new(
        fixture.state.reads.clone(),
        AgentArtifactService::new(fixture.store.clone(), access),
    )
    .with_documents(documents.clone());
    let router =
        super::answer_deliverables_router(service).merge(super::documents_router(documents));
    let (status, document) = agents_tests::json_response(
        &router,
        agents_tests::request(
            Method::POST,
            "/api/v1/documents",
            "owner-a",
            Some("first-document"),
            Some(json!({"title":"Report", "markdown":"# Original\n"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{document}");
    let (status, description) = agents_tests::json_response(
        &router,
        agents_tests::request(
            Method::POST,
            &format!("{ROOT}/deliverable-references"),
            "owner-a",
            None,
            Some(json!({"kind":"document", "id": document["id"], "revision": 1})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{description}");
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Write a report".into(),
        },
        AgentLogKind::AgentMessage {
            content: format!("Report saved.\n{}", description["marker"].as_str().unwrap()),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ] {
        fixture
            .store
            .append_agent_log(&agent.id, "2026-11-03T00:00:00Z".into(), kind)
            .await
            .unwrap();
    }
    let path = format!("{ROOT}/answers/event:5/deliverables");
    let original = read(&router, &path).await;
    assert_eq!(original["items"][0]["reference"]["size_bytes"], 11);
    let (status, edited) = agents_tests::json_response(
        &router,
        agents_tests::request(
            Method::PATCH,
            &format!("/api/v1/documents/{}", document["id"].as_str().unwrap()),
            "owner-a",
            Some("edit-document"),
            Some(json!({"expected_revision":1, "markdown":"# Edited\n"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let changed = read(&router, &path).await;
    assert_eq!(changed["items"][0]["status"], "version_changed");
    assert_eq!(
        changed["items"][0]["reference"],
        original["items"][0]["reference"]
    );
    let response = router
        .oneshot(agents_tests::request(
            Method::GET,
            &format!(
                "{path}/{}/download",
                changed["items"][0]["id"].as_str().unwrap()
            ),
            "owner-a",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "# Original\n"
    );
}

#[tokio::test]
async fn answer_deliverables_require_independent_document_access_and_honor_revocation() {
    use layer2_core::document_artifacts::{
        memory::{MemoryDocumentBlobs, MemoryDocuments},
        DocumentService,
    };
    use layer2_core::sandbox::store::AgentStore;
    let (fixture, access, _, _) =
        shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let agent_id = layer2_core::AgentId("agt_shared_read".into());
    let documents = DocumentService::new(
        Arc::new(MemoryDocuments::default()),
        Arc::new(MemoryDocumentBlobs::default()),
    );
    let router = super::answer_deliverables_router(
        AnswerDeliverableService::new(
            fixture.state.reads.clone(),
            AgentArtifactService::new(fixture.store.clone(), access),
        )
        .with_documents(documents.clone()),
    )
    .merge(super::documents_router(documents));
    let make = |method, path: &str, subject: &str, key: Option<&str>, body| {
        let mut request = agents_tests::request(method, path, subject, key, body);
        request
            .extensions_mut()
            .insert(super::resource_access_tests::context(subject));
        request
            .extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
        request
    };
    let (_, document) = agents_tests::json_response(
        &router,
        make(
            Method::POST,
            "/api/v1/documents",
            "owner-a",
            Some("private"),
            Some(json!({"title":"Private report", "markdown":"Private content"})),
        ),
    )
    .await;
    let (status, description) = agents_tests::json_response(
        &router,
        make(
            Method::POST,
            "/api/v1/agents/agt_shared_read/deliverable-references",
            "owner-a",
            None,
            Some(json!({"kind":"document", "id":document["id"], "revision":1})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{description}");
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 10,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Deliver a private report".into(),
        },
        AgentLogKind::AgentMessage {
            content: description["marker"].as_str().unwrap().into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 10 },
    ] {
        fixture
            .store
            .append_agent_log(&agent_id, "2026-11-03T00:00:00Z".into(), kind)
            .await
            .unwrap();
    }
    let mut agent = fixture.store.get_agent(&agent_id).await.unwrap().unwrap();
    agent.conversation.last_seq = 13;
    let revision = agent.revision;
    agent.revision += 1;
    fixture.store.save_agent(revision, agent).await.unwrap();
    let path = "/api/v1/agents/agt_shared_read/answers/event:12/deliverables";
    let (status, denied) =
        agents_tests::json_response(&router, make(Method::GET, path, "recipient", None, None))
            .await;
    assert_eq!(status, StatusCode::OK, "{denied}");
    assert_eq!(denied["items"][0]["status"], "denied");
    let download = format!(
        "{path}/{}/download",
        denied["items"][0]["id"].as_str().unwrap()
    );
    assert_eq!(
        agents_tests::response(
            &router,
            make(Method::GET, &download, "recipient", None, None)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let share_path = format!(
        "/api/v1/documents/{}/shares/recipient",
        document["id"].as_str().unwrap()
    );
    let (status, granted) = agents_tests::json_response(
        &router,
        make(
            Method::PUT,
            &share_path,
            "owner-a",
            Some("grant"),
            Some(json!({"expected_revision":1,"permission":"read"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{granted}");
    let (_, ready) =
        agents_tests::json_response(&router, make(Method::GET, path, "recipient", None, None))
            .await;
    assert_eq!(ready["items"][0]["status"], "ready");
    assert_eq!(
        agents_tests::response(
            &router,
            make(Method::GET, &download, "recipient", None, None)
        )
        .await
        .1,
        "Private content"
    );
    let (status, revoked) = agents_tests::json_response(
        &router,
        make(
            Method::DELETE,
            &share_path,
            "owner-a",
            Some("revoke"),
            Some(json!({"expected_revision":granted["revision"]})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    let (_, denied_again) =
        agents_tests::json_response(&router, make(Method::GET, path, "recipient", None, None))
            .await;
    assert_eq!(denied_again["items"][0]["status"], "denied");
    assert_eq!(
        agents_tests::response(
            &router,
            make(Method::GET, &download, "recipient", None, None)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        agents_tests::response(
            &router,
            agents_tests::request(Method::GET, path, "owner-b", None, None)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

use super::super::{answer_deliverables_router, documents_router, resource_access_tests};
use super::{
    agents_tests, shared_agent_tests, AgentArtifactService, AgentLogKind, AgentLogStore,
    AnswerDeliverableService,
};
use axum::http::{Method, StatusCode};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn answer_deliverables_require_independent_document_access_and_honor_revocation() {
    use layer2_core::document_artifacts::{
        memory::{recipient_directory, MemoryDocumentBlobs, MemoryDocuments},
        DocumentService,
    };
    use layer2_core::sandbox::store::AgentStore;
    let (fixture, access, _, _) =
        shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let agent_id = layer2_core::AgentId("agt_shared_read".into());
    let documents = DocumentService::new(
        Arc::new(MemoryDocuments::default()),
        Arc::new(MemoryDocumentBlobs::default()),
    )
    .with_directory(recipient_directory(&["recipient"]));
    let router = answer_deliverables_router(
        AnswerDeliverableService::new(
            fixture.state.reads.clone(),
            AgentArtifactService::new(fixture.store.clone(), access),
        )
        .with_documents(documents.clone()),
    )
    .merge(documents_router(documents));
    let make = |method, path: &str, subject: &str, key: Option<&str>, body| {
        let mut request = agents_tests::request(method, path, subject, key, body);
        request
            .extensions_mut()
            .insert(resource_access_tests::context(subject));
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

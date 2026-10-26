use std::collections::BTreeMap;

use layer2_core::sandbox::message_feedback::{MessageFeedback, MessageFeedbackStore, MessageVote};
use layer2_core::sandbox::AgentReadError;

use super::*;

type FeedbackKey = (SubjectId, AgentId, u64);
#[derive(Default)]
pub(in crate::rest) struct FeedbackMemory(Mutex<BTreeMap<FeedbackKey, MessageVote>>);
#[async_trait]
impl MessageFeedbackStore for FeedbackMemory {
    async fn list(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<MessageFeedback>, AgentReadError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|((o, a, seq), _)| o == owner && a == agent && *seq > after && *seq <= through)
            .take(usize::from(limit))
            .map(|((_, _, seq), vote)| MessageFeedback {
                sequence: *seq,
                vote: *vote,
            })
            .collect())
    }
    async fn set(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
        vote: Option<MessageVote>,
    ) -> Result<(), AgentReadError> {
        let mut rows = self.0.lock().unwrap();
        let key = (owner.clone(), agent.clone(), sequence);
        if let Some(vote) = vote {
            rows.insert(key, vote);
        } else {
            rows.remove(&key);
        }
        Ok(())
    }
}

pub(in crate::rest) async fn feedback_fixture() -> (Fixture, Arc<FeedbackMemory>, Agent) {
    let mut fixture = fixture().await;
    let feedback = Arc::new(FeedbackMemory::default());
    fixture.state = fixture.state.with_message_feedback(feedback.clone());
    fixture.router = agent_router(fixture.state.clone());
    let agent = insert_agent(
        &fixture.store,
        seeded_agent("agt_feedback", AgentStatus::Working, 5, Some(3)),
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Question".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Answer".into(),
            },
        ],
    )
    .await;
    (fixture, feedback, agent)
}

const LIST: &str = "/api/v1/agents/agt_feedback/message-feedback";
const OWNER_MESSAGE: &str = "/api/v1/agents/agt_feedback/messages/event:4/feedback";
const AGENT_MESSAGE: &str = "/api/v1/agents/agt_feedback/messages/event:5/feedback";

#[tokio::test]
async fn message_feedback_is_persisted_metadata_that_never_creates_turns_or_runtime_input() {
    let (fixture, feedback, agent) = feedback_fixture().await;
    let before = fixture.store.agent_log(&agent.id, 0, 100).await.unwrap();
    for (path, vote) in [
        (OWNER_MESSAGE, "up"),
        (AGENT_MESSAGE, "down"),
        (AGENT_MESSAGE, "down"),
    ] {
        let (status, body) = json_response(
            &fixture.router,
            request(Method::PUT, path, OWNER, None, Some(json!({"vote": vote}))),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["vote"], vote);
    }
    // Reconstruct the interface/service from the same durable store: no per-tab or process state.
    let reconnected = agent_router(fixture.state.clone().with_message_feedback(feedback));
    let (status, page) = json_response(
        &reconnected,
        request(Method::GET, &format!("{LIST}?limit=1"), OWNER, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        page,
        json!({"items":[{"message_id":"event:4","vote":"up"}],"next":4})
    );
    let (_, page) = json_response(
        &reconnected,
        request(
            Method::GET,
            &format!("{LIST}?after=4&limit=1"),
            OWNER,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(
        page,
        json!({"items":[{"message_id":"event:5","vote":"down"}],"next":null})
    );
    for vote in [json!("up"), Value::Null, Value::Null] {
        let (status, body) = json_response(
            &reconnected,
            request(
                Method::PUT,
                AGENT_MESSAGE,
                OWNER,
                None,
                Some(json!({"vote":vote})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body, json!({"message_id":"event:5","vote":vote}));
    }
    let (_, page) =
        json_response(&reconnected, request(Method::GET, LIST, OWNER, None, None)).await;
    assert_eq!(
        page,
        json!({"items":[{"message_id":"event:4","vote":"up"}],"next":null})
    );
    assert_eq!(
        fixture.store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(
        fixture.store.agent_log(&agent.id, 0, 100).await.unwrap(),
        before
    );
    // TestRegistry.resolve always refuses; all successful feedback writes therefore
    // also prove this feature does not resolve or send work to the runtime.
}

#[tokio::test]
async fn message_feedback_rejects_foreign_unauthenticated_ungranted_and_nonmessage_targets() {
    let (fixture, feedback, _) = feedback_fixture().await;
    let body = Some(json!({"vote":"up"}));
    for (method, path, payload) in [
        (Method::GET, LIST, None),
        (Method::PUT, AGENT_MESSAGE, body.clone()),
    ] {
        let (status, _) = json_response(
            &fixture.router,
            request(method.clone(), path, OTHER, None, payload.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let mut ungranted = request(method.clone(), path, OWNER, None, payload.clone());
        ungranted.extensions_mut().insert(Authorization::new([]));
        assert_eq!(
            response(&fixture.router, ungranted).await.0,
            StatusCode::FORBIDDEN
        );
        let mut anonymous = request(method, path, OWNER, None, payload);
        anonymous.extensions_mut().remove::<AuthenticatedSubject>();
        assert_eq!(
            response(&fixture.router, anonymous).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    for message in ["event:1", "event:2", "event:3", "event:6", "event:999"] {
        let (status, _) = json_response(
            &fixture.router,
            request(
                Method::PUT,
                &format!("/api/v1/agents/agt_feedback/messages/{message}/feedback"),
                OWNER,
                None,
                body.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{message}");
    }
    for message in [
        "event:0",
        "event:05",
        "event:-1",
        "optimistic:5",
        "event:18446744073709551616",
    ] {
        let (status, _) = json_response(
            &fixture.router,
            request(
                Method::PUT,
                &format!("/api/v1/agents/agt_feedback/messages/{message}/feedback"),
                OWNER,
                None,
                body.clone(),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{message}");
    }
    for body in [
        json!({}),
        json!({"vote":"maybe"}),
        json!({"vote":true}),
        json!({"vote":null,"user_id":OTHER}),
    ] {
        let (status, problem) = json_response(
            &fixture.router,
            request(Method::PUT, AGENT_MESSAGE, OWNER, None, Some(body)),
        )
        .await;
        assert!(matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ));
        assert_eq!(problem["code"], "invalid_json");
    }
    assert!(feedback.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn message_feedback_page_is_bounded_and_hidden_for_missing_agents() {
    let (fixture, _, _) = feedback_fixture().await;
    for suffix in ["?limit=0", "?limit=201", "?after=-1", "?owner=other"] {
        let (status, _) = json_response(
            &fixture.router,
            request(Method::GET, &format!("{LIST}{suffix}"), OWNER, None, None),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _) = json_response(
        &fixture.router,
        request(
            Method::GET,
            "/api/v1/agents/agt_missing/message-feedback",
            OWNER,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn message_feedback_preserves_real_share_policy_and_revoked_owner_capability() {
    use crate::rest::{resource_access_tests::context, shared_agent_tests};
    let (fixture, _, _, _) = shared_agent_tests::fixture(shared_agent_tests::Reply::Member).await;
    let feedback = Arc::new(FeedbackMemory::default());
    let router = agent_router(fixture.state.with_message_feedback(feedback.clone()));
    let make = |method: Method, path: &str, who: &str, body: Option<Value>| {
        let mut incoming = request(method, path, who, None, body);
        incoming.extensions_mut().insert(context(who));
        incoming
            .extensions_mut()
            .insert(crate::DelegatedCredential::new("request-credential".into()));
        incoming
    };
    let base = "/api/v1/agents/agt_shared_read";
    assert_eq!(
        response(
            &router,
            make(
                Method::GET,
                &format!("{base}/transcript"),
                "recipient",
                None
            )
        )
        .await
        .0,
        StatusCode::OK
    );
    for (method, suffix, body) in [
        (Method::GET, "/message-feedback", None),
        (
            Method::PUT,
            "/messages/event:6/feedback",
            Some(json!({"vote":"up"})),
        ),
    ] {
        assert_eq!(
            response(
                &router,
                make(method, &format!("{base}{suffix}"), "recipient", body)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let path = format!("{base}/messages/event:6/feedback");
    assert_eq!(
        response(
            &router,
            make(Method::PUT, &path, OWNER, Some(json!({"vote":"up"})))
        )
        .await
        .0,
        StatusCode::OK
    );
    let before = feedback.0.lock().unwrap().clone();
    let mut revoked = request(
        Method::PUT,
        &path,
        OWNER,
        None,
        Some(json!({"vote":"down"})),
    );
    revoked.extensions_mut().insert(Authorization::none());
    assert_eq!(response(&router, revoked).await.0, StatusCode::FORBIDDEN);
    assert_eq!(*feedback.0.lock().unwrap(), before);
}

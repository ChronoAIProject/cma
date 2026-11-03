use std::collections::BTreeMap;

use layer2_core::sandbox::message_feedback::{
    FeedbackDetails, FeedbackReasonError, FeedbackReasonRequest, MessageFeedback,
    MessageFeedbackStore, MessageVote,
};
use layer2_core::sandbox::AgentReadError;

use super::*;

type FeedbackKey = (SubjectId, AgentId, u64);
#[derive(Default)]
pub(in crate::rest) struct FeedbackMemory(Mutex<BTreeMap<FeedbackKey, FeedbackDetails>>);
#[async_trait]
impl MessageFeedbackStore for FeedbackMemory {
    async fn get(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
    ) -> Result<FeedbackDetails, AgentReadError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(owner.clone(), agent.clone(), sequence))
            .cloned()
            .unwrap_or_default())
    }
    async fn set_reasons(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
        request: &FeedbackReasonRequest,
    ) -> Result<FeedbackDetails, FeedbackReasonError> {
        let mut rows = self.0.lock().unwrap();
        let current = rows
            .entry((owner.clone(), agent.clone(), sequence))
            .or_default();
        *current = current.with_reasons(request)?;
        Ok(current.clone())
    }
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
            .filter(|((subject, agent_id, sequence), state)| {
                subject == owner
                    && agent_id == agent
                    && *sequence > after
                    && *sequence <= through
                    && state.vote.is_some()
            })
            .take(usize::from(limit))
            .map(|((_, _, sequence), state)| MessageFeedback {
                sequence: *sequence,
                vote: state.vote.unwrap(),
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
        let current = rows.entry(key).or_default();
        *current = current.with_vote(vote)?;
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
        seeded_agent("agt_feedback", AgentStatus::Ready, 10, None),
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
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 7,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Try again".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 7 },
        ],
    )
    .await;
    (fixture, feedback, agent)
}

const LIST: &str = "/api/v1/agents/agt_feedback/message-feedback";
const EARLIER_ANSWER: &str = "/api/v1/agents/agt_feedback/messages/event:5/feedback";
const AGENT_MESSAGE: &str = "/api/v1/agents/agt_feedback/messages/event:9/feedback";

#[path = "message_feedback_reasons.rs"]
mod reasons;

#[tokio::test]
async fn message_feedback_delayed_reasons_after_up_never_restore_down_or_reasons() {
    let (fixture, feedback, _) = feedback_fixture().await;
    for vote in ["down", "up"] {
        let (status, _) = json_response(
            &fixture.router,
            request(
                Method::PUT,
                AGENT_MESSAGE,
                OWNER,
                None,
                Some(json!({"vote": vote})),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, body) = json_response(
        &fixture.router,
        request(Method::PUT, &format!("{AGENT_MESSAGE}/reasons"), OWNER, None,
            Some(json!({"expected_version": 1, "request_id": "delayed-reasons", "reasons": ["inaccurate"], "comment": "Old draft"}))),
    ).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let reopened = agent_router(fixture.state.clone().with_message_feedback(feedback));
    let (status, body) = json_response(
        &reopened,
        request(Method::GET, AGENT_MESSAGE, OWNER, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["vote"], "up");
    assert_eq!(body["reasons"], json!([]));
    assert_eq!(body["comment"], "");
}

#[tokio::test]
async fn message_feedback_accepts_only_the_completed_final_answer_and_reopens_without_new_turns() {
    let mut fixture = fixture().await;
    let feedback = Arc::new(FeedbackMemory::default());
    fixture.state = fixture.state.with_message_feedback(feedback.clone());
    fixture.router = agent_router(fixture.state.clone());
    insert_agent(
        &fixture.store,
        seeded_agent("agt_feedback", AgentStatus::Ready, 7, None),
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
                content: "Checking the evidence".into(),
            },
            AgentLogKind::AgentMessage {
                content: "The final answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ],
    )
    .await;
    let transcript_path = "/api/v1/agents/agt_feedback/transcript";
    let (status, before) = json_response(
        &fixture.router,
        request(Method::GET, transcript_path, OWNER, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for (message, expected) in [
        (4, StatusCode::NOT_FOUND),
        (5, StatusCode::NOT_FOUND),
        (6, StatusCode::OK),
    ] {
        let (status, body) = json_response(
            &fixture.router,
            request(
                Method::PUT,
                &format!("/api/v1/agents/agt_feedback/messages/event:{message}/feedback"),
                OWNER,
                None,
                Some(json!({"vote":"up"})),
            ),
        )
        .await;
        assert_eq!(status, expected, "{body}");
    }
    let reopened = agent_router(fixture.state.clone().with_message_feedback(feedback));
    let (status, votes) =
        json_response(&reopened, request(Method::GET, LIST, OWNER, None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        votes,
        json!({"items":[{"message_id":"event:6","vote":"up"}],"next":null})
    );
    let (status, after) = json_response(
        &reopened,
        request(Method::GET, transcript_path, OWNER, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after, before);
}

#[tokio::test]
async fn message_feedback_is_persisted_metadata_that_never_creates_turns_or_runtime_input() {
    let (fixture, feedback, agent) = feedback_fixture().await;
    let transcript_path = format!("/api/v1/agents/{}/transcript", agent.id);
    let before = json_response(
        &fixture.router,
        request(Method::GET, &transcript_path, OWNER, None, None),
    )
    .await;
    for (path, vote) in [
        (EARLIER_ANSWER, "up"),
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
        json!({"items":[{"message_id":"event:5","vote":"up"}],"next":5})
    );
    let (_, page) = json_response(
        &reconnected,
        request(
            Method::GET,
            &format!("{LIST}?after=5&limit=1"),
            OWNER,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(
        page,
        json!({"items":[{"message_id":"event:9","vote":"down"}],"next":null})
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
        assert_eq!(body, json!({"message_id":"event:9","vote":vote}));
    }
    let (_, page) =
        json_response(&reconnected, request(Method::GET, LIST, OWNER, None, None)).await;
    assert_eq!(
        page,
        json!({"items":[{"message_id":"event:5","vote":"up"}],"next":null})
    );
    assert_eq!(
        json_response(
            &reconnected,
            request(Method::GET, &transcript_path, OWNER, None, None)
        )
        .await,
        before
    );
    // TestRegistry.resolve always refuses; all successful feedback writes therefore
    // also prove this feature does not resolve or send work to the runtime.
}

#[tokio::test]
async fn message_feedback_rejects_foreign_unauthenticated_ungranted_and_nonmessage_targets() {
    let (fixture, _, _) = feedback_fixture().await;
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
    for message in [
        "event:1",
        "event:2",
        "event:3",
        "event:4",
        "event:6",
        "event:7",
        "event:8",
        "event:10",
        "event:999",
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
    let (status, page) = json_response(
        &fixture.router,
        request(Method::GET, LIST, OWNER, None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page, json!({"items": [], "next": null}));
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
    let agent_id = AgentId("agt_shared_read".into());
    let mut agent = fixture.store.get_agent(&agent_id).await.unwrap().unwrap();
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 10,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Retry the review".into(),
        },
        AgentLogKind::AgentMessage {
            content: "Review complete".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 10 },
    ] {
        fixture
            .store
            .append_agent_log(&agent_id, NOW.into(), kind)
            .await
            .unwrap();
    }
    let revision = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq = 13;
    fixture.store.save_agent(revision, agent).await.unwrap();
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
            "/messages/event:12/feedback",
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
    let path = format!("{base}/messages/event:12/feedback");
    assert_eq!(
        response(
            &router,
            make(Method::PUT, &path, OWNER, Some(json!({"vote":"up"})))
        )
        .await
        .0,
        StatusCode::OK
    );
    let before = json_response(
        &router,
        make(
            Method::GET,
            &format!("{base}/message-feedback"),
            OWNER,
            None,
        ),
    )
    .await;
    let mut revoked = request(
        Method::PUT,
        &path,
        OWNER,
        None,
        Some(json!({"vote":"down"})),
    );
    revoked.extensions_mut().insert(Authorization::none());
    assert_eq!(response(&router, revoked).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        json_response(
            &router,
            make(
                Method::GET,
                &format!("{base}/message-feedback"),
                OWNER,
                None
            )
        )
        .await,
        before
    );
}

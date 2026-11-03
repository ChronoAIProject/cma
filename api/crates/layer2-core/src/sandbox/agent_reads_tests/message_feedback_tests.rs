use std::collections::BTreeMap;

use super::*;
use crate::sandbox::message_feedback::{
    FeedbackDetails, FeedbackReasonError, FeedbackReasonRequest, MessageFeedback,
    MessageFeedbackService, MessageFeedbackStore, MessageVote,
};

#[derive(Default)]
struct FeedbackStore(Mutex<BTreeMap<(SubjectId, AgentId, u64), FeedbackDetails>>);

#[async_trait]
impl MessageFeedbackStore for FeedbackStore {
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
        let key = (owner.clone(), agent.clone(), sequence);
        let mut rows = self.0.lock().unwrap();
        let current = rows.entry(key).or_default();
        *current = current.with_vote(vote)?;
        Ok(())
    }
}

#[tokio::test]
async fn feedback_reopens_only_the_completed_final_answer_without_changing_the_transcript() {
    let store = Arc::new(InMemorySandboxStore::default());
    let agent = insert_hierarchy(
        &store,
        "agt_feedback",
        "wtr_feedback",
        "wks_feedback",
        "sbx_feedback",
        "owner",
        AgentStatus::Starting,
    )
    .await;
    let agent = replace_history(
        &store,
        agent,
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-1".into(),
                runtime_generation: 1,
            },
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
                content: "Final answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ],
        Some("conversation-1"),
        Some(1),
        None,
    )
    .await;
    let owner = caller("owner", true);
    let reads = service(store.clone());
    let before = reads.transcript(&owner, &agent.id).await.unwrap();
    let persistence = Arc::new(FeedbackStore::default());
    let feedback = MessageFeedbackService::new(reads, persistence.clone());
    for sequence in [4, 5] {
        assert!(matches!(
            feedback
                .set(&owner, &agent.id, sequence, Some(MessageVote::Up))
                .await,
            Err(AgentReadError::NotFound)
        ));
    }
    feedback
        .set(&owner, &agent.id, 6, Some(MessageVote::Down))
        .await
        .unwrap();
    let reopened = MessageFeedbackService::new(service(store.clone()), persistence);
    assert_eq!(
        reopened
            .list(&owner, &agent.id, 0, 200)
            .await
            .unwrap()
            .items,
        vec![MessageFeedback {
            sequence: 6,
            vote: MessageVote::Down
        }]
    );
    assert_eq!(
        service(store).transcript(&owner, &agent.id).await.unwrap(),
        before
    );
    for (vote, expected) in [
        (
            Some(MessageVote::Up),
            vec![MessageFeedback {
                sequence: 6,
                vote: MessageVote::Up,
            }],
        ),
        (None, vec![]),
        (None, vec![]),
    ] {
        reopened.set(&owner, &agent.id, 6, vote).await.unwrap();
        let page = reopened.list(&owner, &agent.id, 0, 200).await.unwrap();
        assert_eq!(page.items, expected);
    }
}

#[tokio::test]
async fn feedback_preserves_legacy_votes_but_rejects_unfinished_failed_and_unauthorized_writes() {
    for outcome in ["active", "failed", "interrupted", "gap", "empty"] {
        let store = Arc::new(InMemorySandboxStore::default());
        let agent = insert_hierarchy(
            &store,
            "agt_feedback",
            "wtr_feedback",
            "wks_feedback",
            "sbx_feedback",
            "owner",
            AgentStatus::Starting,
        )
        .await;
        let mut history = vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "conversation-1".into(),
                runtime_generation: 1,
            },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Question".into(),
            },
            AgentLogKind::AgentMessage {
                content: if outcome == "empty" {
                    "  "
                } else {
                    "Retained answer"
                }
                .into(),
            },
        ];
        match outcome {
            "failed" => history.push(AgentLogKind::TurnFailed {
                turn_sequence: 3,
                failure: TypedFailure {
                    class: "failed".into(),
                    detail: "Task failed".into(),
                },
            }),
            "interrupted" => history.push(AgentLogKind::TurnInterrupted { turn_sequence: 3 }),
            "gap" => history.extend([
                AgentLogKind::ConversationGap { frames: 2 },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ]),
            "empty" => history.push(AgentLogKind::TurnCompleted { turn_sequence: 3 }),
            _ => {}
        }
        let agent = replace_history(
            &store,
            agent,
            history,
            Some("conversation-1"),
            Some(1),
            (outcome == "active").then_some(3),
        )
        .await;
        let owner = caller("owner", true);
        let persistence = Arc::new(FeedbackStore::default());
        persistence
            .set(&owner.subject, &agent.id, 4, Some(MessageVote::Up))
            .await
            .unwrap();
        persistence
            .set(&owner.subject, &agent.id, 5, Some(MessageVote::Down))
            .await
            .unwrap();
        let reads = service(store);
        let before = reads.transcript(&owner, &agent.id).await.unwrap();
        let feedback = MessageFeedbackService::new(reads.clone(), persistence);
        for sequence in [4, 5] {
            for vote in [Some(MessageVote::Up), Some(MessageVote::Down), None] {
                assert!(
                    matches!(
                        feedback.set(&owner, &agent.id, sequence, vote).await,
                        Err(AgentReadError::NotFound)
                    ),
                    "{outcome}"
                );
            }
        }
        assert!(matches!(
            feedback
                .set(&caller("other", true), &agent.id, 5, Some(MessageVote::Up))
                .await,
            Err(AgentReadError::NotFound)
        ));
        assert!(matches!(
            feedback
                .list(&caller("other", true), &agent.id, 0, 200)
                .await,
            Err(AgentReadError::NotFound)
        ));
        assert!(matches!(
            feedback
                .set(&caller("owner", false), &agent.id, 5, None)
                .await,
            Err(AgentReadError::Forbidden)
        ));
        assert_eq!(
            feedback
                .list(&owner, &agent.id, 0, 200)
                .await
                .unwrap()
                .items,
            vec![
                MessageFeedback {
                    sequence: 4,
                    vote: MessageVote::Up
                },
                MessageFeedback {
                    sequence: 5,
                    vote: MessageVote::Down
                },
            ]
        );
        assert_eq!(reads.transcript(&owner, &agent.id).await.unwrap(), before);
    }
}

use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, database, ready_agent,
};
use layer2_core::sandbox::agent_io::AnswerContext;
use layer2_core::{AgentId, SubjectId};

fn work(id: &str, agent_id: AgentId, now: u64, delay: u64, answer_sequence: u64) -> SuggestionWork {
    SuggestionWork {
        id: id.into(),
        owner: SubjectId("owner".into()),
        agent_id,
        scope: AnswerContext {
            turn_sequence: answer_sequence - 2,
            answer_sequence,
        },
        completed_sequence: answer_sequence + 1,
        strategy_version: STRATEGY_VERSION.into(),
        authority: "synthetic".into(),
        status: SuggestionStatus::Running,
        context: SuggestionContext {
            goal: "Choose a database.".into(),
            answer: "SQLite suits a local-only app.".into(),
            confirmed_resources: vec![],
        },
        set: None,
        created_at_ms: now,
        started_at_ms: Some(now + 1),
        deadline_ms: Some(now + delay),
        observation: None,
    }
}

fn completion(claim: &SuggestionWork) -> SuggestionCompletion {
    SuggestionCompletion {
        status: SuggestionStatus::Ready,
        set: Some(SuggestedReplySet {
            id: claim.id.clone(),
            viewer_id: claim.owner.to_string(),
            agent_id: claim.agent_id.to_string(),
            answer_id: format!("event:{}", claim.scope.answer_sequence),
            scope: claim.scope,
            source: SuggestionSource {
                kind: "generated".into(),
                version: STRATEGY_VERSION.into(),
            },
            items: vec![SuggestedReply {
                id: "choice".into(),
                text: "Explain SQLite backups.".into(),
                rank: 0,
            }],
        }),
        observation: SuggestionObservation {
            elapsed_ms: 1,
            usage: Default::default(),
            failure: None,
        },
    }
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set with failpoints"]
async fn suggestion_control_excludes_only_one_running_claim_through_release_and_until_cap() {
    let (client, name, store) = database().await;
    let parent = active_sandbox("sbx_control");
    let agent = ready_agent(&parent, "agt_control");
    let target = Target {
        sandbox_id: parent.id.to_string(),
        agent_id: agent.id.to_string(),
    };
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();

    let control = SuggestionControl::new(&store, target.clone());
    let mut replica_options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let replica_app = format!("suggestion-expiry-{}", uuid::Uuid::new_v4().simple());
    replica_options.app_name = Some(replica_app.clone());
    let replica_client = mongodb::Client::with_options(replica_options).unwrap();
    let mut replica = MongoSandboxStore::open(&replica_client, &name)
        .await
        .unwrap();
    replica.writer_policy = std::sync::Arc::new(
        serde_json::from_value(serde_json::json!({
            "mode":"accounts", "accounts":["owner"], "capabilities":["suggested_followup_admission"]
        }))
        .unwrap(),
    );
    let operator = SuggestionControl::new(&replica, target);
    replica.suggestion_control = Some(operator.clone());
    assert!(operator.arm(Mode::LateReady).await.unwrap());
    assert!(!control.arm(Mode::PauseFirst).await.unwrap());
    let arm = control.read().await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    let now = unix_millis();
    let held = work("held", agent.id.clone(), now, 3_000, 5);
    let other = work("other", agent.id, now, 3_000, 9);
    let unrelated_agent = work(
        "unrelated-agent",
        AgentId("agt_unrelated".into()),
        now,
        3_000,
        17,
    );
    let stale = work(
        "stale",
        held.agent_id.clone(),
        arm.armed_at_ms as u64 - 1,
        3_000,
        13,
    );
    let deadline = held.deadline_ms.unwrap();
    store
        .suggestions
        .insert_many(
            [&held, &other, &stale, &unrelated_agent]
                .map(|claim| crate::store::suggestions::document(claim).unwrap()),
        )
        .await
        .unwrap();

    control
        .hold_before_publish(&stale, &completion(&stale))
        .await;
    assert!(operator.read().await.unwrap().unwrap().held_at_ms.is_none());
    let publishing = {
        let held = held.clone();
        tokio::spawn(async move {
            control.hold_before_publish(&held, &completion(&held)).await;
        })
    };
    let bound = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let report = operator.read().await.unwrap().unwrap();
            if report.held_at_ms.is_some() {
                break report;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(bound.answer_sequence, Some(5));
    assert_eq!(bound.durable_status.as_deref(), Some("running"));
    assert_eq!(bound.durable_has_set, Some(false));
    let generation = bound.claim_started_at_ms.unwrap();
    assert!(!operator.release(generation).await.unwrap());

    // Lose only the replica's marker lookup at the deadline. Another Agent's
    // overdue claim can still advance while the configured Agent stays fenced.
    client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1},
            "data": {"appName": &replica_app, "failCommands": ["find"], "errorCode": 2},
        })
        .await
        .unwrap();
    assert!(replica.claim_suggestions(deadline).await.unwrap().is_none());
    for (id, expected) in [("held", "running"), ("unrelated-agent", "failed")] {
        let row = store
            .suggestions
            .find_one(doc! {"_id": id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_str("status").unwrap(), expected);
    }

    // Drive the scanner at the claim's deadline, as a second worker would.
    for _ in 0..3 {
        assert!(replica.claim_suggestions(deadline).await.unwrap().is_none());
    }
    for unrelated in ["other", "stale"] {
        let row = store
            .suggestions
            .find_one(doc! {"_id":unrelated})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_str("status").unwrap(), "failed");
    }
    assert_eq!(
        operator
            .read()
            .await
            .unwrap()
            .unwrap()
            .durable_status
            .as_deref(),
        Some("running")
    );
    tokio::time::sleep(Duration::from_millis(
        deadline.saturating_sub(unix_millis()) + 1,
    ))
    .await;
    assert!(!operator.release(generation - 1).await.unwrap());
    let (left, right) = tokio::join!(operator.release(generation), operator.release(generation));
    assert!(left.unwrap() && right.unwrap());
    tokio::time::timeout(Duration::from_secs(2), publishing)
        .await
        .unwrap()
        .unwrap();
    assert!(replica
        .claim_suggestions(unix_millis())
        .await
        .unwrap()
        .is_none());
    let released = operator.read().await.unwrap().unwrap();
    assert!(released.released_at_ms.is_some());
    assert_eq!(released.durable_status.as_deref(), Some("running"));
    client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1},
            "data": {"appName": replica_app, "failCommands": ["find"], "errorCode": 2},
        })
        .await
        .unwrap();
    assert!(replica
        .claim_suggestions(released.expires_at_ms as u64)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        operator
            .read()
            .await
            .unwrap()
            .unwrap()
            .durable_status
            .as_deref(),
        Some("failed")
    );
    client.database(&name).drop().await.unwrap();
}

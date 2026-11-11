//! Real startup/fleet entry points, not a synthetic call to the hold predicate.
use super::*;
use layer2_core::sandbox::{AgentLogKind, AgentWorkStep};

async fn complete_history(store: &MongoSandboxStore, agent: &mut Agent) {
    let kinds = [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.creation_worktree_id().clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Fixture input.".into(),
        },
        AgentLogKind::AgentMessage {
            content: "Fixture result.".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ];
    store
        .agent_log
        .insert_many(kinds.into_iter().enumerate().map(|(i, kind)| {
            log_document(&AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: i as u64 + 1,
                appended_at: support::NOW.into(),
                kind,
            })
            .unwrap()
        }))
        .await
        .unwrap();
    agent.conversation.last_seq = 6;
    assert_eq!(agent.work.step, AgentWorkStep::None);
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(agent).unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and integrity hooks"]
async fn startup_and_fleet_preparation_have_positive_controls_and_refuse_both_holds() {
    for fleet in [false, true] {
        for held in [0, 1, 2] {
            let (client, name, store) = database().await;
            let input = support::fixture(&store, true).await;
            let mut agent = input.accepted.plan().source.agents[0].clone();
            complete_history(&store, &mut agent).await;
            if held != 0 {
                hold(&store, input, held == 1).await;
            }
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let before = support::all_rows(&store).await;
            if fleet {
                reopened
                    .with_fleet_log_percentage(100)
                    .prepare_fleet_log(&agent.id)
                    .await;
            } else {
                let result = reopened
                    .prepare_bounded_admission(std::slice::from_ref(&agent.id))
                    .await
                    .map(|configured| configured.bounded_admission_agents.contains(&agent.id));
                if held == 0 {
                    assert_eq!(result, Ok(true));
                } else {
                    assert!(result.is_err());
                }
            }
            if held == 0 {
                let document = row(&store, &agent).await;
                assert!(parse_integrity(&document).unwrap().unwrap().semantic.valid);
                assert_ne!(support::all_rows(&store).await, before);
            } else {
                assert_eq!(support::all_rows(&store).await, before);
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and integrity hooks"]
async fn preparation_cas_failure_rolls_back_the_parent_guard_before_positive_retry() {
    let (client, name, store) = database().await;
    let input = support::fixture(&store, true).await;
    let mut agent = input.accepted.plan().source.agents[0].clone();
    complete_history(&store, &mut agent).await;
    client
        .database(&name)
        .run_command(doc! {
            "collMod":store.agents.name(),
            "validator":{"reject_fixture":{"$exists":true}},
            "validationLevel":"strict", "validationAction":"error",
        })
        .await
        .unwrap();
    let before = support::all_rows(&store).await;
    assert!(store
        .clone()
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .is_err());
    assert_eq!(support::all_rows(&store).await, before);
    client
        .database(&name)
        .run_command(doc! {"collMod":store.agents.name(),"validator":{}})
        .await
        .unwrap();
    assert!(store
        .clone()
        .prepare_bounded_admission(std::slice::from_ref(&agent.id))
        .await
        .is_ok());
    assert!(row(&store, &agent).await.contains_key(INTEGRITY_FIELD));
    client.database(&name).drop().await.unwrap();
}

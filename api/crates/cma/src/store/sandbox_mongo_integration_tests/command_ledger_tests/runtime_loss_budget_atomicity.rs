use super::fixtures::{ledger, running_with, settled, turn};
use super::*;
use mongodb::{bson::Document, options::ClientOptions};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_refuses_misbound_saved_obligations_without_publication() {
    for field in ["owner", "agent_id", "turn_sequence"] {
        for already_settled in [false, true] {
            let (client, name) = database("budget_runtime_identity").await;
            let (store, agent, parent, _) = running_with(&client, &name, false, false, false).await;
            let key = format!(
                "turn:{}:{}",
                agent.id,
                agent.conversation.active_turn.unwrap()
            );
            let mut update = doc! {"record.reservation.settled":already_settled};
            update.insert(
                format!("record.{field}"),
                if field == "turn_sequence" {
                    mongodb::bson::Bson::Int64(1000)
                } else {
                    "foreign".into()
                },
            );
            store
                .model_budget
                .update_one(doc! {"_id":key}, doc! {"$set":update})
                .await
                .unwrap();
            let before = ledger(&store).await;
            assert!(store.recover_runtime_loss(loss(&parent)).await.is_err());
            assert_eq!(ledger(&store).await, before);
            assert_eq!(
                store.get_agent(&agent.id).await.unwrap().as_ref(),
                Some(&agent)
            );
            assert_eq!(
                store.get_sandbox(&parent.id).await.unwrap().as_ref(),
                Some(&parent)
            );
            client.database(&name).drop().await.unwrap();
        }
    }
}

pub(super) async fn publication_rows(store: &MongoSandboxStore) -> Vec<Vec<Document>> {
    let db = store.client.database(&store.agents.namespace().db);
    let mut rows = Vec::new();
    for name in [
        "agents",
        "sandboxes",
        "agent_log",
        "sandbox_events",
        crate::store::agent_log_segments::HEADS,
        crate::store::agent_log_segments::PROJECTIONS,
        crate::store::agent_log_segments::PUBLICATIONS,
    ] {
        rows.push(
            db.collection::<Document>(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    rows
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn runtime_loss_budget_is_atomic_across_abort_retry_and_unknown_commit() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    for segmented in [false, true] {
        for mode in ["abort", "transient", "committed", "uncommitted"] {
            let name = format!("budget_runtime_atomic_{}", uuid::Uuid::new_v4().simple());
            let app = format!("budget-atomic-{name}");
            let mut options = ClientOptions::parse(&uri).await.unwrap();
            options.app_name = Some(app.clone());
            let client = Client::with_options(options).unwrap();
            let (store, agent, parent, _) =
                running_with(&client, &name, segmented, true, false).await;
            let before = ledger(&store).await;
            let publication = publication_rows(&store).await;
            let obligation = turn(&store, &agent).await;
            let (times, mut data) = match mode {
                "abort" => (1, doc! {"failCommands":["insert"],"errorCode":2_i32}),
                "transient" => (
                    1,
                    doc! {"failCommands":["update"],"errorCode":112_i32,"errorLabels":["TransientTransactionError"]},
                ),
                "committed" => (
                    5,
                    doc! {"failCommands":["commitTransaction"],"writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"synthetic uncertain recovery"}},
                ),
                "uncommitted" => (
                    5,
                    doc! {"failCommands":["commitTransaction"],"errorCode":50_i32,"errorLabels":["UnknownTransactionCommitResult"]},
                ),
                _ => unreachable!(),
            };
            data.insert("appName", app);
            admin
                .database("admin")
                .run_command(
                    doc! {"configureFailPoint":"failCommand","mode":{"times":times},"data":data},
                )
                .await
                .unwrap();
            let outcome = store.recover_runtime_loss(loss(&parent)).await;
            admin
                .database("admin")
                .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
                .await
                .unwrap();
            if matches!(mode, "abort" | "uncommitted") {
                assert!(
                    matches!(outcome, Err(store::SandboxStoreError::Unavailable(_))),
                    "{mode}: {outcome:?}"
                );
                assert_eq!(ledger(&store).await, before, "{mode}");
                assert_eq!(publication_rows(&store).await, publication, "{mode}");
                assert_eq!(
                    store.get_agent(&agent.id).await.unwrap().as_ref(),
                    Some(&agent)
                );
                assert_eq!(
                    store.get_sandbox(&parent.id).await.unwrap().as_ref(),
                    Some(&parent)
                );
                if mode == "uncommitted" {
                    client.database(&name).drop().await.unwrap();
                    continue;
                }
                store.recover_runtime_loss(loss(&parent)).await.unwrap();
            } else {
                assert_eq!(
                    outcome.unwrap(),
                    store::SandboxRuntimeLossOutcome::Failed,
                    "{mode}"
                );
            }
            settled(&store, &agent, &obligation).await;
            let after = ledger(&store).await;
            assert_eq!(
                store.recover_runtime_loss(loss(&parent)).await.unwrap(),
                store::SandboxRuntimeLossOutcome::Superseded
            );
            assert_eq!(ledger(&store).await, after);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_budget_requires_the_captured_interruption() {
    let (client, name) = database("budget_runtime_transition").await;
    let (store, agent, parent, _) = running_with(&client, &name, false, false, false).await;
    let request = loss(&parent);
    let store::SandboxRuntimeLossApplication::Persist(mutation) =
        store::apply_sandbox_runtime_loss(
            &parent,
            &[(agent.clone(), agent.conversation.last_seq)],
            &request,
        )
        .unwrap()
    else {
        panic!("active turn must be recovered");
    };
    let before = ledger(&store).await;
    for corruption in [
        "missing",
        "duplicate",
        "wrong_turn",
        "replacement",
        "owner",
        "revision",
    ] {
        let mut change = mutation.agents[0].clone();
        let turn = agent.conversation.active_turn.unwrap();
        match corruption {
            "missing" => change
                .logs
                .retain(|log| !matches!(log, AgentLogKind::TurnInterrupted { .. })),
            "duplicate" => change.logs.push(AgentLogKind::TurnInterrupted {
                turn_sequence: turn,
            }),
            "wrong_turn" => {
                change
                    .logs
                    .retain(|log| !matches!(log, AgentLogKind::TurnInterrupted { .. }));
                change.logs.push(AgentLogKind::TurnInterrupted {
                    turn_sequence: turn + 1,
                });
            }
            "replacement" => change.agent.conversation.active_turn = Some(turn + 1),
            "owner" => change.agent.owner = SubjectId::new("foreign"),
            "revision" => change.expected_revision += 1,
            _ => unreachable!(),
        }
        let mut session = client.start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        let result = crate::store::budget::settle_runtime_loss(
            &store,
            &agent,
            &change,
            &request.observed_at,
            &mut session,
        )
        .await;
        session.abort_transaction().await.unwrap();
        assert!(result.is_err(), "{corruption}");
        assert_eq!(ledger(&store).await, before);
    }
    client.database(&name).drop().await.unwrap();
}

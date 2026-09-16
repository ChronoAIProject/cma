use mongodb::{options::ClientOptions, Client};

use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn checkpoint_reconciles_ambiguous_quiet_and_boundary_commits_without_duplicate_logs() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-message-checkpoint-{suffix}");
    let name = format!("cma_message_checkpoint_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let current = working(&store, "sbx_checkpoint_ambiguous").await;
    let mut previous = None;
    for quiet in [true, false] {
        let mutation = (!quiet).then(|| {
            let mut mutation = update(&current, "prefix and final suffix");
            mutation.agent.conversation.active_turn = None;
            mutation.agent.status = AgentStatus::Ready;
            mutation.agent_logs.push(AgentLogKind::TurnCompleted {
                turn_sequence: current.conversation.active_turn.unwrap(),
            });
            mutation.agent.conversation.last_seq += 1;
            mutation
        });
        let mut checkpoint = mutation
            .as_ref()
            .map(boundary)
            .unwrap_or_else(|| partial(&current, "prefix"));
        checkpoint.outcome.turn_closed = !quiet;
        admin.database("admin").run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 5_i32 },
            "data": { "failCommands": ["commitTransaction"], "appName": &app_name,
                "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous message checkpoint" } },
        }).await.unwrap();
        let result = store
            .commit_agent_message_update(
                current.clone(),
                previous.clone(),
                checkpoint.clone(),
                mutation.clone(),
            )
            .await;
        admin
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .expect("disable failpoint before assertions");
        assert_eq!(result.unwrap(), checkpoint.outcome);
        assert_eq!(
            store.agent_message_checkpoint(&current.id).await.unwrap(),
            Some(checkpoint.clone())
        );
        assert_eq!(
            store
                .commit_agent_message_update(
                    current.clone(),
                    previous,
                    checkpoint.clone(),
                    mutation.clone()
                )
                .await
                .unwrap(),
            checkpoint.outcome
        );
        let expected_agent = mutation
            .map(|value| value.agent)
            .unwrap_or_else(|| current.clone());
        assert_eq!(
            store.get_agent(&current.id).await.unwrap(),
            Some(expected_agent.clone())
        );
        assert_eq!(
            store.agent_log(&current.id, 0, 100).await.unwrap().len(),
            expected_agent.conversation.last_seq as usize
        );
        previous = Some(checkpoint);
    }
    client.database(&name).drop().await.unwrap();
}

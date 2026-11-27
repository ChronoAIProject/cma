use super::*;
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions, Client};
use std::sync::{Arc, Mutex};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn prepared_open_ready_recovers_a_lost_commit_acknowledgement_without_republishing() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app = format!("cma-prepared-ack-{}", uuid::Uuid::new_v4().simple());
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let observed_commands = Arc::clone(&commands);
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let command = &event.command;
            if event.command_name == "commitTransaction"
                || command.get_bool("startTransaction").ok() == Some(true)
            {
                observed_commands.lock().unwrap().push((
                    event.command_name,
                    command.get_document("lsid").unwrap().clone(),
                    command.get_i64("txnNumber").unwrap(),
                ));
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &app).await.unwrap();
    let (agent, sandbox, workspace, worktree) = hierarchy(&store).await;
    let done = completion(&agent, &worktree);
    let observed = PreparedAgentOpenContext {
        sandbox,
        workspace,
        worktree,
    };
    commands.lock().unwrap().clear();

    let admin = admin.database("admin");
    let armed = admin
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 10_i32},
            "data": {"failCommands": ["commitTransaction"], "appName": &app,
                "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected prepared publication acknowledgement loss"}},
        })
        .await
        .unwrap();
    let result = store
        .commit_prepared_agent_open_ready(done.clone(), observed.clone())
        .await;
    let disarmed = admin
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();

    // Every commit send stayed on the initial transaction, and every reply
    // was affected. The returned success therefore required reconciliation.
    assert!(result.unwrap());
    let trace = commands.lock().unwrap().clone();
    let starts: Vec<_> = trace
        .iter()
        .filter(|(name, ..)| name != "commitTransaction")
        .collect();
    let commits: Vec<_> = trace
        .iter()
        .filter(|(name, ..)| name == "commitTransaction")
        .collect();
    assert_eq!(starts.len(), 1);
    assert!(commits.len() >= 5);
    assert!(commits
        .iter()
        .all(|(_, session, transaction)| *session == starts[0].1 && *transaction == starts[0].2));
    let entered = |response: &Document| {
        response
            .get_i64("count")
            .or_else(|_| response.get_i32("count").map(i64::from))
            .unwrap()
    };
    assert_eq!(entered(&disarmed) - entered(&armed), commits.len() as i64);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap(),
        done.agent
    );
    for (collection, id) in [
        (&store.sandboxes, observed.sandbox.id.as_str()),
        (&store.workspaces, observed.workspace.id.as_str()),
        (&store.worktrees, observed.worktree.id.as_str()),
        (&store.native_prepared, observed.sandbox.id.as_str()),
    ] {
        let row = collection
            .find_one(doc! {"_id": id})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_i64("agent_prepared_open_ready_fence").unwrap(), 1);
    }
    assert!(!store
        .commit_prepared_agent_open_ready(done, observed)
        .await
        .unwrap());
    client.database(&app).drop().await.unwrap();
}

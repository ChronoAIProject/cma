use super::*;
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
pub(super) type OriginalSession = Arc<std::sync::Mutex<Option<mongodb::bson::Document>>>;

pub(super) async fn client_for(name: &str) -> (Client, String) {
    let (client, app, _) = observed_client_for(name).await;
    (client, app)
}

pub(super) async fn observed_client_for(name: &str) -> (Client, String, OriginalSession) {
    let app = format!("ordinary-v2-{name}-{}", uuid::Uuid::new_v4());
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(app.clone());
    let original = OriginalSession::default();
    let observed = original.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "commitTransaction" {
                *observed.lock().unwrap() =
                    Some(event.command.get_document("lsid").unwrap().clone());
            }
        }
    }));
    (Client::with_options(options).unwrap(), app, original)
}

pub(super) async fn release_abandoned_transaction(admin: &Client, original: &OriginalSession) {
    let original = original
        .lock()
        .unwrap()
        .clone()
        .expect("original commit session");
    // Model server cleanup after the lost process. A failCommand refusal can
    // leave server locks although the driver already considers commit sent.
    // Kill only this operation's observed session; committed receipts survive.
    admin
        .database("admin")
        .run_command(doc! {"killSessions":[original]})
        .await
        .unwrap();
}

pub(super) async fn fail_commit(admin: &Client, app: &str, committed: bool) {
    let mut data = doc! {"failCommands":["commitTransaction"], "appName":app};
    if committed {
        data.insert(
            "writeConcernError",
            doc! {"code":50_i32,"codeName":"MaxTimeMSExpired",
            "errmsg":"injected unknown ordinary publication"},
        );
    } else {
        data.insert("errorCode", 91_i32);
        data.insert("errorLabels", vec!["UnknownTransactionCommitResult"]);
    }
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand",
        "mode":{"times":5_i32},"data":data})
        .await
        .unwrap();
}

pub(super) async fn clear(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn adopted_conversation_unknown_commit_uses_original_receipt_and_never_replans() {
    let (admin, name, _, current, _) = working("v2_unknown_conversation").await;
    let (client, app) = client_for("conversation").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mutation = output(&current, "uncertain original output");
    fail_commit(&admin, &app, true).await;
    let result = store.commit_agent_conversation(mutation.clone()).await;
    clear(&admin).await;
    assert_eq!(
        result.unwrap(),
        conversation::conversation_logs(&mutation).unwrap()
    );
    let reopened = MongoSandboxStore::open(&admin, &name).await.unwrap();
    assert_eq!(
        reopened.get_agent(&current.id).await.unwrap(),
        Some(mutation.agent.clone())
    );
    let before = snapshot(&admin, &name).await;
    assert!(matches!(
        reopened.commit_agent_conversation(mutation.clone()).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(snapshot(&admin, &name).await, before);
    let absent = output(&mutation.agent, "must remain unpublished");
    fail_commit(&admin, &app, false).await;
    let result = store.commit_agent_conversation(absent).await;
    clear(&admin).await;
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    assert_eq!(snapshot(&admin, &name).await, before);
    admin.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn adopted_checkpoint_unknown_quiet_and_boundary_replay_exactly_after_restart() {
    let (admin, name, _, current, _) = working("v2_unknown_checkpoint").await;
    let (client, app) = client_for("checkpoint").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let previous = partial(&current, "retained prefix");
    fail_commit(&admin, &app, true).await;
    let result = store
        .commit_agent_message_update(current.clone(), None, previous.clone(), None)
        .await;
    clear(&admin).await;
    assert_eq!(result.unwrap(), previous.outcome);
    let mutation = output(&current, "retained prefix and suffix");
    let next = boundary(&mutation);
    fail_commit(&admin, &app, true).await;
    let result = store
        .commit_agent_message_update(
            current.clone(),
            Some(previous.clone()),
            next.clone(),
            Some(mutation.clone()),
        )
        .await;
    clear(&admin).await;
    assert_eq!(result.unwrap(), next.outcome);
    let reopened = MongoSandboxStore::open(&admin, &name).await.unwrap();
    let before = snapshot(&admin, &name).await;
    assert_eq!(
        reopened
            .commit_agent_message_update(
                current.clone(),
                Some(previous.clone()),
                next.clone(),
                Some(mutation.clone())
            )
            .await
            .unwrap(),
        next.outcome
    );
    assert_eq!(snapshot(&admin, &name).await, before);
    let mut foreign = next.clone();
    foreign.update_fingerprint = "b".repeat(64);
    assert!(matches!(
        reopened
            .commit_agent_message_update(current, Some(previous), foreign, Some(mutation.clone()))
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(snapshot(&admin, &name).await, before);

    let absent = output(&mutation.agent, "absent unknown boundary");
    let absent_checkpoint = boundary(&absent);
    fail_commit(&admin, &app, false).await;
    let result = store
        .commit_agent_message_update(
            mutation.agent.clone(),
            Some(next),
            absent_checkpoint,
            Some(absent),
        )
        .await;
    clear(&admin).await;
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    assert_eq!(snapshot(&admin, &name).await, before);
    admin.database(&name).drop().await.unwrap();
}

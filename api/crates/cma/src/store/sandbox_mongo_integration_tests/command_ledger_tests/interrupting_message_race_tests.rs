use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn interruption_pair_and_ordinary_admission_cannot_split_the_last_two_queue_slots() {
    let (client, name) = database("interruption_capacity_race").await;
    let (store, agent, control, _) = composed(&name, &client).await;
    let replica = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let mut request = pair(control);
    request.control.command.queue_capacity = 2;
    request.message.queue_capacity = 2;
    let mut ordinary = live_request(&agent, &parent, "last-slot");
    ordinary.queue_capacity = 2;
    let (paired, single) = tokio::join!(
        store.reserve_interrupting_message(request.clone()),
        replica.reserve_command(ordinary.clone())
    );
    let (commands, audits, last_order) = match (paired, single) {
        (Ok(admitted), Err(CommandLedgerError::QueueFull)) => {
            assert_eq!(admitted.control.record.receipt.agent_order, 2);
            assert_eq!(admitted.message.receipt.agent_order, 3);
            assert!(store
                .get_command(&ordinary.envelope.command_id)
                .await
                .unwrap()
                .is_none());
            (3, 2, 3)
        }
        (Err(CommandLedgerError::QueueFull), Ok(admitted)) => {
            assert_eq!(record(admitted).receipt.agent_order, 2);
            for id in [
                &request.control.command.envelope.command_id,
                &request.message.envelope.command_id,
            ] {
                assert!(store.get_command(id).await.unwrap().is_none());
            }
            (2, 0, 2)
        }
        outcome => panic!("one complete admission must win the bounded queue: {outcome:?}"),
    };
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        commands
    );
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        audits
    );
    assert_eq!(
        store
            .agent_command_orders
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
            .get_i64("last_order")
            .unwrap(),
        last_order
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn interruption_pair_unknown_commit_recovers_both_original_receipts() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("cma_interruption_{suffix}");
    let app = format!("cma-interruption-{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let (store, _, control, _) = composed(&name, &client).await;
    let request = pair(control);
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"], "appName":&app,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected interruption ambiguity"}},
    }).await.unwrap();
    let result = store.reserve_interrupting_message(request.clone()).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    assert_eq!(result.unwrap_err(), CommandLedgerError::Unavailable);
    crate::store::test_visibility::majority_barrier(&admin, &name).await;
    let accepted = store
        .reserve_interrupting_message(request.clone())
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&admin, &name).await.unwrap();
    assert_eq!(
        reopened
            .reserve_interrupting_message(request)
            .await
            .unwrap(),
        accepted
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        3
    );
    assert_eq!(
        store
            .command_authorization_audits
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .agent_commands
            .count_documents(doc! {"explicit_control":{"$exists":true}})
            .await
            .unwrap(),
        1
    );
    admin.database(&name).drop().await.unwrap();
}

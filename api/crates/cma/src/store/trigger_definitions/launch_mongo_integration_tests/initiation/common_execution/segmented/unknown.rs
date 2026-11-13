use super::*;

pub(super) async fn isolate(f: &mut Fixture) -> String {
    let app = format!(
        "segmented-trigger-unknown-{}",
        uuid::Uuid::new_v4().simple()
    );
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let client = mongodb::Client::with_options(options).unwrap();
    f.resources = Arc::new(
        crate::store::MongoSandboxStore::open(&client, &f.seed.database)
            .await
            .unwrap(),
    );
    app
}

pub(super) async fn lose_ack(f: &Fixture, app: &str) {
    f.seed.client.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":app,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected segmented commit ambiguity"}}
    }).await.unwrap();
}

pub(super) async fn clear(f: &Fixture) {
    f.seed
        .client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_prepared_reuse_reconciles_lost_ack_without_new_admission() {
    let (mut f, launch, request) = pending().await;
    let original = reuse::prepared(&f, &request).await;
    adopt(&f, &original).await;
    let app = isolate(&mut f).await;
    lose_ack(&f, &app).await;
    let result = f
        .resources
        .admit_agent_with_trigger_initiator(
            request.clone(),
            launch.initiation.clone().unwrap(),
            false,
        )
        .await;
    clear(&f).await;
    let result = result.unwrap();
    assert_eq!(result.creation().agent.id, original.id);
    assert!(result
        .creation()
        .agent
        .conversation
        .first_message
        .as_ref()
        .unwrap()
        .common_binding
        .is_some());
    assert_eq!(
        f.resources
            .agent_reuse_bindings
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.resources
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    append(&f, &original, "after-unknown-reuse").await;
    assert_eq!(
        f.resources
            .admit_agent_with_trigger_initiator(request, launch.initiation.clone().unwrap(), false)
            .await
            .unwrap()
            .creation()
            .agent
            .id,
        original.id
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_trigger_refusal_reconciles_lost_ack_without_second_event() {
    let mut a = accepted().await;
    let app = isolate(&mut a.f).await;
    lose_ack(&a.f, &app).await;
    let result =
        a.f.resources
            .reject_unclaimed_command(
                &a.command.receipt.envelope.command_id,
                a.command.revision,
                CommandFailureCode::QueueTimeout,
                a.command.queue_deadline.clone(),
            )
            .await;
    clear(&a.f).await;
    result.unwrap();
    assert_eq!(
        a.current_agent().await.conversation.last_seq,
        a.agent.conversation.last_seq + 1
    );
    assert_eq!(a.current_command().await.receipt, a.command.receipt);
    assert!(a.current_command().await.work.dispatch.is_none());
    a.f.close().await;
}

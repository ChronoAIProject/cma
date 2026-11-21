use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_unknown_admission_commit_recovers_all_permanent_artifacts() {
    let (mut f, launch, request) = pending().await;
    let app_name = format!("trigger-common-unknown-{}", uuid::Uuid::new_v4().simple());
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = mongodb::Client::with_options(options).unwrap();
    f.resources = Arc::new(
        crate::store::MongoSandboxStore::open(&client, &f.seed.database)
            .await
            .unwrap(),
    );
    f.seed.client.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":&app_name,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected common admission commit ambiguity"}}
    }).await.unwrap();
    let outcome = f
        .resources
        .admit_agent_with_trigger_initiator(
            request.clone(),
            launch.initiation.clone().unwrap(),
            false,
        )
        .await;
    f.seed
        .client
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let result = outcome.unwrap();
    let agent = &result.creation().agent;
    let binding = agent
        .conversation
        .first_message
        .as_ref()
        .unwrap()
        .common_binding
        .as_ref()
        .unwrap();
    let durable = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    binding.require_launch(&durable).unwrap();
    let command = f
        .resources
        .get_command(&binding.receipt().envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&command.receipt, binding.receipt());
    assert_eq!(
        f.resources
            .admit_agent_with_trigger_initiator(request, launch.initiation.unwrap(), false)
            .await
            .unwrap()
            .creation()
            .agent,
        *agent
    );
    f.close().await;
}

use super::*;
use crate::tools::threads::{fixture::App, rename_tests::rename};
use serde_json::{json, Value};
use std::time::Duration;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn uncertain_discord_rename_replays_after_gateway_restart_without_another_write() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_rename_contract_{}", uuid::Uuid::new_v4().simple());
    let database = admin.database(&name);
    let task_database = database.clone();
    let result = tokio::spawn(async move {
        exercise_rename(&uri, &task_database).await;
    })
    .await;
    database.drop().await.unwrap();
    result.unwrap();
}

async fn exercise_rename(uri: &str, database: &Database) {
    let name = database.name();
    mongo::migrate(database).await.unwrap();
    mongo::migrate_thread_fences(database).await.unwrap();
    for role in [
        mongo::role(name, false),
        mongo::thread_fences_role(name, false),
    ] {
        database.run_command(role).await.unwrap();
    }
    let password = uuid::Uuid::new_v4().to_string();
    database
        .run_command(doc! {
            "createUser":"runtime", "pwd":&password, "roles":[
                {"role":"cma_eg_runtime","db":name},
                {"role":"cma_eg_thread_fences_runtime","db":name}
            ]
        })
        .await
        .unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username("runtime".to_owned())
            .password(password)
            .source(name.to_owned())
            .build(),
    );
    let first = MongoRecords::open(
        Client::with_options(options.clone())
            .unwrap()
            .database(name),
    )
    .await
    .unwrap();
    first.thread_fences_ready().await.unwrap();
    let mut app = App::with_mongo(first).await;
    app.memory.lock().unwrap().delay = Some((
        "PATCH".into(),
        "/api/v10/channels/99".into(),
        Duration::from_secs(12),
    ));
    let target = app.target();
    let initial = rename(&app, target.clone(), "Incident triage", "rename-first").await;
    assert_eq!(initial["outcome"]["status"], "unknown", "{initial}");
    assert!(initial.get("thread").is_none());
    let writes = vec![(
        "PATCH".into(),
        "/api/v10/channels/99".into(),
        json!({"name":"Incident triage"}),
    )];
    assert_eq!(app.writes(), writes);
    assert_eq!(
        app.memory.lock().unwrap().channels["99"]["name"],
        "Incident triage"
    );
    let fences = database.collection::<Document>(mongo::THREAD_FENCES);
    assert_eq!(fences.count_documents(doc! {}).await.unwrap(), 1);
    let fence = fences.find_one(doc! {}).await.unwrap().unwrap();
    assert!(!fence.contains_key("expires_at"));
    app.memory.lock().unwrap().delay = None;

    let second = MongoRecords::open(Client::with_options(options).unwrap().database(name))
        .await
        .unwrap();
    app.restart_on_mongo(second).await;
    assert_eq!(
        rename(&app, target.clone(), "Incident triage", "rename-first").await,
        initial
    );
    let archive = app.manage(target, "archive", "archive-next").await;
    assert_eq!(archive.status(), reqwest::StatusCode::OK);
    let archive: Value = archive.json().await.unwrap();
    assert_eq!(
        archive["outcome"],
        json!({"status":"unknown","reason":"thread_mutation_in_progress_or_uncertain"})
    );
    assert!(archive.get("thread").is_none());
    assert_eq!(app.writes(), writes);
    assert_eq!(fences.find_one(doc! {}).await.unwrap().unwrap(), fence);
}

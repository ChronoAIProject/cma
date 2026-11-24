use super::profile_mongo::MongoProfileStore;
use mongodb::{bson::doc, options::ClientOptions, Client};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn initial_publication_independent_mongo_stores_keep_exact_revision_one() {
    run(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn initial_publication_unknown_commits_reconcile_exact_initial_content() {
    run(true).await;
}

async fn run(ambiguous: bool) {
    let variable = if ambiguous {
        "CHRONO_TEST_MONGODB_FAILPOINT_URI"
    } else {
        "CHRONO_TEST_MONGODB_URI"
    };
    let uri = std::env::var(variable).expect("disposable Mongo replica set");
    let name = format!("cma_initial_profiles_{}", uuid::Uuid::new_v4().simple());
    let app = format!("initial-profile-{}", uuid::Uuid::new_v4().simple());
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let first_client = Client::with_options(options.clone()).unwrap();
    let second_client = Client::with_options(options).unwrap();
    let first = MongoProfileStore::open(&first_client, &name).await.unwrap();
    let second = MongoProfileStore::open(&second_client, &name)
        .await
        .unwrap();
    if ambiguous {
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "alwaysOn",
                "data": { "failCommands": ["commitTransaction"], "appName": &app,
                    "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired",
                        "errmsg": "injected ambiguous initial publication" } },
            })
            .await
            .unwrap();
    }
    // appName scopes injection to these two clients. Disable the failpoint and
    // remove the disposable database before propagating an assertion panic.
    let result = tokio::spawn(async move {
        layer2_core::profiles::initial_publication_contract::verify(&first, &second).await;
        layer2_core::profiles::initial_publication_contract::verify_creation(
            std::sync::Arc::new(first),
            std::sync::Arc::new(second),
        )
        .await;
    })
    .await;
    if ambiguous {
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .expect("disable failpoint before assertions");
    }
    admin.database(&name).drop().await.unwrap();
    result.unwrap();
}

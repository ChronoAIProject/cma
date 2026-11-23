use super::*;
use mongodb::{bson::doc, options::ClientOptions, Client};

struct AbortingModel {
    admin: Client,
    app: String,
    failures: i32,
    unknown: bool,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl SuggestionGenerator for AbortingModel {
    async fn generate(
        &self,
        _: &SuggestionWork,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // The queue claim has already committed. Only result publication can
        // consume this app-scoped fault; the generated result is retained once.
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":if self.unknown {
                    mongodb::bson::Bson::String("alwaysOn".into())
                } else {
                    mongodb::bson::Bson::Document(doc! {"times":self.failures})
                },
                "data":{"appName":&self.app,
                    "failCommands":[if self.unknown {"commitTransaction"} else {"update"}],
                    "errorCode":if self.unknown {1_i32} else {112_i32},
                    "errorLabels":[if self.unknown {"UnknownTransactionCommitResult"} else {"TransientTransactionError"}]}
            })
            .await
            .unwrap();
        Ok(SuggestionGeneration {
            output: r#"{"suggestions":[{"text":"Explain SQLite backup options.","evidence":"local-only app"}]}"#.into(),
            usage: SuggestionUsage {
                input_tokens: Some(23),
                ..Default::default()
            },
        })
    }
}

async fn suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_case(
    failures: i32,
    unknown: bool,
) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app = format!("suggestion-completion-{suffix}");
    let name = format!("suggestion_completion_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(enabled_store(&client, &name).await);
    let (_, agent) = seed(
        &store,
        &suffix,
        "Compare SQLite for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    let model = AbortingModel {
        admin: admin.clone(),
        app,
        failures,
        unknown,
        calls: Default::default(),
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        SuggestionGenerationService::new(store.clone()).run_next(&model),
    )
    .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .expect("disable app-scoped failpoint before assertions");
    let reading = read(&router(store.clone()), &agent, "owner").await.1;
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let second = SuggestionGenerationService::new(store)
        .run_next(&model)
        .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
    let result = result.expect("bounded completion retry");
    if failures == 1 {
        assert!(result.unwrap());
        assert_eq!(reading["status"], "ready");
        assert_eq!(
            reading["set"]["items"][0]["text"],
            "Explain SQLite backup options."
        );
        assert_eq!(reading["observation"]["usage"]["input_tokens"], 23);
        assert_eq!(row.get_i64("suggestion_completion_fence").unwrap(), 1);
    } else {
        assert!(
            result.is_err(),
            "exhausted or unknown result must stay unavailable"
        );
        assert_eq!(reading["status"], "running");
        assert!(!row.contains_key("suggestion_completion_fence"));
    }
    assert!(!second.unwrap());
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
}

#[path = "completion_contention_cases.rs"]
mod cases;

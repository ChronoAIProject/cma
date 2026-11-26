use super::*;
use layer2_core::AgentId;

pub(super) fn after_abort(
    app: String,
    database: String,
    agent: AgentId,
    trace: Trace,
    fault: AbortFault,
) -> Arc<dyn Fn() + Send + Sync> {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    Arc::new(move || {
        let uri = uri.clone();
        let app = app.clone();
        let database = database.clone();
        let agent = agent.clone();
        let mut replacement = acquired_marker(&trace.lock().unwrap());
        replacement.insert("token", "replacement-marker-token");
        // The real abort has completed, releasing transaction locks. An
        // independent runtime models replacement without inheriting authority.
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
                .block_on(async move {
                    tokio::time::timeout(std::time::Duration::from_secs(8), async move {
                    let admin = Client::with_uri_str(uri).await.unwrap();
                    if matches!(fault, AbortFault::Replacement) {
                        let result = admin.database(&database).collection::<Document>("agents")
                            .update_one(doc! {"_id":agent.as_str()}, doc! {"$set":{FIELD:replacement}})
                            .write_concern(mongodb::options::WriteConcern::majority())
                            .await.unwrap();
                        assert_eq!(result.matched_count, 1);
                    } else {
                        let mut data = doc! {"appName":app,"failCommands":["update"]};
                        if matches!(fault, AbortFault::CleanupUncertain) {
                            data.insert("writeConcernError",doc! {"code":64_i32,
                                "codeName":"WriteConcernFailed","errmsg":"cleanup acknowledgment unavailable"});
                        } else {
                            data.insert("errorCode", 1_i32);
                        }
                        admin.database("admin").run_command(doc! {
                            "configureFailPoint":"failCommand", "mode":{"times":1}, "data":data,
                        }).await.unwrap();
                    }
                    admin.shutdown().await;
                    }).await.expect("independent cleanup barrier must finish promptly");
                });
        }).join().unwrap();
    })
}

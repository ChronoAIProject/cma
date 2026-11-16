use super::*;
use axum::http::StatusCode;
use public::request;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn personal_agent_list_counts_driver_retries_and_refuses_partial_reads() {
    let f = fixture::Fixture::new(201, false).await;
    let app = f.app(true, false);
    let admin = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
        .await
        .unwrap();
    for (path, provenance) in [
        ("/api/v1/agents", false),
        ("/api/v1/agents/provenance?limit=200", true),
    ] {
        let expected = request(&app, path).await;
        assert_eq!(expected.0, StatusCode::OK);
        for times in [1, 2] {
            admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint":"failCommand", "mode":{"times":times},
                    "data":{"failCommands":["aggregate"],"appName":&f.app_name,"errorCode":91_i32},
                })
                .await
                .unwrap();
            f.meter.take();
            let result = request(&app, path).await;
            let stats = f.meter.take();
            admin
                .database("admin")
                .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
                .await
                .unwrap();
            println!("AGENT_LIST_RETRY path={path} injected={times} stats={stats:?}");
            assert_eq!(stats.failed, times as usize);
            if times == 1 {
                assert_eq!(result, expected);
                assert_eq!(
                    stats.commands.get("agents/aggregate"),
                    Some(&if provenance { 4 } else { 2 })
                );
            } else {
                assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE);
                assert!(result.1.get("items").is_none());
                assert_eq!(stats.commands.get("agents/aggregate"), Some(&2));
            }
            assert!(stats.count() <= 10);
        }
        assert_eq!(request(&app, path).await, expected);
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_counts_byte_limited_getmore_without_exposing_private_sources() {
    let f = fixture::Fixture::new(201, false).await;
    // Retained sources are larger than their public list projection. Preserve
    // and decode the complete originals, and exercise the driver's byte boundary.
    f.store
        .agents
        .update_many(
            doc! {},
            doc! {"$set":{"record.profile.content.system_prompt":"p".repeat(32*1024)}},
        )
        .await
        .unwrap();
    let message = layer2_core::sandbox::FirstMessageRecord::new(
        "large-list-message",
        "m".repeat(64 * 1024),
        NOW,
    )
    .unwrap();
    f.store.agents.update_many(doc!{}, doc!{"$set":{"record.conversation.first_message":crate::store::sandbox_mongo::record_bson(&message).unwrap()}}).await.unwrap();
    let expected = request(&f.app(false, false), "/api/v1/agents").await;
    assert_eq!(expected.0, StatusCode::OK);
    let app = f.app(true, false);
    f.meter.take();
    assert_eq!(request(&app, "/api/v1/agents").await, expected);
    let stats = f.meter.take();
    println!("AGENT_LIST_LARGE limit=200 stats={stats:?}");
    assert!(stats.commands.get("agents/getMore").copied().unwrap_or(0) >= 2);
    assert!(
        stats.count() <= 10,
        "byte-limited pages still include all getMore commands"
    );
    assert!(expected.1.to_string().len() < 512 * 1024);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_preserves_sources_near_the_bson_document_ceiling() {
    let f = fixture::Fixture::new(1, false).await;
    let mut source = f.store.agents.find_one(doc! {}).await.unwrap().unwrap();
    let target_bytes = 16 * 1024 * 1024 - 32;
    source
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("profile")
        .unwrap()
        .get_document_mut("content")
        .unwrap()
        .insert("system_prompt", "");
    let padding = target_bytes - source.to_vec().unwrap().len();
    source
        .get_document_mut("record")
        .unwrap()
        .get_document_mut("profile")
        .unwrap()
        .get_document_mut("content")
        .unwrap()
        .insert("system_prompt", "p".repeat(padding));
    assert_eq!(source.to_vec().unwrap().len(), target_bytes);
    f.store.agents.replace_one(doc! {}, source).await.unwrap();
    let expected = request(&f.app(false, false), "/api/v1/agents").await;
    assert_eq!(expected.0, StatusCode::OK);
    f.meter.take();
    assert_eq!(
        request(&f.app(true, false), "/api/v1/agents").await,
        expected
    );
    let stats = f.meter.take();
    println!("AGENT_LIST_BSON_CEILING source_bytes={target_bytes} stats={stats:?}");
    assert_eq!(stats.failed, 0);
    assert!(stats.count() <= 10, "near-limit source getMore must count");
    assert!(expected.1.to_string().len() < 16 * 1024);
    f.close().await;
}

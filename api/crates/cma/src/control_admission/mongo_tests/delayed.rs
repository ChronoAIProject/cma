use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI disposable replica set; run serially"]
async fn delayed_mongo_quota_keeps_credits_after_http_timeout_and_disconnect() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    for disconnect in [false, true] {
        let name = format!("cma_control_delay_{}", uuid::Uuid::new_v4().simple());
        let replica = replica(&uri, &name).await;
        admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand",
            "mode":{"times":1}, "data":{"appName":&name, "failCommands":["findAndModify"],
                "blockConnection":true, "blockTimeMS":3500}})
            .await
            .unwrap();
        let request = client
            .post(format!("{}/api/v1/agents/delayed/stop", replica.address))
            .header("x-fixture-subject", "slow");
        let observer = tokio::spawn(async move { request.send().await.unwrap() });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !replica.pool.lock().unwrap().held.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if disconnect {
            observer.abort();
            assert!(observer.await.unwrap_err().is_cancelled());
        } else {
            let response = observer.await.unwrap();
            assert_eq!(response.status(), 503);
            assert_eq!(
                response.json::<serde_json::Value>().await.unwrap()["code"],
                "request_budget_unavailable"
            );
        }
        assert!(matches!(
            replica
                .policy
                .resources()
                .try_stage("slow", "other", Class::Stop, 1),
            Err(Refusal::Caller(_))
        ));
        // A blocked quota command owns only its local credit. Other work still
        // executes on this exact client/pool, including the reserved Stop slot.
        assert_eq!(
            client
                .get(format!("{}/sentinel", replica.address))
                .header("x-fixture-subject", "reader")
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        assert_eq!(
            client
                .post(format!("{}/api/v1/agents/sentinel/stop", replica.address))
                .header("x-fixture-subject", "sentinel")
                .send()
                .await
                .unwrap()
                .status(),
            202
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if replica
                    .policy
                    .resources()
                    .try_stage("slow", "other", Class::Stop, 1)
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
            .await
            .unwrap();
        assert_eq!(
            replica.state.accepted[0].load(Ordering::SeqCst),
            1,
            "timed-out quota must not invoke the control handler later"
        );
        let tasks = std::mem::take(&mut *replica.state.deferred.lock().unwrap());
        for task in tasks {
            task.await.unwrap();
        }
        settle_pool(&replica.pool).await;
        {
            let pool = replica.pool.lock().unwrap();
            assert!(pool.held.is_empty());
            println!(
                "CONTROL_DELAY {}",
                serde_json::json!({"disconnect":disconnect,"peak_checkout":pool.peak,"connection_seconds":pool.connection_seconds,"retained_until_joined":true})
            );
        }
        admin.database(&name).drop().await.unwrap();
    }
}

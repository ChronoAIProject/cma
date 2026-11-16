use super::*;
use axum::http::StatusCode;
use public::request;
use std::time::Instant;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_public_parity_and_complete_command_budget() {
    let mut over_budget = Vec::new();
    for distinct in [false, true] {
        let f = fixture::Fixture::new(201, distinct).await;
        for limit in [20, 200] {
            let path = format!("/api/v1/agents?limit={limit}");
            let legacy = request(&f.app(false, false), &path).await;
            assert_eq!(legacy.0, StatusCode::OK, "{legacy:?}");
            assert_eq!(legacy.1["items"].as_array().unwrap().len(), limit);
            assert_eq!(legacy.1["next"], format!("agt_list_{:04}", limit - 1));
            for joined in [false, true] {
                let app = f.app(true, joined);
                for cold in [true, false] {
                    if cold {
                        f.cold().await;
                    } else {
                        f.meter.take();
                    }
                    let start = Instant::now();
                    assert_eq!(request(&app, &path).await, legacy);
                    let elapsed = start.elapsed();
                    let stats = f.meter.take();
                    println!("AGENT_LIST limit={limit} distinct={distinct} joined_auth={joined} cold={cold} elapsed_us={} stats={stats:?}", elapsed.as_micros());
                    assert_eq!(stats.commands.get("agents/aggregate"), Some(&1));
                    assert_eq!(stats.commands.get("activity/insert"), Some(&1));
                    assert_eq!(stats.failed, 0);
                    assert_eq!(stats.count(), stats.checkouts);
                    if stats.count() > 10 {
                        over_budget.push((limit, distinct, joined, cold, stats.count()));
                    }
                }
            }
            let cursor = legacy.1["next"].as_str().unwrap();
            let path = format!("{path}&cursor={cursor}");
            assert_eq!(
                request(&f.app(false, false), &path).await,
                request(&f.app(true, false), &path).await
            );
        }
        f.close().await;
    }
    assert!(
        over_budget.is_empty(),
        "full requests exceeding ten commands: {over_budget:?}"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_provenance_inventory_preserves_rows_within_complete_budget() {
    let mut over_budget = Vec::new();
    for distinct in [false, true] {
        let f = fixture::Fixture::new(201, distinct).await;
        let old = f.app(false, false);
        let new = f.app(true, false);
        for limit in [20, 200] {
            let path = format!("/api/v1/agents/provenance?limit={limit}");
            let expected = request(&old, &path).await;
            assert_eq!(expected.0, StatusCode::OK, "{expected:?}");
            assert_eq!(expected.1["items"].as_array().unwrap().len(), limit);
            assert_eq!(expected.1["next"], format!("agt_list_{:04}", limit - 1));
            f.meter.take();
            assert_eq!(request(&new, &path).await, expected);
            let stats = f.meter.take();
            println!("AGENT_PROVENANCE limit={limit} distinct={distinct} stats={stats:?}");
            assert_eq!(stats.failed, 0);
            if stats.count() > 10 {
                over_budget.push((limit, distinct, stats.count()));
            }
        }
        f.close().await;
    }
    assert!(
        over_budget.is_empty(),
        "provenance pages exceeding ten commands: {over_budget:?}"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_filters_lookahead_and_rollback_preserve_public_rows() {
    let f = fixture::Fixture::new(21, true).await;
    let old = f.app(false, false);
    let new = f.app(true, false);
    for path in [
        "/api/v1/agents?limit=20",
        "/api/v1/agents?limit=20&cursor=agt_list_0019",
        "/api/v1/agents?sandbox=sbx_list_0002",
        "/api/v1/agents?workspace=wks_list_0003",
        "/api/v1/agents?status=starting",
        "/api/v1/agents?status=ready",
        "/api/v1/agents?cursor=zzz",
        "/api/v1/agents?limit=0",
        "/api/v1/agents?limit=201",
        "/api/v1/agents?cursor=",
        "/api/v1/agents?organization=org",
    ] {
        assert_eq!(
            request(&old, path).await,
            request(&new, path).await,
            "{path}"
        );
    }
    // A broken parent behind the lookahead is not revealed by the first page.
    f.store
        .workspaces
        .delete_one(doc! {"_id":"wks_list_0020"})
        .await
        .unwrap();
    let path = "/api/v1/agents?limit=20";
    let first = request(&old, path).await;
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(request(&new, path).await, first);
    let next = "/api/v1/agents?limit=20&cursor=agt_list_0019";
    assert_eq!(request(&old, next).await, request(&new, next).await);
    assert!(!request(&new, next).await.0.is_success());
    // A legacy writer and a new writer share unchanged authoritative records.
    let id = AgentId("agt_list_0000".into());
    let mut agent = f.legacy.get_agent(&id).await.unwrap().unwrap();
    agent.updated_at = MINUTE.into();
    agent.revision += 1;
    f.legacy
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    assert_eq!(request(&new, path).await, request(&old, path).await);
    agent.updated_at = TWO_MINUTES.into();
    agent.revision += 1;
    f.store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    assert_eq!(request(&new, path).await, request(&old, path).await);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_refuses_missing_stale_or_malformed_authority_like_legacy() {
    let f = fixture::Fixture::new(2, true).await;
    let old = f.app(false, false);
    let new = f.app(true, false);
    let path = "/api/v1/agents";
    for (collection, update) in [
        ("agents", doc! {"$set":{"record.owner":"other"}}),
        (
            "agents",
            doc! {"$set":{"record.profile.content.system_prompt":false}},
        ),
        ("worktrees", doc! {"$set":{"record.agent_id":"other"}}),
        ("workspaces", doc! {"$set":{"owner":"other"}}),
        ("sandboxes", doc! {"$set":{"record.created_at":false}}),
        (
            "sandboxes",
            doc! {"$set":{"document_schema_version":999_i64}},
        ),
        ("sandboxes", doc! {"$unset":{"record":""}}),
    ] {
        let collection = f
            .client
            .database(&f.name)
            .collection::<Document>(collection);
        let original = collection
            .find_one(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .unwrap();
        let filter = doc! {"_id": original.get("_id").unwrap().clone()};
        collection.update_one(filter.clone(), update).await.unwrap();
        let expected = request(&old, path).await;
        assert!(!expected.0.is_success(), "{}", collection.name());
        assert_eq!(request(&new, path).await, expected);
        collection.replace_one(filter, original).await.unwrap();
    }
    for collection in [&f.store.worktrees, &f.store.workspaces, &f.store.sandboxes] {
        let original = collection
            .find_one(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .unwrap();
        collection
            .delete_one(doc! {"_id":original.get("_id").unwrap().clone()})
            .await
            .unwrap();
        let expected = request(&old, path).await;
        assert!(!expected.0.is_success());
        assert_eq!(request(&new, path).await, expected);
        collection.insert_one(original).await.unwrap();
    }
    let target = ResourceTarget::Agent {
        agent_id: AgentId("agt_list_0000".into()),
    };
    let snapshot = f.store.resource_access_snapshot(&target).await.unwrap();
    f.store
        .mutate_share(mutation(&snapshot, "personal-list", ShareMode::View))
        .await
        .unwrap();
    assert_eq!(request(&old, path).await, request(&new, path).await);
    for change in [
        doc! {"$set":{"record.revision":99_i64}},
        doc! {"$set":{"owner_kind":"organization"}},
    ] {
        let original = f
            .store
            .resource_policies
            .find_one(doc! {})
            .await
            .unwrap()
            .unwrap();
        f.store
            .resource_policies
            .update_one(doc! {}, change)
            .await
            .unwrap();
        let expected = request(&old, path).await;
        assert!(!expected.0.is_success());
        assert_eq!(request(&new, path).await, expected);
        f.store
            .resource_policies
            .replace_one(doc! {}, original)
            .await
            .unwrap();
    }
    let share = f
        .store
        .resource_shares
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    for field in ["sandbox_id", "selector_kind", "selector_id", "revoked"] {
        let mut values = Document::new();
        values.insert(field, vec![share.get(field).unwrap().clone()]);
        f.store
            .resource_shares
            .update_one(doc! {}, doc! {"$set":values})
            .await
            .unwrap();
        let expected = request(&old, path).await;
        assert!(
            !expected.0.is_success(),
            "legacy must detect selected malformed {field}"
        );
        assert_eq!(request(&new, path).await, expected, "{field}");
        f.store
            .resource_shares
            .replace_one(doc! {}, share.clone())
            .await
            .unwrap();
    }
    f.store
        .resource_policies
        .delete_many(doc! {})
        .await
        .unwrap();
    let expected = request(&old, path).await;
    assert!(
        !expected.0.is_success(),
        "shares without policy must be refused"
    );
    assert_eq!(request(&new, path).await, expected);
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_agent_list_one_hundred_concurrent_public_readers_meet_budgets() {
    let mut percentiles = Vec::new();
    for distinct in [false, true] {
        let f = fixture::Fixture::readers(100, 201, distinct).await;
        let app = f.app(true, false);
        let mut expected_pages = Vec::new();
        for reader in 0..100 {
            let cookie = identity::cookie(reader);
            let expected = public::request_cookie(&app, "/api/v1/agents", &cookie).await;
            assert_eq!(expected.0, StatusCode::OK, "reader {reader}");
            assert_eq!(expected.1["items"].as_array().unwrap().len(), 200);
            expected_pages.push((cookie, expected));
        }
        f.meter.take();
        let barrier = Arc::new(tokio::sync::Barrier::new(101));
        let mut readers = tokio::task::JoinSet::new();
        for (cookie, expected) in expected_pages {
            let (app, barrier) = (app.clone(), barrier.clone());
            readers.spawn(async move {
                barrier.wait().await;
                let start = Instant::now();
                let result = public::request_cookie(&app, "/api/v1/agents", &cookie).await;
                assert_eq!(
                    result.0,
                    expected.0,
                    "{}",
                    result.1.get("code").unwrap_or(&serde_json::Value::Null)
                );
                assert!(
                    result.1 == expected.1,
                    "concurrent page differs from its owner's reference"
                );
                start.elapsed().as_micros()
            });
        }
        barrier.wait().await;
        let mut times = Vec::new();
        while let Some(result) = readers.join_next().await {
            times.push(result.unwrap());
        }
        times.sort_unstable();
        let stats = f.meter.take();
        println!("AGENT_LIST_CONCURRENT readers=100 limit=200 distinct={distinct} p95_us={} p99_us={} stats={stats:?}", times[94], times[98]);
        // Every successful request requires exactly this complete set of commands;
        // any retry or getMore makes the total exceed the asserted eight per read.
        assert_eq!(
            stats.commands,
            std::collections::BTreeMap::from([
                ("agents/aggregate".into(), 100),
                ("agents/find".into(), 100),
                ("auth_browser_logins/find".into(), 200),
                ("auth_subjects/find".into(), 200),
                ("request_rate_limits/findAndModify".into(), 100),
                ("activity/insert".into(), 100),
            ])
        );
        assert_eq!(stats.failed, 0);
        percentiles.push((distinct, times[94], times[98]));
        f.close().await;
    }
    assert!(
        percentiles.iter().all(|(_, p95, _)| *p95 <= 500_000),
        "Agent200 p95 must be at most 500 ms: {percentiles:?}"
    );
    assert!(
        percentiles.iter().all(|(_, _, p99)| *p99 < 2_000_000),
        "Agent200 p99 must be below 2 s: {percentiles:?}"
    );
}

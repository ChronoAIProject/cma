//! Real authentication, admission, organization policy and final page fences.
use super::*;
use axum::http::StatusCode;
use std::time::Instant;

#[path = "organization/fences.rs"]
mod fences;

const PATH: &str = "/api/v1/agents?organization=owner&limit=200";
pub(super) async fn owned(rows: usize, distinct: bool) -> fixture::Fixture {
    owned_with_command_observer(rows, distinct, true).await
}
async fn owned_with_command_observer(
    rows: usize,
    distinct: bool,
    observe_commands: bool,
) -> fixture::Fixture {
    let f = fixture::Fixture::new_with_command_observer(rows, distinct, observe_commands).await;
    f.identity.add_reader(1).await;
    let mut policies = Vec::new();
    let mut cursor = f.store.sandboxes.find(doc! {}).await.unwrap();
    while cursor.advance().await.unwrap() {
        let sandbox =
            crate::store::sandboxes::read_sandbox(&cursor.deserialize_current().unwrap()).unwrap();
        let policy = ResourcePolicyRecord {
            sandbox_id: sandbox.id,
            sandbox_created_at: sandbox.created_at,
            owner: ResourceOwner::Organization {
                subject: sandbox.owner.clone(),
            },
            revision: 1,
            organization_evidence: Some(OrganizationOwnershipEvidence {
                issuer: "https://identity.example.test".into(),
                organization: sandbox.owner,
                admitted_by: ResourceActor::Human {
                    subject: SubjectId::new("fixture-admin"),
                },
                verified_at: NOW.into(),
            }),
        };
        policies.push(crate::store::resource_access::records::policy_document(&policy).unwrap());
    }
    f.store
        .resource_policies
        .insert_many(policies)
        .await
        .unwrap();
    f
}
pub(super) fn app(f: &fixture::Fixture, batched: bool) -> axum::Router {
    let store = if batched { &f.store } else { &f.legacy };
    f.app_with_authority(store.clone(), store.clone(), false, f.identity.auth(false))
}
async fn request(app: &axum::Router, path: &str) -> (StatusCode, serde_json::Value) {
    public::request_cookie(app, path, &identity::cookie(1)).await
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn organization_batch_driver_failure_refuses_without_serial_fallback() {
    let f = owned(2, false).await;
    let app = app(&f, true);
    assert_eq!(request(&app, PATH).await.0, StatusCode::OK);
    let admin = f.client.database("admin");
    admin
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1},
            "data":{"failCommands":["aggregate"], "errorCode":2, "appName":&f.app_name}
        })
        .await
        .unwrap();
    f.meter.take();
    let result = request(&app, PATH).await;
    let stats = f.meter.take();
    admin
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE, "{result:?}");
    assert!(result.1.get("items").is_none());
    assert_eq!(stats.failed, 1);
    assert_eq!(
        stats
            .commands
            .iter()
            .filter(|(key, _)| key.ends_with("/aggregate"))
            .map(|(_, count)| count)
            .sum::<usize>(),
        1,
        "failed discovery must not fall back: {stats:?}"
    );
    assert_eq!(request(&app, PATH).await.0, StatusCode::OK);
    f.close().await;
}
#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn organization_public_pages_preserve_redaction_cursors_and_complete_command_budget() {
    for distinct in [false, true] {
        let f = owned(201, distinct).await;
        for limit in [20, 200] {
            let path = format!("/api/v1/agents?organization=owner&limit={limit}");
            let old = request(&app(&f, false), &path).await;
            assert_eq!(old.0, StatusCode::OK, "{old:?}");
            assert_eq!(old.1["items"].as_array().unwrap().len(), limit);
            assert!(old.1["next"].is_string());
            f.cold().await;
            let new = request(&app(&f, true), &path).await;
            let stats = f.meter.take();
            assert_eq!(new, old);
            for row in new.1["items"].as_array().unwrap() {
                assert!(row.get("sandbox").is_none());
                assert!(row.get("workspace").is_none());
                assert!(row.get("worktree").is_none());
            }
            println!(
                "ORGANIZATION_PAGE limit={limit} distinct={distinct} commands={} map={:?}",
                stats.count(),
                stats.commands
            );
            assert!(stats.count() <= 20, "{stats:?}");
            assert_eq!(stats.failed, 0);
            assert_eq!(stats.count(), stats.checkouts);
            assert_eq!(stats.commands.get("agents/aggregate"), Some(&2));
            assert!(stats.commands.contains_key("auth_subjects/find"));
            if limit == 200 {
                assert!(
                    stats.commands.keys().any(|key| key.ends_with("/getMore")),
                    "count discovery getMore: {stats:?}"
                );
            }
            let next = format!("{path}&cursor={}", new.1["next"].as_str().unwrap());
            assert_eq!(
                request(&app(&f, true), &next).await,
                request(&app(&f, false), &next).await
            );
        }
        let denied = "/api/v1/agents?organization=other-org";
        assert_eq!(
            request(&app(&f, true), denied).await,
            request(&app(&f, false), denied).await
        );
        assert!(!request(&app(&f, true), denied).await.0.is_success());
        f.close().await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable MongoDB replica set and optimized quiet-host build"]
async fn organization_one_hundred_concurrent_public_readers_meet_budgets() {
    let mut percentiles = Vec::new();
    for distinct in [false, true] {
        // Match production observation: count actual server commands without
        // adding eager full-reply BSON decoding to every measured request.
        let f = owned_with_command_observer(201, distinct, false).await;
        for reader in 2..101 {
            f.identity.add_reader(reader).await;
        }
        let app = app(&f, true);
        let expected = request(&app, PATH).await;
        assert_eq!(expected.0, StatusCode::OK, "{expected:?}");
        assert_eq!(expected.1["items"].as_array().unwrap().len(), 200);
        for reader in 1..101 {
            assert_eq!(
                public::request_cookie(&app, PATH, &identity::cookie(reader)).await,
                expected
            );
        }
        profile::server(&f, true).await;
        f.meter.take();
        let barrier = Arc::new(tokio::sync::Barrier::new(101));
        let mut readers = tokio::task::JoinSet::new();
        for reader in 1..101 {
            let (app, barrier, expected) = (app.clone(), barrier.clone(), expected.clone());
            readers.spawn(async move {
                barrier.wait().await;
                let start = Instant::now();
                assert_eq!(
                    public::request_cookie(&app, PATH, &identity::cookie(reader)).await,
                    expected
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
        let mut stats = f.meter.take();
        profile::server(&f, false).await;
        let server = profile::server_report(&f).await;
        stats.commands = server.commands;
        stats.failed = server.failed;
        println!("ORGANIZATION_CONCURRENT readers=100 limit=200 distinct={distinct} p95_us={} p99_us={} commands={} map={:?}", times[94], times[98], stats.count(), stats.commands);
        assert!(stats.count() <= 2000, "{stats:?}");
        assert_eq!(stats.count(), stats.checkouts);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.commands.get("agents/aggregate"), Some(&200));
        assert_eq!(
            stats.commands,
            std::collections::BTreeMap::from([
                ("activity/insert".into(), 100),
                ("agents/aggregate".into(), 200),
                ("auth_browser_logins/find".into(), 200),
                ("auth_subjects/find".into(), 200),
                ("request_rate_limits/findAndModify".into(), 100),
                ("resource_policies/aggregate".into(), 100),
                ("resource_policies/getMore".into(), 100),
                ("resource_shares/aggregate".into(), 200),
            ])
        );
        percentiles.push((distinct, times[94], times[98]));
        f.close().await;
    }
    assert!(
        percentiles.iter().all(|(_, p95, _)| *p95 < 1_000_000),
        "p95 target: {percentiles:?}"
    );
    assert!(
        percentiles.iter().all(|(_, _, p99)| *p99 < 2_000_000),
        "p99 target: {percentiles:?}"
    );
}

//! TC-107. Exercise the public list routes through authentication and MongoDB.
use super::*;
use axum::{http::StatusCode, Router};
use futures_util::future::join_all;
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};
use tokio::sync::Barrier;

const AGENTS: &str = "/api/v1/agents?limit=200";
const ORGANIZATIONS: &str = "/api/v1/organizations";
const READERS: usize = 100;

fn assert_budget(stats: &meter::Stats, max_commands: usize) {
    assert!((1..=max_commands).contains(&stats.count()), "{stats:?}");
    assert_eq!(stats.count(), stats.checkouts, "{stats:?}");
    assert_eq!(stats.failed, 0, "{stats:?}");
    assert_eq!(stats.checkout_failed, 0, "{stats:?}");
}

fn assert_agent_ids(body: &Value, expected: std::ops::Range<usize>) {
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), expected.len());
    for (item, index) in items.iter().zip(expected) {
        assert_eq!(item["id"], format!("agt_list_{index:04}"));
    }
}

async fn seed_history(fixture: &fixture::Fixture, sequences: std::ops::RangeInclusive<u64>) {
    let mut agents = fixture.store.agents.find(doc! {}).await.unwrap();
    while agents.advance().await.unwrap() {
        let agent =
            crate::store::agents::read_agent(&agents.deserialize_current().unwrap()).unwrap();
        let entries: Vec<_> = sequences
            .clone()
            .map(|seq| {
                log_document(&AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: NOW.into(),
                    kind: AgentLogKind::WorkStepCompleted {
                        step: AgentWorkStep::AddWorktree,
                    },
                })
                .unwrap()
            })
            .collect();
        fixture.store.agent_log.insert_many(entries).await.unwrap();
    }
    let last_seq = i64::try_from(*sequences.end()).unwrap();
    fixture
        .store
        .agents
        .update_many(
            doc! {},
            doc! {"$set":{"last_seq":last_seq,"record.conversation.last_seq":last_seq}},
        )
        .await
        .unwrap();
}

async fn concurrent_reads(
    app: &Router,
    cookie: &str,
    requests: &[(&str, &Value)],
    fixture: &fixture::Fixture,
    budget: usize,
) {
    fixture.meter.take();
    let cases: Vec<_> = requests
        .iter()
        .map(|(path, expected)| (path.to_string(), Arc::new((**expected).clone())))
        .collect();
    let admission = Arc::new(Barrier::new(READERS + 1));
    let readers: Vec<_> = (0..READERS)
        .map(|index| {
            let (path, expected) = cases[index % cases.len()].clone();
            let app = app.clone();
            let cookie = cookie.to_owned();
            let admission = admission.clone();
            tokio::spawn(async move {
                admission.wait().await;
                let start = Instant::now();
                let (status, body) = public::request_cookie(&app, &path, &cookie).await;
                let micros = start.elapsed().as_micros();
                assert_eq!(status, StatusCode::OK, "{path}");
                assert_eq!(&body, expected.as_ref(), "{path} reader {index}");
                micros
            })
        })
        .collect();
    // Release the same reader cohort across the fixture's Tokio workers.
    admission.wait().await;
    let mut elapsed: Vec<_> = join_all(readers)
        .await
        .into_iter()
        .map(|result| result.expect("reader task completed"))
        .collect();
    let stats = fixture.meter.take();
    assert_budget(&stats, READERS * budget);
    elapsed.sort_unstable();
    println!(
        "TC-107 {} readers={READERS} p95_us={} p99_us={} commands={}",
        requests[0].0,
        elapsed[94],
        elapsed[98],
        stats.count()
    );
    // TC-107 scores latency with an optimized binary. The ordinary debug
    // partitions still enforce the complete request, query and parity checks.
    if !cfg!(debug_assertions) {
        let p95 = elapsed[94];
        let p99 = elapsed[98];
        if requests[0].0 == ORGANIZATIONS {
            assert!(p95 < 1_000_000, "organization p95 {p95} us");
            assert!(p99 < 2_000_000, "organization p99 {p99} us");
        } else {
            assert!(p95 <= 500_000, "Agent p95 {p95} us");
            assert!(p99 <= 2_000_000, "Agent p99 {p99} us");
        }
    }
    assert!(stats
        .commands
        .keys()
        .all(|key| !key.starts_with("agent_log/")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable MongoDB replica set"]
async fn tc107_agent_pages_preserve_order_and_query_budget_after_history_growth() {
    for distinct in [false, true] {
        let fixture = fixture::Fixture::new(400, distinct).await;
        seed_history(&fixture, 1..=1).await;
        let app = fixture.app(true, false);
        let filtered = format!("{AGENTS}&status=starting");
        let mut before = None;
        for history_size in [1, 10] {
            if history_size == 10 {
                seed_history(&fixture, 2..=10).await;
            }
            fixture.cold().await;
            profile::server(&fixture, true).await;
            fixture.meter.take();
            let (status, first) = public::request(&app, AGENTS).await;
            assert_eq!(status, StatusCode::OK, "{first}");
            assert_agent_ids(&first, 0..200);
            assert_eq!(first["next"], "agt_list_0199");
            let first_stats = fixture.meter.take();
            assert_budget(&first_stats, 10);
            let tail = format!("{AGENTS}&cursor={}", first["next"].as_str().unwrap());
            let (status, last) = public::request(&app, &tail).await;
            assert_eq!(status, StatusCode::OK, "{last}");
            assert_agent_ids(&last, 200..400);
            assert!(last["next"].is_null());
            let last_stats = fixture.meter.take();
            assert_budget(&last_stats, 10);
            assert_eq!(
                public::request(&app, &filtered).await,
                (StatusCode::OK, first.clone())
            );
            assert_budget(&fixture.meter.take(), 10);

            profile::server(&fixture, false).await;
            let database = fixture.client.database(&fixture.name);
            let profiles = database.collection::<Document>("system.profile");
            let query_collections = [
                "agents",
                "agent_log",
                "sandboxes",
                "workspaces",
                "worktrees",
            ]
            .map(|name| format!("{}.{name}", fixture.name));
            let mut rows = profiles
                .find(doc! {
                    "appName": &fixture.app_name,
                    "ns": {"$in": query_collections.to_vec()},
                    "docsExamined": {"$exists": true},
                })
                .await
                .unwrap();
            let mut documents_examined = 0;
            while rows.advance().await.unwrap() {
                let row: Document = rows.deserialize_current().unwrap();
                documents_examined += row
                    .get_i64("docsExamined")
                    .or_else(|_| row.get_i32("docsExamined").map(i64::from))
                    .unwrap();
            }
            assert!(documents_examined > 0);
            profiles.drop().await.unwrap();
            let measured = (
                first_stats.commands,
                last_stats.commands,
                documents_examined,
            );
            println!(
                "TC-107 agent history={history_size} distinct={distinct} commands={measured:?}"
            );
            if let Some(before) = &before {
                assert_eq!(before, &measured);
            } else {
                before = Some(measured);
            }
            concurrent_reads(
                &app,
                &identity::cookie(0),
                &[(AGENTS, &first), (&tail, &last), (&filtered, &first)],
                &fixture,
                10,
            )
            .await;
        }
        fixture.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable MongoDB replica set"]
async fn tc107_organization_list_and_agent_cursor_reflect_membership_revocation() {
    let fixture = organization::owned(201, true).await;
    let app = organization::app(&fixture, true);
    let mut choices: Vec<_> = (0..199)
        .rev()
        .map(|index| {
            json!({
                "id": format!("org-{index:03}"),
                "your_role": if index % 2 == 0 { "member" } else { "viewer" },
            })
        })
        .collect();
    choices.push(json!({"id":"owner", "your_role":"viewer"}));
    fixture.identity.set_organization_choices(choices.clone());
    let reader = identity::cookie(1);

    fixture.cold().await;
    let (status, before) = public::request_cookie(&app, ORGANIZATIONS, &reader).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    assert_budget(&fixture.meter.take(), 20);
    let items = before["items"].as_array().unwrap();
    assert_eq!(items.len(), 200);
    for (item, expected) in items
        .iter()
        .zip(choices[..199].iter().rev().chain(choices.last()))
    {
        assert_eq!(item["id"], expected["id"]);
        assert_eq!(item["role"], expected["your_role"]);
    }

    concurrent_reads(&app, &reader, &[(ORGANIZATIONS, &before)], &fixture, 20).await;
    let scoped = format!("{AGENTS}&organization=owner");
    let (status, page) = public::request_cookie(&app, &scoped, &reader).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_agent_ids(&page, 0..200);
    let continuation = format!("{scoped}&cursor={}", page["next"].as_str().unwrap());
    let (status, page) = public::request_cookie(&app, &continuation, &reader).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_agent_ids(&page, 200..201);
    assert!(page["next"].is_null());

    choices.pop();
    fixture.identity.set_organization_choices(choices);
    fixture.meter.take();
    let (status, after) = public::request_cookie(&app, ORGANIZATIONS, &reader).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    assert_eq!(after["items"], json!(items[..199]));
    assert_budget(&fixture.meter.take(), 20);
    for path in [&scoped, &continuation] {
        let (status, body) = public::request_cookie(&app, path, &reader).await;
        assert!(
            matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{status}"
        );
        assert!(body.get("items").is_none());
    }

    fixture.close().await;
}

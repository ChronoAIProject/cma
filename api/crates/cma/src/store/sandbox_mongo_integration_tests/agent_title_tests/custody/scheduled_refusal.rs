use super::*;
use crate::sandbox::{scheduled_title_fixture, start_title_scheduler};
use crate::store::request_limits::MongoRequestLimits;
use axum::{extract::State, http::StatusCode, routing::post, Router};
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::SandboxStore;
use layer2_core::sandbox::{AGENT_TITLE_CLAIM_SECONDS, AGENT_TITLE_RETRY_SECONDS};
use mongodb::bson::DateTime;
use tokio::sync::{watch, Notify};

const SCAN_INTERVAL: Duration = Duration::from_secs(5);
const PROMPT: &str = "Summarize the release risks";

#[derive(Default)]
struct Refusal {
    calls: AtomicUsize,
    posted: Notify,
}

async fn refuse_title(State(refusal): State<Arc<Refusal>>) -> StatusCode {
    refusal.calls.fetch_add(1, Ordering::SeqCst);
    refusal.posted.notify_one();
    StatusCode::NOT_FOUND
}

async fn wait_until(store: &MongoSandboxStore, id: &AgentId, time: DateTime) {
    if let Ok(remaining) = time.to_system_time().duration_since(SystemTime::now()) {
        tokio::time::sleep(remaining).await;
    }
    tokio::time::timeout(SCAN_INTERVAL, async {
        loop {
            let row = store
                .agents
                .aggregate([
                    doc! {"$match": {"_id": id.as_str()}},
                    doc! {"$project": {"now": "$$NOW"}},
                ])
                .await
                .unwrap()
                .try_next()
                .await
                .unwrap()
                .unwrap();
            let now = row.get_datetime("now").unwrap();
            if *now >= time {
                break;
            }
            tokio::time::sleep(
                time.to_system_time()
                    .duration_since(now.to_system_time())
                    .unwrap(),
            )
            .await;
        }
    })
    .await
    .expect("Mongo must confirm the stored eligibility timestamp before starting the scheduler");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn scheduled_permanent_404_persists_prompt_fallback_without_redispatch() {
    let (client, name) = database("title_scheduled_refusal").await;
    let first = Arc::new(peer(&name, true).await);
    let second = Arc::new(peer(&name, true).await);
    let budget = Arc::new(
        MongoRequestLimits::open(&client.database(&name))
            .await
            .unwrap(),
    );
    let (sandbox, agent) = scheduled_title_fixture();
    let id = agent.id.clone();
    first.create_sandbox(sandbox.clone()).await.unwrap();
    first.create_agent(agent).await.unwrap();
    append_kinds(&first, &id, completed_turn(PROMPT, "Synthetic answer", 1)).await;

    let refusal = Arc::new(Refusal::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/api/v1/proxy/s/chrono-llm/responses", post(refuse_title))
        .with_state(refusal.clone());
    let provider = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (stop, shutdown) = watch::channel(false);
    let scheduler = start_title_scheduler(
        AgentTitleService::new(first.clone(), "initial-scheduler").unwrap(),
        first,
        &sandbox,
        budget.clone(),
        origin.clone(),
        shutdown,
    );
    tokio::time::timeout(Duration::from_secs(20), refusal.posted.notified())
        .await
        .expect("the initial scheduler must dispatch a title request");
    stop.send(true).unwrap();
    scheduler.await.unwrap();

    assert_eq!(refusal.calls.load(Ordering::SeqCst), 1);
    assert!(second.agent_title_effect(&id).await.unwrap().unknown());
    assert_eq!(second.agent_title(&id).await.unwrap(), None);
    let filter = doc! {"_id": id.as_str()};
    let original = second
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    let acquired_at = original.get_datetime("title_lease_acquired_at").unwrap();
    let expires_at = original.get_datetime("title_lease_expires_at").unwrap();
    let retry_at = original.get_datetime("title_next_attempt_at").unwrap();
    assert_eq!(
        expires_at.timestamp_millis() - acquired_at.timestamp_millis(),
        i64::try_from(AGENT_TITLE_CLAIM_SECONDS * 1000).unwrap()
    );
    assert_eq!(
        retry_at.timestamp_millis() - acquired_at.timestamp_millis(),
        i64::try_from(AGENT_TITLE_RETRY_SECONDS * 1000).unwrap()
    );

    // Use the stored timestamps so this exercises the real retry window.
    wait_until(&second, &id, *expires_at).await;
    let page = PageRequest::new(20, None).unwrap();
    assert!(second
        .agent_title_candidates(&page)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(second.agent_title(&id).await.unwrap(), None);
    wait_until(&second, &id, *retry_at).await;

    // A replacement scheduler scans immediately, with only durable state from
    // the first scheduler. Bound publication to one normal scan interval.
    let (stop, shutdown) = watch::channel(false);
    let scheduler = start_title_scheduler(
        AgentTitleService::new(second.clone(), "successor-scheduler").unwrap(),
        second.clone(),
        &sandbox,
        budget,
        origin,
        shutdown,
    );
    let remaining = (retry_at.to_system_time() + SCAN_INTERVAL)
        .duration_since(SystemTime::now())
        .expect("successor starts within one scan interval of retry eligibility");
    let title = tokio::time::timeout(remaining, async {
        loop {
            if let Some(title) = second.agent_title(&id).await.unwrap() {
                break title;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the first eligible scan must persist the prompt fallback");
    let publication_delay = SystemTime::now()
        .duration_since(retry_at.to_system_time())
        .unwrap();
    assert!(publication_delay <= SCAN_INTERVAL);
    println!("persisted prompt fallback after retry eligibility in {publication_delay:?}");
    stop.send(true).unwrap();
    scheduler.await.unwrap();

    assert_eq!(title.title.as_str(), PROMPT);
    assert_eq!(title.source, AgentTitleSource::Fallback);
    let settled = second.agents.find_one(filter).await.unwrap().unwrap();
    assert!(settled.get_bool("title_settled").unwrap());
    assert_eq!(settled.get_i64("title_attempt_count").unwrap(), 1);
    assert_eq!(
        settled.get("title_effect_custody"),
        original.get("title_effect_custody")
    );
    assert_eq!(
        second
            .get_agent(&id)
            .await
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some(PROMPT)
    );
    let events = second.agent_log(&id, 0, 20).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, AgentLogKind::TitleSet { .. }))
            .count(),
        1
    );
    assert!(second
        .agent_title_candidates(&page)
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(refusal.calls.load(Ordering::SeqCst), 1);

    provider.abort();
    assert!(provider.await.unwrap_err().is_cancelled());
    client.database(&name).drop().await.unwrap();
}

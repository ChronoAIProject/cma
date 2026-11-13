use super::allocated::request;
use super::history_batch::{event, observed_store};
use super::*;
use crate::config::report_views::{ViewPolicy, WarmScope};
use std::sync::atomic::Ordering;
use std::time::Instant;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_default_day_on_seven_day_history_measures_complete_snapshot_and_refresh() {
    let (original, commands) = observed_store().await;
    let store = original.with_view_policy(ViewPolicy {
        enabled: true,
        view_scopes: vec![WarmScope {
            principal: "operator".into(),
            admin: true,
            sandbox_id: None,
        }],
        ..Default::default()
    });
    let to = now() / 600 * 600;
    let from = to - 7 * 86_400;
    let mut states = Vec::new();
    const MEMBERS: usize = 421;
    for id in 0..MEMBERS {
        let mut s = state(id, from);
        s.last_observed_at = to;
        s.revision = 7 * 48;
        let docs = (0..7 * 48)
            .map(|n| {
                let mut e = event(&s, n, from + n * 1800);
                e.ended_at = e.started_at + 1800;
                e.quantity = e.allocation.usage(1800).unwrap();
                super::super::super::sandbox_usage::event_document(&e).unwrap()
            })
            .collect::<Vec<_>>();
        store
            .collection("sandbox_usage_events")
            .insert_many(docs)
            .await
            .unwrap();
        states.push(s);
    }
    seed(&store, &states).await;
    store
        .collection("sandbox_usage_events")
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"sandbox_id":1,"revision":1})
                .build(),
        )
        .await
        .unwrap();
    let path = "/api/v1/admin/usage/metrics/view?window=24h";
    let started = Instant::now();
    let (status, pending) = request(
        &store,
        "GET",
        path,
        "operator",
        true,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, 202);
    eprintln!(
        "retained_default admission_us={} status={status}",
        started.elapsed().as_micros()
    );
    let mut prior_id = pending["report"]["report_id"].as_str().unwrap().to_owned();
    for cycle in 0..2 {
        let lease = store
            .claim_report("report:0", "measurement-worker")
            .await
            .unwrap()
            .unwrap();
        if cycle == 0 {
            assert_eq!(lease.report.report_id, prior_id);
        }
        commands.event_reads.store(0, Ordering::SeqCst);
        let started = Instant::now();
        let generated =
            crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
                .await
                .unwrap();
        assert_eq!(
            generated.summary.as_ref().unwrap().sandbox_count,
            MEMBERS as u64
        );
        store.finish_report(&lease, generated, true).await.unwrap();
        eprintln!(
            "retained_default generation_cycle={cycle} elapsed_ms={} event_reads={}",
            started.elapsed().as_millis(),
            commands.event_reads.load(Ordering::SeqCst)
        );
        commands.event_reads.store(0, Ordering::SeqCst);
        let started = Instant::now();
        let (status, view) = request(
            &store,
            "GET",
            path,
            "operator",
            true,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(view["report"]["status"], "ready");
        assert_eq!(view["report"]["summary"]["sandbox_count"], MEMBERS as u64);
        let summary_us = started.elapsed().as_micros();
        let id = view["report"]["report_id"].as_str().unwrap();
        assert_eq!(id, lease.report.report_id);
        let (page_status, page) = request(
            &store,
            "GET",
            &format!("/api/v1/admin/usage/metrics/reports/{id}/sandboxes?limit=50"),
            "operator",
            true,
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(page_status, 200);
        assert_eq!(page["items"].as_array().unwrap().len(), 50);
        assert_eq!(page["total_count"], MEMBERS as u64);
        assert_eq!(commands.event_reads.load(Ordering::SeqCst), 0);
        eprintln!("retained_default read_cycle={cycle} summary_us={summary_us} summary_plus_page_us={} summary_bytes={} first_page_bytes={} event_reads=0",started.elapsed().as_micros(),serde_json::to_vec(&view).unwrap().len(),serde_json::to_vec(&page).unwrap().len());
        if cycle == 0 {
            prior_id = id.to_owned();
            let started = Instant::now();
            let (status, refresh) = request(
                &store,
                "GET",
                &format!("{path}&refresh=true"),
                "operator",
                true,
                serde_json::Value::Null,
            )
            .await;
            assert_eq!(status, 200);
            assert_eq!(refresh["report"]["report_id"], prior_id);
            assert_eq!(refresh["refresh"]["state"], "pending");
            assert_ne!(refresh["refresh"]["report_id"], prior_id);
            eprintln!(
                "retained_default during_refresh_us={} bytes={}",
                started.elapsed().as_micros(),
                serde_json::to_vec(&refresh).unwrap().len()
            );
        }
    }
    store.db.drop().await.unwrap();
}

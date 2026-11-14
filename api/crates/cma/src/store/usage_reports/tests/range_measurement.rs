//! Cold report execution after separately recorded source-bucket preparation.
use super::*;
use super::allocated::request;
use super::bucket_ranges::{history, prepare_days};
use super::history_batch::observed_store;
use crate::config::report_views::{ViewPolicy, WarmScope};
use std::sync::atomic::Ordering;
use std::time::Instant;
const DAY: u64 = 86400;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_ranges_measure_421_members_over_31_days_with_exact_first_pages() {
    let (original,commands)=observed_store().await;
    let from=(now()/DAY-31)*DAY;
    let to=now()/3600*3600;
    let prepared=Instant::now();
    original.initialize_allocation_buckets(true).await.unwrap();
    let mut states=Vec::new();
    let mut source_count=0;
    for id in 0..421 {
        let mut s=state(id,from);
        let mut events=history(&s,from,32);
        events.retain(|e|e.ended_at<=to);
        s.revision=events.last().unwrap().revision;
        s.last_observed_at=to;
        source_count+=events.len();
        original.collection("sandbox_usage_events").insert_many(events.iter().map(|e|super::super::super::sandbox_usage::event_document(e).unwrap())).await.unwrap();
        prepare_days(&original,&s,&events,from,to).await;
        states.push(s);
    }
    seed(&original,&states).await;
    for n in 0..12 {
        super::owner_names::name(&original,&format!("owner-{n}"),&format!("Public person {n}")).await;
    }
    original.collection("sandbox_usage_events").create_index(
        mongodb::IndexModel::builder().keys(doc!{"sandbox_id":1,"revision":1}).build()
    ).await.unwrap();
    eprintln!("range_fixture members=421 source_events={source_count} preparation_ms={} preparation=production_projector_direct_seed worker_execution=false",prepared.elapsed().as_millis());
    for bucketed in [false,true] {
        let principal=if bucketed {"bucket-fixture"} else {"raw-fixture"};
        let store=original.with_view_policy(ViewPolicy {
            enabled:true,allocation_rollups_enabled:bucketed,
            view_scopes:vec![WarmScope{principal:principal.into(),admin:true,sandbox_id:None}],
            ..Default::default()
        });
        for window in ["24h","7d","30d"] {
            // Each range starts without any immutable report or legacy cache;
            // the source buckets are present only for the explicitly enabled path.
            store.collection("sandbox_usage_rollups").delete_many(doc!{}).await.unwrap();
            commands.event_reads.store(0,Ordering::SeqCst);
            let began=Instant::now();
            let path=format!("/api/v1/admin/usage/metrics/view?window={window}");
            let (status,pending)=request(&store,"GET",&path,principal,true,serde_json::Value::Null).await;
            assert_eq!(status,202,"{pending}");
            let admission_us=began.elapsed().as_micros();
            let lease=store.claim_report("report:0","range-measurement").await.unwrap().unwrap();
            assert_eq!(pending["report"]["report_id"],lease.report.report_id);
            let generation=Instant::now();
            let result=crate::sandbox_metrics::generate_fixture_with_collection(store.clone(),&lease,false).await;
            let generation_us=generation.elapsed().as_micros();
            let success=result.is_ok();
            if !success && bucketed { panic!("prepared range {window} failed: {result:?}"); }
            let report=result.unwrap_or_else(|_|lease.report.clone());
            let finalize=Instant::now();
            store.finish_report(&lease,report,success).await.unwrap();
            let finalization_us=finalize.elapsed().as_micros();
            let event_reads=commands.event_reads.load(Ordering::SeqCst);
            if !success {
                eprintln!("range_measurement window={window} bucketed={bucketed} status=failed admission_us={admission_us} generation_us={generation_us} finalization_us={finalization_us} event_reads={event_reads}");
                continue;
            }
            let reads=Instant::now();
            let (status,view)=request(&store,"GET",&path,principal,true,serde_json::Value::Null).await;
            assert_eq!(status,200,"{view}");
            let id=view["report"]["report_id"].as_str().unwrap();
            assert_eq!(id,lease.report.report_id);
            assert!(view["generated_at"].is_number());
            let (status,page)=request(&store,"GET",&format!("/api/v1/admin/usage/metrics/reports/{id}/sandboxes?limit=50"),principal,true,serde_json::Value::Null).await;
            assert_eq!(status,200,"{page}");
            let rows=page["items"].as_array().unwrap();
            assert_eq!(rows.len(),50);
            assert!(rows.iter().all(|r|r["owner"]["display_name"].is_string()));
            assert_eq!(page["total_count"],421);
            assert_eq!(view["report"]["summary"]["sandbox_count"],421);
            eprintln!("range_measurement window={window} bucketed={bucketed} status=ready admission_us={admission_us} generation_us={generation_us} finalization_us={finalization_us} first_page_us={} committed_reads_us={} event_reads={event_reads} rows=50 labels=50 summary_bytes={} page_bytes={}",began.elapsed().as_micros(),reads.elapsed().as_micros(),serde_json::to_vec(&view).unwrap().len(),serde_json::to_vec(&page).unwrap().len());
        }
    }
    store_cleanup(&original).await;
}
async fn store_cleanup(store:&ReportingStore) { store.db.drop().await.unwrap(); }

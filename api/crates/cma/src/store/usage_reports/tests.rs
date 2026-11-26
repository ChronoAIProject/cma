mod aggregate_projections;
mod binding_fence;
mod initial_snapshot;
mod ownership;
mod projection_lifecycle;
mod projection_scale;
mod publication_fence;
mod publication_races;
mod transaction_retries;
use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::*;
use layer2_core::sandbox_usage::{Allocation, SandboxUsage, UsageTotals};
use layer2_core::{SandboxId, SubjectId};
async fn fixture() -> Arc<ReportingStore> {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required");
    ReportingStore::connect(
        &uri,
        &format!("metrics_{}", uuid::Uuid::new_v4().simple()),
        &[19; 32],
    )
    .await
    .unwrap()
}
fn scope(principal: &str, admin: bool) -> Scope {
    Scope {
        principal: principal.into(),
        admin,
    }
}
fn state(id: usize, at: u64) -> SandboxUsage {
    SandboxUsage {
        sandbox_id: SandboxId(format!("sbx_{id:032x}")),
        owner: SubjectId(format!("owner-{}", id % 12)),
        name: format!("Sandbox {id:06}"),
        status: "active".into(),
        allocation: Allocation {
            running: true,
            cpu_millis: 2000,
            memory_mib: 1024,
            disk_gib: 64,
        },
        created_at: at,
        tracking_started_at: at,
        last_observed_at: at,
        source_seq: 1,
        source_revision: 1,
        revision: 1,
        has_started: true,
        complete_lifecycle: true,
        totals: UsageTotals::default(),
    }
}
async fn seed(store: &ReportingStore, states: &[SandboxUsage]) {
    let docs: Vec<_> = states
        .iter()
        .map(|s| super::super::sandbox_usage::state_document(s).unwrap())
        .collect();
    store
        .collection(super::super::sandbox_usage::STATES)
        .insert_many(docs)
        .await
        .unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_admission_coalesces_and_enforces_tenant_and_shared_queue_limits() {
    let store = fixture().await;
    let at = now();
    let query = ReportInput::default().validate(at).unwrap();
    let a = store
        .submit(scope("owner", false), query.clone(), at)
        .await
        .unwrap();
    let b = store
        .submit(scope("owner", false), query.clone(), at)
        .await
        .unwrap();
    assert_eq!(a.report_id, b.report_id);
    let second = ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        store.db.name(),
        &[19; 32],
    )
    .await
    .unwrap();
    assert_eq!(
        second
            .submit(scope("owner", false), query.clone(), at)
            .await
            .unwrap()
            .report_id,
        a.report_id
    );
    let mut changed = query.clone();
    changed.filters.search = "x".into();
    store
        .submit(scope("owner", false), changed.clone(), at)
        .await
        .unwrap();
    changed.filters.search = "y".into();
    assert!(matches!(
        store.submit(scope("owner", false), changed, at).await,
        Err(MetricsError::Busy)
    ));
    for id in 0..6 {
        store
            .submit(scope(&format!("other-{id}"), true), query.clone(), at)
            .await
            .unwrap();
    }
    assert!(matches!(
        store.submit(scope("ninth", true), query, at).await,
        Err(MetricsError::Busy)
    ));
    store.db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_admission_reserves_exact_bytes_across_budget_changes() {
    let original = fixture().await;
    let mut store = (*original).clone();
    store.budgets.report_bytes = 200;
    store.budgets.tenant_bytes = 300;
    store.budgets.global_bytes = 450;
    let at = now();
    let query = ReportInput::default().validate(at).unwrap();
    store
        .submit(scope("$owner", false), query.clone(), at)
        .await
        .unwrap();
    let mut larger_worker = store.clone();
    larger_worker.budgets.report_bytes = 300;
    let lease = larger_worker
        .claim_report("report:0", "larger-worker")
        .await
        .unwrap()
        .unwrap();
    let chart = doc! {"_id":"budget-test","document_schema_version":1_i64,"record":"x".repeat(210)};
    assert!(matches!(
        larger_worker
            .write_report_chunk(&lease, &[], vec![chart])
            .await,
        Err(MetricsError::Busy)
    ));
    let mut changed = query.clone();
    changed.filters.search = "second".into();
    assert!(matches!(
        store
            .submit(scope("$owner", false), changed.clone(), at)
            .await,
        Err(MetricsError::Busy)
    ));
    let mut reduced = store.clone();
    reduced.budgets.report_bytes = 100;
    reduced
        .submit(scope("$owner", false), changed, at)
        .await
        .unwrap();
    reduced
        .submit(scope("another", false), query.clone(), at)
        .await
        .unwrap();
    // Only 50 bytes remain globally. The new 100-byte reservation must fail,
    // even though three jobs times the new smaller budget would leave room.
    assert!(matches!(
        reduced.submit(scope("third", false), query, at).await,
        Err(MetricsError::Busy)
    ));
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        3
    );
    store.db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_lease_takeover_fences_old_writes_and_cache_rechecks_current_owner() {
    let store = fixture().await;
    let at = now();
    let original = state(1, at - 3600);
    seed(&store, std::slice::from_ref(&original)).await;
    let report = store
        .submit(
            scope("owner-1", false),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "old")
        .await
        .unwrap()
        .unwrap();
    store
        .collection(JOBS)
        .update_one(
            doc! {"_id":&report.report_id},
            doc! {"$set":{"lease_until":0_i64}},
        )
        .await
        .unwrap();
    let next = store
        .claim_report("report:1", "new")
        .await
        .unwrap()
        .unwrap();
    assert!(store.write_report_chunk(&lease, &[], vec![]).await.is_err());
    assert!(store
        .finish_report(&lease, report.clone(), true)
        .await
        .is_err());
    let row = fixture_row(&original);
    store
        .write_report_chunk(&next, std::slice::from_ref(&row), vec![])
        .await
        .unwrap();
    store
        .finish_report(&next, report.clone(), true)
        .await
        .unwrap();
    assert!(store.write_report_chunk(&next, &[], vec![]).await.is_err());
    store.reauthorize(&report).await.unwrap();
    // A current Sandbox transfer invalidates a frozen ledger-owner projection,
    // even before the retained ledger catches up to the new owner.
    store
        .collection("sandboxes")
        .insert_one(doc! {"_id":original.sandbox_id.as_str(),"owner":"another-owner"})
        .await
        .unwrap();
    assert!(matches!(
        store.reauthorize(&report).await,
        Err(MetricsError::NotFound)
    ));
    assert!(matches!(
        store
            .authorize_sandbox(&report.scope, original.sandbox_id.as_str())
            .await,
        Err(MetricsError::NotFound)
    ));
    store.db.drop().await.unwrap();
}
fn fixture_row(state: &SandboxUsage) -> Row {
    let measurement = |m| {
        Measurement::project(
            &Aggregate {
                integral: 60.,
                display_integral: 60.,
                covered_seconds: 60.,
                expected_seconds: 60.,
                observed_at: Some(state.created_at as f64),
                latest: Some(1.),
                ..Default::default()
            },
            m,
            state.created_at,
            true,
            0.,
        )
    };
    Row {
        sandbox_id: state.sandbox_id.to_string(),
        name: state.name.clone(),
        owner: Owner {
            subject: state.owner.to_string(),
            display_name: None,
        },
        status: state.status.clone(),
        runtime_version: None,
        agent_count: 1,
        last_activity_at: Some(state.created_at),
        allocation: Capacity::default(),
        allocated_period: AllocatedPeriod {
            complete: true,
            ..Default::default()
        },
        measured: Measured {
            cpu: measurement(Metric::Cpu),
            memory: measurement(Metric::Memory),
            disk: measurement(Metric::Disk),
        },
        billing: Billing::default(),
        shared_sandbox_usage: false,
    }
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_all_sorts_are_indexed_stable_null_last_and_cursor_bound() {
    let store = fixture().await;
    let at = now();
    let mut report = store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "sorter")
        .await
        .unwrap()
        .unwrap();
    let mut rows: Vec<_> = (0..4).map(|n| fixture_row(&state(n, at - 60))).collect();
    rows[0].measured.cpu.mean = Some(4.);
    rows[1].measured.cpu.mean = Some(2.);
    rows[2].measured.cpu.mean = None;
    rows[3].measured.cpu.mean = Some(2.);
    store
        .write_report_chunk(&lease, &rows, vec![])
        .await
        .unwrap();
    report.summary = Some(Summary {
        sandbox_count: 4,
        ..Default::default()
    });
    store
        .finish_report(&lease, report.clone(), true)
        .await
        .unwrap();
    for (order, expected) in [
        (Order::Asc, vec![1, 3, 0, 2]),
        (Order::Desc, vec![0, 3, 1, 2]),
    ] {
        let mut cursor = None;
        let mut ids = Vec::new();
        loop {
            let page = store
                .page(
                    &report,
                    PageQuery {
                        limit: Some(1),
                        cursor: cursor.clone(),
                        sort: Sort::CpuMeanCores,
                        order,
                    },
                )
                .await
                .unwrap();
            ids.extend(page.items.iter().map(|r| r.sandbox_id.clone()));
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
            assert!(matches!(
                store
                    .page(
                        &report,
                        PageQuery {
                            cursor: cursor.clone(),
                            sort: Sort::Name,
                            order,
                            ..Default::default()
                        }
                    )
                    .await,
                Err(MetricsError::Invalid(_))
            ));
        }
        assert_eq!(
            ids,
            expected
                .into_iter()
                .map(|n| format!("sbx_{n:032x}"))
                .collect::<Vec<_>>()
        );
    }
    for sort in Sort::ALL {
        for direction in [1, -1] {
            let sorting = doc! {format!("nulls.{}",sort.key()):1,format!("sorts.{}",sort.key()):direction,"sandbox_id":direction};
            let explain=store.db.run_command(doc!{"explain":{"find":ROWS,"filter":{"report_id":&report.report_id},"sort":sorting,"limit":50},"verbosity":"executionStats"}).await.unwrap();
            let plan = explain
                .get_document("queryPlanner")
                .unwrap()
                .get_document("winningPlan")
                .unwrap()
                .to_string();
            assert!(plan.contains("IXSCAN"), "{sort:?} {direction}: {plan}");
            assert!(
                !plan.contains("COLLSCAN") && !plan.contains("\"SORT\""),
                "{plan}"
            );
        }
    }
    let mut indexes = store.collection(ROWS).list_indexes().await.unwrap();
    let mut ttl = false;
    while let Some(index) = indexes.try_next().await.unwrap() {
        ttl |= index.options.and_then(|o| o.expire_after) == Some(Duration::ZERO);
    }
    assert!(ttl);
    store.db.drop().await.unwrap();
}

mod allocated;
mod allocated_sort;
mod allocation_cold_range;
mod allocation_interval;
pub(super) mod circuit_capture;
mod control;
mod history_batch;
mod options;
mod options_authority;
mod options_context;
mod options_pressure;
mod owner_allocation;
mod owner_allocation_boundaries;
mod owner_names;
mod page_batch;
mod pressure;
mod read_models;
mod scale;
mod submission;
mod tenant_isolation;
mod terminal_reports;
mod terminal_usage;

mod retained_recovery;
mod retained_restart;
mod retained_views;

mod retained_measurement;

mod retained_windows;

mod boundary_deltas;
mod bucket_ranges;
mod rebaseline;

mod range_measurement;

mod hotfix_matrix;
mod on_demand_races;
mod on_demand_views;

mod hotfix_cpu;

mod allocation_write_order;

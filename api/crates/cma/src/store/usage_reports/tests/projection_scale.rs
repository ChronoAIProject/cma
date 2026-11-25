//! Local qualification measures the immutable report reader with synthetic
//! frozen chart/row inputs. Source sampling and rollup preparation are separate.
use super::*;
use crate::config::report_views::{ViewPolicy, WarmScope};
use crate::store::migrations::usage_projections::{self, COLLECTION};
use layer2_core::sandbox_metrics::{allocation::Expectation, series::Bucket};
use layer2_core::{Authorization, Caller, Capability};
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

async fn counted_store() -> (Arc<ReportingStore>, Arc<AtomicUsize>) {
    let original = fixture().await;
    let commands = Arc::new(AtomicUsize::new(0));
    let counter = commands.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    configure_pool(&mut options, 4);
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(_) = event {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }));
    let mut store = (*original).clone();
    store.persistence = store
        .persistence
        .with_client(Client::with_options(options).unwrap());
    (Arc::new(store), commands)
}

fn chart_buckets(range: Range, step: u64, member: usize, dense: bool) -> Vec<Bucket> {
    let count = range.seconds().div_ceil(step);
    let stride = if dense { 1 } else { (count / 8).max(1) };
    (0..count)
        .filter(|index| index % stride == 0)
        .map(|index| {
            let from = range.from + index * step;
            let to = (from + step).min(range.to);
            let duration = (to - from) as f64;
            let value = ((member + index as usize) % 379 + 1) as f64 / 100.;
            let mut bucket = Bucket {
                from,
                to,
                integral: value * duration,
                display_integral: value * duration,
                covered_seconds: duration,
                expected_seconds: duration,
                min: Some(value),
                max: Some(value),
                ..Default::default()
            };
            bucket.finish_time();
            bucket
        })
        .collect()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn aggregate_projection_ranges_measure_sparse_and_dense_frozen_sources() {
    for (members, dense) in [(64, false), (64, true), (20_001, false), (20_001, true)] {
        for (window, seconds, step) in [
            ("24h", 86_400, 60),
            ("7d", 604_800, 600),
            ("30d", 2_592_000, 3600),
        ] {
            let (legacy, commands) = counted_store().await;
            let store = legacy.with_view_policy(ViewPolicy {
                aggregate_projections_enabled: true,
                view_scopes: vec![WarmScope {
                    admin: true,
                    principal: "operator".into(),
                    sandbox_id: None,
                }],
                ..Default::default()
            });
            for (collection, index) in usage_projections::inspect(&store.db).await.unwrap() {
                store
                    .db
                    .run_command(doc! {"createIndexes":collection,"indexes":[index]})
                    .await
                    .unwrap();
            }
            let at = now();
            let to = (at - 3600) / 3600 * 3600;
            let from = to - seconds;
            store
                .submit(
                    scope("operator", true),
                    ReportInput {
                        from: Some(from),
                        to: Some(to),
                        step_seconds: Some(step),
                        ..Default::default()
                    }
                    .validate(at)
                    .unwrap(),
                    at,
                )
                .await
                .unwrap();
            let lease = store
                .claim_report("report:0", "projection-scale")
                .await
                .unwrap()
                .unwrap();
            let report_id = &lease.report.report_id;
            let seed_started = Instant::now();
            let mut accepted = 0;
            let mut seed_error = None;
            let batch_size = if dense { 8 } else { 100 };
            for offset in (0..members).step_by(batch_size) {
                let states: Vec<_> = (offset..(offset + batch_size).min(members))
                    .map(|member| {
                        let mut s = state(member, from);
                        // Multiple Sandboxes per owner with a bounded distinct
                        // owner distribution; the scale outcome reports this.
                        s.owner = SubjectId(format!("owner-{:04}", member % 431));
                        s
                    })
                    .collect();
                seed(&store, &states).await;
                let rows: Vec<_> = states.iter().map(fixture_row).collect();
                let mut charts = Vec::new();
                let expectations = [Expectation {
                    from,
                    to,
                    running: Some(true),
                    home: Some(true),
                }];
                for (index, row) in rows.iter().enumerate() {
                    for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
                        charts.extend(
                            chart_documents(
                                &lease.report,
                                &row.sandbox_id,
                                &row.owner.subject,
                                metric,
                                chart_buckets(
                                    lease.report.effective_range,
                                    step,
                                    offset + index,
                                    dense,
                                ),
                                &expectations,
                            )
                            .unwrap(),
                        );
                    }
                }
                match store
                    .write_prepared_report_chunk(&lease, &rows, charts)
                    .await
                {
                    Ok(()) => accepted += rows.len(),
                    Err(error) => {
                        seed_error = Some(error);
                        break;
                    }
                }
            }
            let before = store
                .collection(JOBS)
                .find_one(doc! {"_id":report_id})
                .await
                .unwrap()
                .unwrap();
            eprintln!("projection_scale seed window={window} members={members} owners={} dense={dense} accepted={accepted} elapsed_ms={} bytes={} outcome={seed_error:?}", members.min(431), seed_started.elapsed().as_millis(), before.get_i64("bytes").unwrap());
            if let Some(error) = seed_error {
                assert!(matches!(
                    error,
                    MetricsError::Busy | MetricsError::Unavailable
                ));
                store.db.drop().await.unwrap();
                continue;
            }
            let mut report = lease.report.clone();
            report.summary = Some(Summary {
                sandbox_count: members as u64,
                ..Default::default()
            });
            let started = Instant::now();
            commands.store(0, Ordering::Relaxed);
            let projection = store.prepare_aggregate_projections(&lease, &report).await;
            let generation_commands = commands.load(Ordering::Relaxed);
            let generation_ms = started.elapsed().as_millis();
            let after = store
                .collection(JOBS)
                .find_one(doc! {"_id":report_id})
                .await
                .unwrap()
                .unwrap();
            let projected = after.contains_key("projection_generation");
            eprintln!("projection_scale build window={window} members={members} dense={dense} projected={projected} generation_ms={generation_ms} commands={generation_commands} projection_bytes={} outcome={projection:?}", after.get_i64("bytes").unwrap()-before.get_i64("bytes").unwrap());
            projection.unwrap();
            store.finish_report(&lease, report, true).await.unwrap();
            let report = store
                .report(&scope("operator", true), report_id, now())
                .await
                .unwrap();
            let caller = Caller::new(
                SubjectId("operator".into()),
                Authorization::new([Capability::ReadAllSandboxes]),
            );
            for route in ["initial", "total", "top", "explicit_owner", "allocation"] {
                let mut baseline = None;
                for (enabled, reader) in [(false, &legacy), (true, &store)] {
                    let service = MetricsService::new(reader.clone());
                    commands.store(0, Ordering::Relaxed);
                    let started = Instant::now();
                    let result = match route {
                        "initial" => service
                            .initial_report(
                                &caller,
                                true,
                                report_id,
                                InitialProjection {
                                    total_metrics: vec![Metric::Cpu, Metric::Memory, Metric::Disk],
                                    ..Default::default()
                                },
                                now(),
                            )
                            .await
                            .map(|value| serde_json::to_value(value).unwrap()),
                        "allocation" => service
                            .owner_allocations(
                                &caller,
                                report_id,
                                OwnerAllocationQuery {
                                    metric: Metric::Cpu,
                                    limit: Some(50),
                                    cursor: None,
                                },
                                now(),
                            )
                            .await
                            .map(|value| serde_json::to_value(value).unwrap()),
                        _ => {
                            let query = SeriesQuery {
                                metric: Metric::Cpu,
                                group_by: if route == "total" {
                                    GroupBy::Total
                                } else {
                                    GroupBy::Owner
                                },
                                top: if route == "top" { Some(10) } else { None },
                                series_id: if route == "explicit_owner" {
                                    Some("owner-0001".into())
                                } else {
                                    None
                                },
                            };
                            service
                                .series(&caller, true, report_id, query, now())
                                .await
                                .map(|value| serde_json::to_value(value).unwrap())
                        }
                    };
                    let elapsed = started.elapsed();
                    let bytes = result
                        .as_ref()
                        .ok()
                        .map_or(0, |body| serde_json::to_vec(body).unwrap().len());
                    let error = result.as_ref().err();
                    eprintln!("projection_scale read window={window} members={members} dense={dense} projected={projected} enabled={enabled} route={route} error={error:?} read_us={} commands={} response_bytes={bytes}", elapsed.as_micros(), commands.load(Ordering::Relaxed));
                    if let Some(previous) = baseline.take() {
                        if let (Ok(body), Ok(previous_body)) = (&result, previous) {
                            assert_eq!(body, &previous_body);
                        }
                    } else {
                        baseline = Some(result);
                    }
                }
            }
            assert_eq!(report.summary.unwrap().sandbox_count, members as u64);
            let physical = store
                .collection(COLLECTION)
                .count_documents(doc! {})
                .await
                .unwrap();
            assert_eq!(physical > 0, projected);
            store.db.drop().await.unwrap();
        }
    }
}

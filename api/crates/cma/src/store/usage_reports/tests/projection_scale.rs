//! Local qualification measures the immutable report reader with synthetic
//! frozen chart/row inputs. Source sampling and rollup preparation are separate.
use super::*;
use crate::config::report_views::{ViewPolicy, WarmScope};
use crate::store::migrations::usage_projections::{self, COLLECTION};
use layer2_core::sandbox_metrics::{allocation::Expectation, series::Bucket};
use layer2_core::{Authorization, Caller, Capability};
use mongodb::event::{command::CommandEvent, EventHandler};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

async fn counted_store() -> (Arc<ReportingStore>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let original = fixture().await;
    let commands = Arc::new(AtomicUsize::new(0));
    let counter = commands.clone();
    let get_mores = Arc::new(AtomicUsize::new(0));
    let get_more_counter = get_mores.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    configure_pool(&mut options, 4);
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            counter.fetch_add(1, Ordering::Relaxed);
            if event.command_name == "getMore" {
                get_more_counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    }));
    let mut store = (*original).clone();
    store.persistence = store
        .persistence
        .with_client(Client::with_options(options).unwrap());
    (Arc::new(store), commands, get_mores)
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

fn initial_projection() -> InitialProjection {
    InitialProjection {
        total_metrics: vec![Metric::Cpu, Metric::Memory, Metric::Disk],
        ..Default::default()
    }
}

fn route_query(route: &str, metric: Metric) -> SeriesQuery {
    SeriesQuery {
        metric,
        group_by: if route == "total" {
            GroupBy::Total
        } else {
            GroupBy::Owner
        },
        top: (route == "top").then_some(10),
        series_id: (route == "explicit_owner").then(|| "owner-0001".into()),
    }
}

async fn legacy_reference(
    store: &Arc<ReportingStore>,
    caller: &Caller,
    report: &Report,
    route: &str,
) -> Result<Vec<u8>> {
    let service = MetricsService::new(store.clone());
    if route == "initial" {
        let projection = initial_projection();
        let rows = service
            .page(
                caller,
                true,
                &report.report_id,
                PageQuery {
                    limit: Some(projection.first_page.limit),
                    sort: projection.first_page.sort,
                    order: projection.first_page.order,
                    cursor: None,
                },
                now(),
            )
            .await?;
        let mut charts = Vec::new();
        for &metric in &projection.total_metrics {
            charts.push(
                cma_am_reporting::read_reference_series(
                    store.collection(CHARTS),
                    report,
                    route_query("total", metric),
                )
                .await?,
            );
        }
        let response = InitialReport {
            report: report.clone(),
            initial: Some(InitialData {
                projection,
                rows,
                charts,
            }),
        };
        Ok(serde_json::to_vec(&response).unwrap())
    } else {
        let response = cma_am_reporting::read_reference_series(
            store.collection(CHARTS),
            report,
            route_query(route, Metric::Cpu),
        )
        .await?;
        Ok(serde_json::to_vec(&response).unwrap())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn aggregate_projection_ranges_measure_sparse_and_dense_frozen_sources() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let mut failures = Vec::new();
    for (members, dense) in [(64, false), (64, true), (20_001, false), (20_001, true)] {
        for (window, seconds, step) in [
            ("24h", 86_400, 60),
            ("7d", 604_800, 600),
            ("30d", 2_592_000, 3600),
        ] {
            let (legacy, commands, get_mores) = counted_store().await;
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
            // Existing usage states precede report admission in production. Keep
            // synthetic state ingestion outside the report generation deadline.
            let states: Vec<_> = (0..members)
                .map(|member| {
                    let mut state = state(member, from);
                    state.owner = SubjectId(format!("owner-{:04}", member % 431));
                    state
                })
                .collect();
            for chunk in states.chunks(250) {
                seed(&store, chunk).await;
            }
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
            let mut totals = cma_am_reporting::TotalProjection::new(
                &lease.report.report_id,
                lease.generation,
                &lease.holder,
            );
            let seed_started = Instant::now();
            let mut accepted = 0;
            let mut refused_batch_bytes = None;
            let mut charged_bytes = 0_i64;
            let mut chart_count = 0_i64;
            let batch_size = if dense { 8 } else { 250 };
            for offset in (0..members).step_by(batch_size) {
                let rows: Vec<_> = states[offset..(offset + batch_size).min(members)]
                    .iter()
                    .map(fixture_row)
                    .collect();
                let mut charts = Vec::new();
                let expectations = [Expectation {
                    from,
                    to,
                    running: Some(true),
                    home: Some(true),
                }];
                for (index, row) in rows.iter().enumerate() {
                    let buckets =
                        chart_buckets(lease.report.effective_range, step, offset + index, dense);
                    let sparse: BTreeMap<_, _> =
                        buckets.iter().map(|b| (b.from, b.clone())).collect();
                    totals
                        .add_member(
                            &lease.report,
                            &row.sandbox_id,
                            &[sparse.clone(), sparse.clone(), sparse],
                            &expectations,
                        )
                        .unwrap();
                    for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
                        charts.extend(
                            chart_documents(
                                &lease.report,
                                &row.sandbox_id,
                                &row.owner.subject,
                                metric,
                                buckets.clone(),
                                &expectations,
                            )
                            .unwrap(),
                        );
                    }
                }
                let row_bytes: i64 = rows
                    .iter()
                    .map(|row| {
                        let document =
                            super::super::work::row_document(&lease.report, row).unwrap();
                        mongodb::bson::serialize_to_vec(&document).unwrap().len() as i64
                    })
                    .sum();
                let chunk_bytes = row_bytes
                    + charts
                        .iter()
                        .map(|doc| doc.as_bytes().len() as i64)
                        .sum::<i64>();
                let batch_charts = charts.len() as i64;
                let outcome = store
                    .write_prepared_report_chunk(&lease, &rows, charts)
                    .await;
                if charged_bytes + chunk_bytes > store.budgets.report_bytes {
                    assert!(
                        matches!(outcome, Err(MetricsError::Busy)),
                        "expected report byte budget refusal: {outcome:?}"
                    );
                    refused_batch_bytes = Some(chunk_bytes);
                    break;
                }
                outcome.unwrap_or_else(|error| panic!("unexpected source write failure window={window} members={members} dense={dense} offset={offset}: {error:?}"));
                charged_bytes += chunk_bytes;
                chart_count += batch_charts;
                accepted += rows.len();
                totals.acknowledge_source(rows.len()).unwrap();
            }
            let before = store
                .collection(JOBS)
                .find_one(doc! {"_id":report_id})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(before.get_i64("bytes").unwrap(), charged_bytes);
            assert_eq!(before.get_i64("row_count").unwrap(), accepted as i64);
            assert_eq!(before.get_i64("chart_count").unwrap(), chart_count);
            assert_eq!(
                before.get_i64("reserved_bytes").unwrap(),
                store.budgets.report_bytes
            );
            eprintln!("projection_scale seed window={window} members={members} owners={} dense={dense} accepted={accepted} elapsed_ms={} bytes={charged_bytes} budget_bytes={} refused_batch_bytes={refused_batch_bytes:?}", members.min(431), seed_started.elapsed().as_millis(), store.budgets.report_bytes);
            if refused_batch_bytes.is_some() {
                assert!(
                    dense && members == 20_001,
                    "unexpected source report byte budget"
                );
                assert_eq!(before.get_str("state").unwrap(), "running");
                store
                    .finish_report(&lease, lease.report.clone(), false)
                    .await
                    .unwrap();
                let failed = store
                    .collection(JOBS)
                    .find_one(doc! {"_id":report_id})
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(failed.get_str("state").unwrap(), "failed");
                assert_eq!(failed.get_i64("bytes").unwrap(), charged_bytes);
                assert!(!failed.contains_key("projection_generation"));
                store.db.drop().await.unwrap();
                continue;
            }
            assert_eq!(accepted, members);
            let mut report = lease.report.clone();
            report.summary = Some(Summary {
                sandbox_count: members as u64,
                ..Default::default()
            });
            let started = Instant::now();
            commands.store(0, Ordering::Relaxed);
            let projection = store
                .prepare_aggregate_projections(
                    &lease,
                    &report,
                    Some(totals),
                    seed_started + Duration::from_secs(60),
                )
                .await;
            let generation_commands = commands.load(Ordering::Relaxed);
            let generation_ms = started.elapsed().as_millis();
            let after = store
                .collection(JOBS)
                .find_one(doc! {"_id":report_id})
                .await
                .unwrap()
                .unwrap();
            let projected = after.contains_key("projection_generation");
            let families: Vec<_> = after
                .get_document("projection_manifest")
                .ok()
                .and_then(|manifest| manifest.get_array("families").ok())
                .into_iter()
                .flatten()
                .map(|family| family.as_document().unwrap().get_str("kind").unwrap())
                .collect();
            eprintln!("projection_scale families window={window} members={members} dense={dense} published={families:?}");
            eprintln!("projection_scale build window={window} members={members} dense={dense} projected={projected} generation_ms={generation_ms} commands={generation_commands} projection_bytes={} outcome={projection:?}", after.get_i64("bytes").unwrap()-before.get_i64("bytes").unwrap());
            for kind in ["chart_owner", "chart_sandbox"] {
                if !families.contains(&kind) {
                    failures.push(format!(
                        "missing ranked projection window={window} members={members} dense={dense} kind={kind}"
                    ));
                }
            }
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
                    let family = match route {
                        "initial" | "total" => "chart_total",
                        "top" => "chart_owner",
                        "explicit_owner" => "chart_explicit_owner",
                        "allocation" => "owner_allocation",
                        _ => unreachable!(),
                    };
                    let source = if enabled && families.contains(&family) {
                        "projection"
                    } else {
                        "legacy"
                    };
                    let result = match route {
                        "initial" => service
                            .initial_report(&caller, true, report_id, initial_projection(), now())
                            .await
                            .map(|value| serde_json::to_vec(&value).unwrap()),
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
                            .map(|value| serde_json::to_vec(&value).unwrap()),
                        _ => {
                            let query = route_query(route, Metric::Cpu);
                            service
                                .series(&caller, true, report_id, query, now())
                                .await
                                .map(|value| serde_json::to_vec(&value).unwrap())
                        }
                    };
                    let elapsed = started.elapsed();
                    let bytes = result.as_ref().ok().map_or(0, Vec::len);
                    let error = result.as_ref().err();
                    eprintln!("projection_scale read window={window} members={members} dense={dense} projected={projected} enabled={enabled} route={route} source={source} error={error:?} read_us={} commands={} response_bytes={bytes}", elapsed.as_micros(), commands.load(Ordering::Relaxed));
                    match result {
                        Ok(value) if enabled => {
                            if baseline.take().as_ref() != Some(&value) {
                                failures.push(format!("response byte parity window={window} members={members} dense={dense} route={route}"));
                            }
                        }
                        Ok(value) => {
                            if members == 64 && matches!(route, "initial" | "total" | "top") {
                                let reference = legacy_reference(&legacy, &caller, &report, route).await
                                    .expect("small reference must complete");
                                assert_eq!(reference, value, "small fixture reference differs from public response");
                            }
                            baseline = Some(value);
                        }
                        Err(MetricsError::Busy) if !enabled && members == 20_001 && !dense
                            && matches!(route, "initial" | "total" | "top") => {
                            let reference_started = Instant::now();
                            commands.store(0, Ordering::Relaxed);
                            let reference = legacy_reference(&legacy, &caller, &report, route).await
                                .expect("large reference must complete");
                            eprintln!("projection_scale reference window={window} members={members} route={route} elapsed_ms={} commands={} response_bytes={}", reference_started.elapsed().as_millis(), commands.load(Ordering::Relaxed), reference.len());
                            baseline = Some(reference);
                        }
                        Err(error) => failures.push(format!("authorized read failed window={window} members={members} dense={dense} enabled={enabled} route={route}: {error:?}")),
                    }
                }
            }
            if members == 20_001 && !dense {
                let base = doc! {"report_id":report_id,"generation":lease.generation,
                "document_schema_version":1_i64,"expires_at":expires(report.expires_at + 60)};
                let charts = store.collection(CHARTS);
                let mut completed = None;
                for size in [100, 1000] {
                    commands.store(0, Ordering::Relaxed);
                    get_mores.store(0, Ordering::Relaxed);
                    let started = Instant::now();
                    let mut batch = cma_am_reporting::ProjectionBatch::new(32 * 1024 * 1024);
                    let deadline = started + Duration::from_secs(10);
                    let outcome = tokio::time::timeout_at(
                        tokio::time::Instant::from_std(deadline),
                        cma_am_reporting::benchmark_ranked_projections(
                            &charts, &report, &base, &mut batch, deadline, size,
                        ),
                    )
                    .await
                    .unwrap_or(Err(MetricsError::Busy));
                    let documents = batch.into_documents();
                    let bytes = documents
                        .iter()
                        .map(|doc| doc.as_bytes().len())
                        .sum::<usize>();
                    eprintln!("projection_scale batch window={window} size={size} result={outcome:?} elapsed_ms={} commands={} get_mores={} scratch_chunks={} scratch_bytes={bytes}", started.elapsed().as_millis(), commands.load(Ordering::Relaxed), get_mores.load(Ordering::Relaxed), documents.len());
                    assert!(matches!(outcome, Ok(()) | Err(MetricsError::Busy)));
                    if outcome.is_ok() {
                        if let Some(previous) = completed.as_ref() {
                            assert_eq!(
                                previous, &documents,
                                "batching changed stored projection bytes"
                            );
                        }
                        completed = Some(documents);
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
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

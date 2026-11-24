use super::*;
use store::SANDBOX;

fn input() -> InitialReportInput {
    InitialReportInput {
        query: ReportInput {
            from: Some(AT - 600),
            to: Some(AT - 120),
            filters: Filters {
                agent_id: Some("agt_00000000000000000000000000000001".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        projection: projection(),
    }
}
fn latest() -> InitialViewInput {
    InitialViewInput {
        selection: ViewInput {
            window: ViewWindow::Custom,
            from: input().query.from,
            to: input().query.to,
            filters: input().query.filters,
            ..Default::default()
        },
        projection: projection(),
    }
}

#[tokio::test]
async fn initial_create_and_latest_select_then_authorize_once() {
    for admin in [false, true] {
        for is_latest in [false, true] {
            let store = Arc::new(Store::new("", admin));
            let service = MetricsService::new(store.clone());
            let (report, initial) = if is_latest {
                let result = service
                    .latest_initial(&caller(admin), admin, latest(), AT)
                    .await
                    .unwrap();
                (result.view.report, result.initial)
            } else {
                let result = service
                    .submit_initial(&caller(admin), admin, input(), AT)
                    .await
                    .unwrap();
                (result.report, result.initial)
            };
            assert_eq!(report.query.filters.sandbox_id.as_deref(), Some(SANDBOX));
            assert_eq!(initial.unwrap().charts.len(), 3);
            let calls = store.calls.lock().unwrap().clone();
            assert_eq!(&calls[..2], ["resolve", "selected"]);
            assert_eq!(calls.iter().filter(|&&c| c == "authorize").count(), 1);
            assert!(!calls.contains(&"report"));
            store.revoked.store(true, Ordering::SeqCst);
            let result = if is_latest {
                service
                    .latest_initial(&caller(admin), admin, latest(), AT)
                    .await
                    .map(|_| ())
            } else {
                service
                    .submit_initial(&caller(admin), admin, input(), AT)
                    .await
                    .map(|_| ())
            };
            assert!(matches!(result, Err(MetricsError::NotFound)));
        }
    }
}

#[tokio::test]
async fn initial_admission_rejects_invalid_projection_before_selection_or_work() {
    for is_latest in [false, true] {
        for fault in ["limit", "metric", "capability", "query", "owner"] {
            let store = Arc::new(Store::new("", false));
            let service = MetricsService::new(store.clone());
            let mut input = input();
            let mut who = caller(false);
            match fault {
                "limit" => input.projection.first_page.limit = 51,
                "metric" => input.projection.total_metrics.push(Metric::Cpu),
                "capability" => who.authorization = Authorization::new([]),
                "query" => input.query.step_seconds = Some(1),
                "owner" => input.query.filters.owner = Some("foreign".into()),
                _ => unreachable!(),
            }
            let result = if is_latest {
                let mut view = latest();
                view.projection = input.projection;
                view.selection.step_seconds = input.query.step_seconds;
                view.selection.filters = input.query.filters;
                service
                    .latest_initial(&who, false, view, AT)
                    .await
                    .map(|_| ())
            } else {
                service
                    .submit_initial(&who, false, input, AT)
                    .await
                    .map(|_| ())
            };
            assert!(result.is_err(), "{fault}");
            assert!(store.calls.lock().unwrap().is_empty(), "{fault}");
        }
    }
}

#[tokio::test]
async fn initial_admission_refuses_parent_mismatch_and_changed_returned_selection() {
    for is_latest in [false, true] {
        let store = Arc::new(Store::new("", false));
        let service = MetricsService::new(store.clone());
        let mut input = input();
        input.query.filters.sandbox_id = Some("sbx_00000000000000000000000000000002".into());
        let result = if is_latest {
            let mut view = latest();
            view.selection.filters = input.query.filters;
            service
                .latest_initial(&caller(false), false, view, AT)
                .await
                .map(|_| ())
        } else {
            service
                .submit_initial(&caller(false), false, input, AT)
                .await
                .map(|_| ())
        };
        assert!(matches!(result, Err(MetricsError::Invalid(_))));
        assert_eq!(*store.calls.lock().unwrap(), ["resolve"]);
        for fault in ["scope", "submit-query", "post-submit-revocation"] {
            let store = Arc::new(Store::new(fault, false));
            let service = MetricsService::new(store.clone());
            let result = if is_latest {
                service
                    .latest_initial(&caller(false), false, latest(), AT)
                    .await
                    .map(|_| ())
            } else {
                service
                    .submit_initial(&caller(false), false, self::input(), AT)
                    .await
                    .map(|_| ())
            };
            assert!(result.is_err(), "{fault}");
            assert!(!store.calls.lock().unwrap().contains(&"page"));
        }
    }
}

#[tokio::test]
async fn initial_latest_keeps_pending_identity_and_typed_disabled_or_mismatch_errors() {
    let store = Arc::new(Store::new("pending", false));
    let service = MetricsService::new(store.clone());
    let result = service
        .latest_initial(&caller(false), false, latest(), AT)
        .await
        .unwrap();
    assert_eq!(result.view.refresh.report_id.as_deref(), Some(REPORT));
    assert!(result.initial.is_none());
    let result = service
        .initial_report(&caller(false), false, REPORT, projection(), AT)
        .await
        .unwrap();
    assert_eq!(result.report.report_id, REPORT);
    assert!(result.initial.is_none());
    let calls = store.calls.lock().unwrap().clone();
    assert_eq!(calls.iter().filter(|&&c| c == "submit").count(), 1);
    assert_eq!(calls.iter().filter(|&&c| c == "authorize").count(), 2);
    assert!(!calls.contains(&"page"));
    for fault in ["view-disabled", "view-range"] {
        let service = MetricsService::new(Arc::new(Store::new(fault, false)));
        let result = service
            .latest_initial(&caller(false), false, latest(), AT)
            .await;
        assert!(if fault == "view-disabled" {
            matches!(result, Err(MetricsError::ViewDisabled))
        } else {
            matches!(result, Err(MetricsError::Unavailable))
        });
    }
}

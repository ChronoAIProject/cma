mod selection;
mod store;
use super::*;
use crate::{Authorization, Caller, Capability, SubjectId};
use std::sync::{atomic::Ordering, Arc};
use store::{Store, AT, REPORT};

fn caller(admin: bool) -> Caller {
    Caller::new(
        SubjectId("owner".into()),
        Authorization::new([if admin {
            Capability::ReadAllSandboxes
        } else {
            Capability::UseOwnSandboxes
        }]),
    )
}
fn projection() -> InitialProjection {
    InitialProjection {
        total_metrics: vec![Metric::Cpu, Metric::Memory, Metric::Disk],
        ..Default::default()
    }
}

#[test]
fn initial_inputs_refuse_cursors_grouping_children_and_excess_bounds() {
    for json in [
        r#"{"first_page":{"cursor":"next"}}"#,
        r#"{"first_page":{"report_id":"other"}}"#,
        r#"{"group_by":"owner"}"#,
        r#"{"series_id":"other"}"#,
        r#"{"total_metrics":[{"metric":"cpu","group_by":"sandbox"}]}"#,
    ] {
        assert!(
            serde_json::from_str::<InitialProjection>(json).is_err(),
            "{json}"
        );
    }
    for limit in [1, 50] {
        let mut p = projection();
        p.first_page.limit = limit;
        p.validate(false).unwrap();
    }
    for limit in [0, 51, u16::MAX] {
        let mut p = projection();
        p.first_page.limit = limit;
        assert!(p.validate(false).is_err());
    }
    for metrics in [vec![Metric::Cpu, Metric::Cpu], vec![Metric::Cpu; 4]] {
        assert!(InitialProjection {
            total_metrics: metrics,
            ..Default::default()
        }
        .validate(true)
        .is_err());
    }
    InitialProjection::default().validate(false).unwrap();
}

#[tokio::test]
async fn initial_ready_reads_authorize_once_and_use_only_bound_children() {
    for admin in [false, true] {
        let store = Arc::new(Store::new("", admin));
        let service = MetricsService::new(store.clone());
        let result = service
            .initial_report(&caller(admin), admin, REPORT, projection(), AT)
            .await
            .unwrap();
        let data = result.initial.unwrap();
        assert_eq!(data.rows.items.len(), 1);
        assert_eq!(data.charts.len(), 3);
        assert_eq!(
            *store.calls.lock().unwrap(),
            [
                "report",
                "snapshot",
                "authorize",
                "page",
                "series",
                "series",
                "series",
                "snapshot"
            ]
        );
        assert_eq!(
            data.rows.items[0].owner.display_name.as_deref(),
            Some("Current public name")
        );
        store.revoked.store(true, Ordering::SeqCst);
        assert!(matches!(
            service
                .initial_report(&caller(admin), admin, REPORT, projection(), AT)
                .await,
            Err(MetricsError::NotFound)
        ));
        assert_eq!(
            store
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|&&s| s == "authorize")
                .count(),
            2
        );
        assert_eq!(
            store
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|&&s| s == "page")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn invalid_initial_inputs_and_capabilities_refuse_before_store_access() {
    let store = Arc::new(Store::new("", false));
    let service = MetricsService::new(store.clone());
    for fault in ["limit", "duplicate", "owner-sort", "id", "capability"] {
        let mut p = projection();
        let mut who = caller(false);
        let mut id = REPORT;
        match fault {
            "limit" => p.first_page.limit = 51,
            "duplicate" => p.total_metrics.push(Metric::Cpu),
            "owner-sort" => p.first_page.sort = Sort::Owner,
            "id" => id = "other",
            "capability" => who.authorization = Authorization::new([]),
            _ => unreachable!(),
        }
        assert!(
            service
                .initial_report(&who, false, id, p, AT)
                .await
                .is_err(),
            "{fault}"
        );
    }
    assert!(store.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn initial_pending_returns_only_descriptor_after_current_authority() {
    let store = Arc::new(Store::new("pending", false));
    let service = MetricsService::new(store.clone());
    let response = service
        .initial_report(&caller(false), false, REPORT, projection(), AT)
        .await
        .unwrap();
    let wire = serde_json::to_value(response).unwrap();
    assert_eq!(wire.as_object().unwrap().len(), 1);
    assert_eq!(wire["report"]["status"], "pending");
    assert_eq!(*store.calls.lock().unwrap(), ["report", "authorize"]);
}

#[tokio::test]
async fn incomplete_or_mismatched_initial_children_never_return_partial_success() {
    for fault in [
        "rows-report",
        "rows-as-of",
        "rows-count",
        "rows-missing",
        "rows-extra",
        "rows-owner",
        "series-report",
        "series-metric",
        "series-unit",
        "series-group",
        "series-points",
        "series-range",
        "page-error",
        "series-error",
        "generation",
        "snapshot-lost",
        "zero-generation",
        "unsupported",
        "scope",
        "report-id",
        "descriptor-range",
        "descriptor-step",
        "failed",
        "malformed-pending",
    ] {
        let store = Arc::new(Store::new(fault, false));
        let service = MetricsService::new(store.clone());
        assert!(
            service
                .initial_report(&caller(false), false, REPORT, projection(), AT)
                .await
                .is_err(),
            "{fault}"
        );
    }
}

#[tokio::test]
async fn allocated_only_initial_view_does_not_read_charts() {
    let store = Arc::new(Store::new("", false));
    let service = MetricsService::new(store.clone());
    let result = service
        .initial_report(
            &caller(false),
            false,
            REPORT,
            InitialProjection::default(),
            AT,
        )
        .await
        .unwrap();
    assert!(result.initial.unwrap().charts.is_empty());
    assert!(!store.calls.lock().unwrap().contains(&"series"));
}

#[tokio::test(start_paused = true)]
async fn expiry_while_initial_children_are_read_refuses_the_whole_result() {
    let store = Arc::new(Store::new("expires", false));
    let service = MetricsService::new(store);
    assert!(matches!(
        service
            .initial_report(&caller(false), false, REPORT, projection(), AT)
            .await,
        Err(MetricsError::Expired)
    ));
}

use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use serde_json::json;

#[path = "fixture.rs"]
mod fixture;
use fixture::{flags, row, Manager, Provider, Reply};

#[test]
fn lifecycle_mapping_only_recognizes_provider_running_and_pending() {
    use layer2_core::RuntimeStatus;
    for (state, expected) in [
        ("running", RuntimeStatus::Ready),
        ("Running", RuntimeStatus::Ready),
        ("RUNNING", RuntimeStatus::Ready),
        ("pending", RuntimeStatus::Starting),
        ("Pending", RuntimeStatus::Starting),
        ("failed", RuntimeStatus::Unavailable),
        ("Failed", RuntimeStatus::Unavailable),
        ("stopped", RuntimeStatus::Unavailable),
        ("suspended", RuntimeStatus::Unavailable),
        ("terminating", RuntimeStatus::Unavailable),
        ("unknown", RuntimeStatus::Unavailable),
        ("ready", RuntimeStatus::Unavailable),
        ("new-provider-state", RuntimeStatus::Unavailable),
        ("running ", RuntimeStatus::Unavailable),
        ("", RuntimeStatus::Unavailable),
    ] {
        assert_eq!(super::lifecycle_status(state), expected, "{state}");
    }
}

#[tokio::test]
async fn both_admin_compositions_read_complete_provider_inventory() {
    let provider = Provider::new(Reply::Complete(vec![]));
    let managers = [
        Manager::new("manager-a", provider.clone(), flags(Some("true")).unwrap()),
        Manager::new("manager-b", provider.clone(), flags(Some("true")).unwrap()),
    ];
    let before = layer2_core::now_rfc3339();
    for manager in &managers {
        let (status, body) = manager.get(true).await;
        assert_eq!(status, StatusCode::OK, "{} empty: {body}", manager.source);
        assert_eq!(body["runtimes"], json!([]), "{}", manager.source);
    }
    *provider.reply.lock().unwrap() = Reply::Complete(vec![
        row("b", 3, "Running"),
        row("a", 1, "Pending"),
        row("c", 4, "Failed"),
    ]);
    for manager in &managers {
        let (status, body) = manager.get(true).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{} populated: {body}",
            manager.source
        );
        assert_eq!(
            body["runtimes"],
            json!([
                {"runtime_id":"runtime-a","generation":1,"owner_reference":"sbx_a","status":"starting"},
                {"runtime_id":"runtime-b","generation":3,"owner_reference":"sbx_b","status":"ready"},
                {"runtime_id":"runtime-c","generation":4,"owner_reference":"sbx_c","status":"unavailable"},
            ]),
            "{}",
            manager.source
        );
        let observed = body["observed_at"].as_str().unwrap();
        assert!(
            before.as_str() <= observed && observed <= layer2_core::now_rfc3339().as_str(),
            "{} observation time",
            manager.source
        );
        assert_eq!(
            body.as_object().unwrap().len(),
            2,
            "{} public schema",
            manager.source
        );
        eprintln!(
            "{}: empty and populated provider inventory passed",
            manager.source
        );
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn authorization_and_default_off_refuse_before_provider_dispatch() {
    let provider = Provider::new(Reply::Complete(vec![row("a", 1, "running")]));
    for source in ["manager-a", "manager-b"] {
        for value in [None, Some("false"), Some("true")] {
            let manager = Manager::new(source, provider.clone(), flags(value).unwrap());
            let (status, _) = manager.get(false).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{source} flag={value:?}");
            if value != Some("true") {
                let (status, body) = manager.get(true).await;
                assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{source}: {body}");
                assert_eq!(body["code"], "operator_service_unavailable", "{source}");
            }
        }
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn inventory_flag_requires_an_explicit_strict_boolean() {
    assert!(!flags(None).unwrap().admin_runtime_inventory);
    assert!(!flags(Some("false")).unwrap().admin_runtime_inventory);
    assert!(flags(Some("true")).unwrap().admin_runtime_inventory);
    for value in ["", "1", "TRUE", "true "] {
        assert!(flags(Some(value)).is_err(), "{value}");
    }
}

#[tokio::test]
async fn invalid_or_unavailable_provider_inventory_never_becomes_empty_success() {
    let mut no_owner = row("a", 1, "running");
    no_owner.sandbox_id.clear();
    let mut incomplete = row("a", 1, "running");
    incomplete.image_sha256.clear();
    let mut repeated_owner = row("b", 1, "running");
    repeated_owner.sandbox_id = "sbx_a".into();
    let cases = [
        ("unsupported", Reply::Unsupported),
        ("failure", Reply::Failure),
        ("incomplete pages", Reply::Incomplete),
        ("incomplete observation", Reply::Complete(vec![incomplete])),
        ("missing owner", Reply::Complete(vec![no_owner])),
        (
            "zero generation",
            Reply::Complete(vec![row("a", 0, "running")]),
        ),
        (
            "duplicate identity",
            Reply::Complete(vec![row("a", 1, "running"), row("a", 1, "running")]),
        ),
        (
            "reused handle",
            Reply::Complete(vec![row("a", 1, "running"), row("a", 2, "running")]),
        ),
        (
            "repeated owner generation",
            Reply::Complete(vec![row("a", 1, "running"), repeated_owner]),
        ),
    ];
    for (case, reply) in cases {
        let provider = Provider::new(reply);
        for source in ["manager-a", "manager-b"] {
            let manager = Manager::new(source, provider.clone(), flags(Some("true")).unwrap());
            let (status, body) = manager.get(true).await;
            assert_eq!(
                status,
                StatusCode::SERVICE_UNAVAILABLE,
                "{source}/{case}: {body}"
            );
            assert_eq!(
                body["code"], "operator_service_unavailable",
                "{source}/{case}"
            );
            assert!(body.get("runtimes").is_none(), "{source}/{case}");
        }
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2, "{case}");
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_provider_is_bounded_and_cancelled_without_retry() {
    let provider = Provider::new(Reply::Pending);
    for source in ["manager-a", "manager-b"] {
        let manager = Manager::new(source, provider.clone(), flags(Some("true")).unwrap());
        let started = tokio::time::Instant::now();
        let (status, body) =
            tokio::time::timeout(std::time::Duration::from_secs(11), manager.get(true))
                .await
                .expect("inventory must finish within ten seconds");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{source}: {body}");
        assert_eq!(body["code"], "operator_service_unavailable", "{source}");
        assert_eq!(
            started.elapsed(),
            std::time::Duration::from_secs(10),
            "{source}"
        );
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(provider.cancelled.load(Ordering::SeqCst), 2);
}

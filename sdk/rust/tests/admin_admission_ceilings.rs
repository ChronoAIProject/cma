#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;

use cma_sdk::{
    Error,
    admin::{AdmissionCeilings, ExistingReserve, ReplaceAdmissionCeilingsRequest},
};
use serde_json::{Value, json};
use std::time::Duration;
use wire::WireServer;

fn policy() -> AdmissionCeilings {
    AdmissionCeilings {
        registrations: Some(10),
        retained_homes: Some(5),
        active_sessions: Some(3),
        owner_active_sessions: Some(1),
        existing_reserve: ExistingReserve {
            retained_homes: Some(1),
            active_sessions: Some(1),
        },
    }
}

fn request() -> ReplaceAdmissionCeilingsRequest {
    ReplaceAdmissionCeilingsRequest {
        ceilings: policy(),
        reason: "Bound public capacity".into(),
        expected_revision: 0,
    }
}

fn change(revision: u64) -> Value {
    json!({"revision":revision,"at":100,"actor":"operator","reason":"Bound public capacity",
        "old_ceilings":AdmissionCeilings::default(),"new_ceilings":policy()})
}

fn state() -> Value {
    json!({"revision":1,"ceilings":policy(),"last_change":change(1),"writes_available":true})
}

fn read() -> Value {
    json!({"revision":7,"operator_stop":null,"last_change":null,"history":[],
        "writes_available":false,"ceilings":state(),"effective_limits":policy(),
        "capacity":{"registrations":{"observed":10,"bound":10,"saturated":true},
            "retained_homes":{"observed":2,"bound":5,"saturated":false},
            "pending_launch_homes":1,"active_sessions":null},"ceilings_history":[change(1)]})
}

#[tokio::test]
async fn replacement_and_read_preserve_independent_cas_policy_counts_and_history() {
    let server = WireServer::new().await;
    let body = request();
    server.expect(wire::json(
        "PUT",
        "/api/v1/admin/admission/ceilings",
        Some(serde_json::to_value(&body).unwrap()),
        200,
        state(),
        true,
    ));
    let replaced = server
        .client
        .admin()
        .replace_admission_ceilings(&body)
        .await
        .unwrap();
    assert_eq!(replaced.revision, 1);
    assert_eq!(replaced.ceilings, policy());
    let last = replaced.last_change.unwrap();
    assert_eq!(last.old_ceilings, AdmissionCeilings::default());
    assert_eq!(last.new_ceilings, policy());
    assert_eq!(last.actor, "operator");
    assert_eq!(last.at, 100);
    assert_eq!(last.reason, body.reason);

    let mut response = read();
    // The deployment cap may be lower than the configured existing-consumer reserve.
    response["effective_limits"]["retained_homes"] = json!(0);
    server.expect(wire::json(
        "GET",
        "/api/v1/admin/admission",
        None,
        200,
        response,
        true,
    ));
    let read = server.client.admin().read_admission().await.unwrap();
    assert_eq!(read.control.revision, 7);
    assert!(!read.control.writes_available);
    let ceilings = read.ceilings.unwrap();
    assert_eq!(ceilings.revision, 1);
    assert!(ceilings.writes_available);
    assert_eq!(read.ceilings_history.unwrap()[0], last);
    let effective = read.effective_limits.unwrap();
    assert_eq!(effective.retained_homes, Some(0));
    assert_eq!(effective.existing_reserve.retained_homes, Some(1));
    let counts = read.capacity.unwrap();
    let registrations = counts.registrations.unwrap();
    assert_eq!(
        (
            registrations.observed,
            registrations.bound,
            registrations.saturated
        ),
        (10, 10, true)
    );
    let homes = counts.retained_homes.unwrap();
    assert_eq!(
        (homes.observed, homes.bound, homes.saturated),
        (2, 5, false)
    );
    assert_eq!(counts.pending_launch_homes, 1);
    assert!(counts.active_sessions.is_none());
    assert_eq!(server.request_count(), 2);
    server.assert_drained();
}

#[tokio::test]
async fn unset_zero_and_unsigned_boundaries_remain_distinct() {
    let server = WireServer::new().await;
    for ceilings in [
        AdmissionCeilings::default(),
        AdmissionCeilings {
            registrations: Some(u32::MAX),
            retained_homes: Some(0),
            active_sessions: Some(u32::MAX),
            owner_active_sessions: Some(0),
            existing_reserve: ExistingReserve {
                retained_homes: Some(0),
                active_sessions: Some(u32::MAX),
            },
        },
        AdmissionCeilings {
            existing_reserve: ExistingReserve {
                retained_homes: Some(u32::MAX),
                active_sessions: Some(1),
            },
            ..Default::default()
        },
    ] {
        let body = ReplaceAdmissionCeilingsRequest {
            ceilings: ceilings.clone(),
            reason: "é".repeat(256),
            expected_revision: u64::MAX,
        };
        server.expect(wire::json("PUT", "/api/v1/admin/admission/ceilings",
            Some(serde_json::to_value(&body).unwrap()),200,
            json!({"revision":u64::MAX,"ceilings":ceilings,"last_change":null,"writes_available":true}),true));
        assert_eq!(
            server
                .client
                .admin()
                .replace_admission_ceilings(&body)
                .await
                .unwrap()
                .ceilings,
            ceilings
        );
    }
    let mut response = read();
    response["capacity"] = json!({"registrations":null,"retained_homes":null,
        "active_sessions":null,"pending_launch_homes":u64::MAX});
    server.expect(wire::json(
        "GET",
        "/api/v1/admin/admission",
        None,
        200,
        response,
        true,
    ));
    let counts = server
        .client
        .admin()
        .read_admission()
        .await
        .unwrap()
        .capacity
        .unwrap();
    assert_eq!(counts.pending_launch_homes, u64::MAX);
    assert!(counts.registrations.is_none());
    assert!(counts.retained_homes.is_none());
    assert!(counts.active_sessions.is_none());
    server.assert_drained();
}

#[tokio::test]
async fn invalid_reasons_and_reserves_refuse_before_http() {
    let server = WireServer::new().await;
    for reason in [
        "".into(),
        "  ".into(),
        "bad\nreason".into(),
        "bad\u{0085}reason".into(),
        "x".repeat(513),
        "é".repeat(257),
    ] {
        let mut body = request();
        body.reason = reason;
        assert!(matches!(
            server
                .client
                .admin()
                .replace_admission_ceilings(&body)
                .await,
            Err(Error::Validation { .. })
        ));
    }
    for field in ["retained_homes", "active_sessions"] {
        let mut value = serde_json::to_value(request()).unwrap();
        value["ceilings"]["existing_reserve"][field] = json!(6);
        let body = serde_json::from_value(value).unwrap();
        assert!(matches!(
            server
                .client
                .admin()
                .replace_admission_ceilings(&body)
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn typed_refusals_preserve_problem_metadata_without_retry() {
    let server = WireServer::new().await;
    for (status, code) in [
        (401, "not_authenticated"),
        (403, "forbidden"),
        (409, "admission_ceilings_conflict"),
        (422, "invalid_request"),
        (503, "admission_ceilings_writes_unavailable"),
        (503, "admission_ceilings_unavailable"),
    ] {
        let mut expected = wire::json(
            "PUT",
            "/api/v1/admin/admission/ceilings",
            Some(serde_json::to_value(request()).unwrap()),
            status,
            json!({"status":status,"code":code,"detail":"Read current state before retrying.",
                "field":"expected_revision"}),
            true,
        );
        expected
            .response_headers
            .insert("retry-after".into(), "60".into());
        expected
            .response_headers
            .insert("x-request-id".into(), "ceilings-refusal".into());
        server.expect(expected);
        let error = server
            .client
            .admin()
            .replace_admission_ceilings(&request())
            .await
            .unwrap_err();
        assert_eq!(error.extension("field"), Some("expected_revision"));
        assert!(
            matches!(error,Error::Api{status:actual,code:Some(actual_code),request_id:Some(id),
            retry_after:Some(delay),..} if actual==status && actual_code==code
            && id=="ceilings-refusal" && delay==Duration::from_secs(60))
        );
    }
    assert_eq!(server.request_count(), 6);
    server.assert_drained();
}

#[tokio::test]
async fn malformed_capacity_and_history_responses_fail_closed() {
    let server = WireServer::new().await;
    let mut cases = Vec::new();
    for pointer in [
        "/ceilings",
        "/effective_limits",
        "/capacity",
        "/ceilings_history",
        "/ceilings/last_change",
        "/capacity/registrations",
        "/capacity/retained_homes",
        "/capacity/active_sessions",
        "/capacity/pending_launch_homes",
    ] {
        let mut invalid = read();
        let (parent, field) = pointer.rsplit_once('/').unwrap();
        invalid
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        // Missing top-level additions are supported for predecessor servers.
        if !parent.is_empty() {
            cases.push(invalid);
        }
    }
    for (pointer, value) in [
        ("/ceilings", json!(null)),
        ("/effective_limits", json!(null)),
        ("/capacity", json!(null)),
        ("/ceilings_history", json!(null)),
        ("/ceilings/revision", json!(-1)),
        ("/ceilings/writes_available", json!("true")),
        ("/ceilings/ceilings/registrations", json!(4294967296_u64)),
        ("/ceilings/ceilings/active_sessions", json!(-1)),
        (
            "/ceilings/ceilings/existing_reserve/retained_homes",
            json!(6),
        ),
        ("/capacity/pending_launch_homes", json!(-1)),
        ("/capacity/registrations/observed", json!(-1)),
        ("/capacity/registrations/bound", json!(1.5)),
        ("/capacity/registrations/saturated", json!("true")),
        ("/ceilings/last_change/reason", json!("\n")),
        (
            "/ceilings/last_change/old_ceilings/existing_reserve/active_sessions",
            json!(-1),
        ),
    ] {
        let mut invalid = read();
        *invalid.pointer_mut(pointer).unwrap() = value;
        cases.push(invalid);
    }
    for history in [
        json!([change(1), change(2)]),
        json!([change(3), change(1)]),
        json!([change(1), change(1)]),
        json!((1..=21).rev().map(change).collect::<Vec<_>>()),
        json!([{"revision":1}]),
    ] {
        let mut invalid = read();
        invalid["ceilings_history"] = history;
        cases.push(invalid);
    }
    for invalid in cases {
        server.expect(wire::json(
            "GET",
            "/api/v1/admin/admission",
            None,
            200,
            invalid,
            true,
        ));
        assert!(matches!(
            server.client.admin().read_admission().await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

#[tokio::test]
async fn malformed_replacement_outcomes_are_not_retried() {
    let server = WireServer::new().await;
    for (status, response) in [
        (200, json!({})),
        (
            200,
            json!({"revision":1,"ceilings":null,
        "last_change":null,"writes_available":true}),
        ),
        (202, state()),
    ] {
        server.expect(wire::json(
            "PUT",
            "/api/v1/admin/admission/ceilings",
            Some(serde_json::to_value(request()).unwrap()),
            status,
            response,
            true,
        ));
        assert!(matches!(
            server
                .client
                .admin()
                .replace_admission_ceilings(&request())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    assert_eq!(server.request_count(), 3);
    server.assert_drained();
}

#[tokio::test]
async fn ceiling_history_accepts_twenty_and_predecessor_reads_remain_supported() {
    let server = WireServer::new().await;
    let mut response = read();
    response["ceilings_history"] = json!((1..=20).rev().map(change).collect::<Vec<_>>());
    server.expect(wire::json(
        "GET",
        "/api/v1/admin/admission",
        None,
        200,
        response,
        true,
    ));
    let read = server.client.admin().read_admission().await.unwrap();
    let history = read.ceilings_history.unwrap();
    assert_eq!(history.len(), 20);
    assert_eq!(history[0].revision, 20);
    assert_eq!(history[19].revision, 1);
    server.expect(wire::json("GET","/api/v1/admin/admission",None,200,
        json!({"revision":0,"operator_stop":null,"last_change":null,"writes_available":false,"history":[]}),true));
    let old = server.client.admin().read_admission().await.unwrap();
    assert!(
        old.ceilings.is_none()
            && old.effective_limits.is_none()
            && old.capacity.is_none()
            && old.ceilings_history.is_none()
    );
    server.assert_drained();
}

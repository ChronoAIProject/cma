#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;

use cma_sdk::{Error, admin::ChangeAdmissionRequest};
use serde_json::{Value, json};
use wire::WireServer;

fn change(revision: u64) -> Value {
    json!({"revision":revision,"at":100,"actor":"operator","reason":"incident",
        "old_stop":null,"new_stop":{"since":100,"revision":revision,
        "actor":"operator","reason":"incident"}})
}

fn view() -> Value {
    json!({"revision":0,"operator_stop":null,"last_change":null,"writes_available":false})
}

fn request(reason: &str, expected_revision: u64) -> ChangeAdmissionRequest {
    ChangeAdmissionRequest {
        reason: reason.into(),
        expected_revision,
    }
}

#[tokio::test]
async fn admission_read_stop_resume_preserve_cas_and_history() {
    let server = WireServer::new().await;
    let mut initial = view();
    initial["history"] = json!([]);
    server.expect(wire::json(
        "GET",
        "/api/v1/admin/admission",
        None,
        200,
        initial,
        true,
    ));
    let read = server.client.admin().read_admission().await.unwrap();
    assert_eq!(read.control.revision, 0);
    assert!(!read.control.writes_available);
    assert!(read.history.is_empty());
    let stopped = json!({"revision":1,"operator_stop":change(1)["new_stop"],
        "last_change":change(1),"writes_available":true});
    server.expect(wire::json(
        "POST",
        "/api/v1/admin/admission/stop",
        Some(json!({"reason":"incident","expected_revision":0})),
        200,
        stopped.clone(),
        true,
    ));
    let accepted = server
        .client
        .admin()
        .stop_admission(&request("incident", 0))
        .await
        .unwrap();
    assert_eq!(accepted.operator_stop.unwrap().reason, "incident");
    let resumed_change = json!({"revision":2,"at":101,"actor":"operator","reason":"resolved",
        "old_stop":change(1)["new_stop"],"new_stop":null});
    let resumed = json!({"revision":2,"operator_stop":null,
        "last_change":resumed_change,"writes_available":true});
    server.expect(wire::json(
        "POST",
        "/api/v1/admin/admission/resume",
        Some(json!({"reason":"resolved","expected_revision":1})),
        200,
        resumed.clone(),
        true,
    ));
    assert!(
        server
            .client
            .admin()
            .resume_admission(&request("resolved", 1))
            .await
            .unwrap()
            .operator_stop
            .is_none()
    );
    let mut observed = resumed;
    observed["history"] = json!([resumed_change, change(1)]);
    server.expect(wire::json(
        "GET",
        "/api/v1/admin/admission",
        None,
        200,
        observed,
        true,
    ));
    let read = server.client.admin().read_admission().await.unwrap();
    assert_eq!(read.history.len(), 2);
    assert_eq!(read.history[0], read.control.last_change.unwrap());
    assert_eq!(read.history[1].revision, 1);
    assert_eq!(server.request_count(), 4);
    server.assert_drained();
}

#[tokio::test]
async fn invalid_reasons_never_send_and_utf8_byte_boundary_is_accepted() {
    let server = WireServer::new().await;
    for reason in [
        "".into(),
        "  ".into(),
        "bad\nreason".into(),
        "bad\u{0085}reason".into(),
        "a".repeat(513),
        "é".repeat(257),
    ] {
        let body = request(&reason, 0);
        assert!(matches!(
            server.client.admin().stop_admission(&body).await,
            Err(Error::Validation { .. })
        ));
        assert!(matches!(
            server.client.admin().resume_admission(&body).await,
            Err(Error::Validation { .. })
        ));
    }
    assert_eq!(server.request_count(), 0);
    // The public request schema accepts u64; CAS interpretation belongs to the server.
    let body = request(&"é".repeat(256), u64::MAX);
    server.expect(wire::json(
        "POST",
        "/api/v1/admin/admission/stop",
        Some(serde_json::to_value(&body).unwrap()),
        200,
        view(),
        true,
    ));
    server.client.admin().stop_admission(&body).await.unwrap();
    assert_eq!(server.request_count(), 1);
    server.assert_drained();
}

#[tokio::test]
async fn typed_refusals_are_preserved_without_implicit_retry() {
    let server = WireServer::new().await;
    for (status, code) in [
        (403, "not_an_operator"),
        (409, "admission_control_conflict"),
        (503, "admission_control_writes_unavailable"),
        (503, "admission_control_unavailable"),
    ] {
        for operation in ["stop", "resume"] {
            let mut expected = wire::json(
                "POST",
                &format!("/api/v1/admin/admission/{operation}"),
                Some(json!({"reason":"incident","expected_revision":1})),
                status,
                json!({"status":status,"code":code,"detail":"Read the current state before retrying."}),
                true,
            );
            expected
                .response_headers
                .insert("retry-after".into(), "60".into());
            server.expect(expected);
            let body = request("incident", 1);
            let result = if operation == "stop" {
                server.client.admin().stop_admission(&body).await
            } else {
                server.client.admin().resume_admission(&body).await
            };
            assert!(
                matches!(result, Err(Error::Api { status: actual, code: Some(actual_code), .. })
                if actual == status && actual_code == code)
            );
        }
    }
    assert_eq!(server.request_count(), 8);
    server.assert_drained();
}

#[tokio::test]
async fn malformed_admission_responses_fail_closed() {
    let server = WireServer::new().await;
    let mut cases = Vec::new();
    cases.push(view()); // History must be present, even when empty.
    for history in [
        json!(null),
        json!([{"revision":1}]),
        json!([change(1), change(2)]),
        json!([change(3), change(1)]),
        json!([change(1), change(1)]),
        json!((1..=21).rev().map(change).collect::<Vec<_>>()),
    ] {
        let mut invalid = view();
        invalid["history"] = history;
        cases.push(invalid);
    }
    for (field, value) in [("revision", json!(-1)), ("writes_available", json!("true"))] {
        let mut invalid = view();
        invalid["history"] = json!([]);
        invalid[field] = value;
        cases.push(invalid);
    }
    for reason in [" ".to_owned(), "x\n".to_owned(), "é".repeat(257)] {
        let mut invalid = view();
        let mut malformed = change(1);
        malformed["reason"] = json!(reason);
        invalid["history"] = json!([malformed]);
        cases.push(invalid);
    }
    for field in ["operator_stop", "last_change"] {
        let mut invalid = view();
        invalid.as_object_mut().unwrap().remove(field);
        invalid["history"] = json!([]);
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

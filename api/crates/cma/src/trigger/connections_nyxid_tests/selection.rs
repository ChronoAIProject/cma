use super::*;
use crate::auth::keys::{scope_plan_fixture, KeyIssuer};
use crate::dependency_test_support::{finish, Exchange, Peer};
use axum::http::Method;

fn assert_request(request: &Exchange, method: Method, path: &str) {
    assert_eq!(request.method, method);
    assert_eq!(request.uri.path(), path);
    assert!(request.uri.query().is_none());
    assert_eq!(
        request.headers[header::AUTHORIZATION],
        format!("Bearer {VISITOR_TOKEN}")
    );
}

async fn assert_admission_and_issuance_select(
    admission_rows: Value,
    issuance_rows: Value,
    wanted: &str,
    selected: &str,
) {
    let selected_row = admission_rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == selected)
        .unwrap()
        .clone();
    let mut peer = Peer::start().await;
    let auth = crate::auth::nyxid_contract_tests::auth(&peer.origin);
    let broker = NyxidServiceConnectionBroker::new(
        auth.http.clone(),
        &peer.origin,
        "https://nyx.example.test",
        Tokens::available(),
    )
    .unwrap();
    let service = ServiceId::new(wanted);
    let requirement_service = service.clone();
    let admission = tokio::spawn(async move {
        broker
            .requirements(&SubjectId::new("visitor-one"), &[requirement_service])
            .await
    });
    let listing = peer.next().await;
    assert_request(&listing, Method::GET, "/api/v1/user-services");
    listing.json(admission_rows);
    let probe = peer.next().await;
    assert_request(&probe, Method::GET, &format!("/api/v1/keys/{selected}"));
    probe.json(json!({
        "id":selected,"slug":"api-github","is_active":true,"status":"active",
        "connected":true,"credential_missing":false,
        "credential_source":selected_row["credential_source"]
    }));
    let readiness = peer.next().await;
    assert_request(&readiness, Method::GET, "/api/v1/assistant/readiness");
    readiness.json(json!({
        "revision":"nyxid-assistant-readiness.v1",
        "capabilities":[{"capabilityId":"api-github","connectionState":"connected"}]
    }));
    let requirements = finish(admission).await.unwrap();
    assert_eq!(requirements.len(), 1);
    assert_eq!(requirements[0].state, ServiceRequirementState::Satisfied);

    let issuer = KeyIssuer::new(auth.http.clone(), &peer.origin, "contract");
    let mint = tokio::spawn(async move {
        issuer
            .mint(
                "sbx_selection",
                &SubjectId::new("visitor-one"),
                VISITOR_TOKEN,
                &[service],
            )
            .await
    });
    let listing = peer.next().await;
    assert_request(&listing, Method::GET, "/api/v1/user-services");
    listing.json(issuance_rows);
    let scope = peer.next().await;
    assert_request(&scope, Method::POST, "/api/v1/api-keys/scope-plan");
    let scope_body: Value = serde_json::from_slice(&scope.body).unwrap();
    let plan = scope_plan_fixture("visitor-one", &scope_body, &["contract-node"]);
    scope.json(plan.clone());
    let request = peer.next().await;
    assert_request(&request, Method::POST, "/api/v1/api-keys");
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        body["allowed_service_ids"],
        scope_body["selected_service_ids"]
    );
    assert_eq!(body["allowed_node_ids"], json!(["contract-node"]));
    assert_eq!(body["scope_plan_digest"], plan["normalized_grant_digest"]);
    assert_eq!(body["scopes"], "proxy read");
    for flag in [
        "allow_all_services",
        "allow_all_nodes",
        "allow_auto_connected_services",
    ] {
        assert_eq!(body[flag], false);
    }
    request.json(json!({
        "id":"contract-provider-key","full_key":"contract-scoped-fixture",
        "scopes":"proxy read","state_version":7,
        "allowed_service_ids":body["allowed_service_ids"],
        "allowed_node_ids":body["allowed_node_ids"],
        "allow_all_services":false,"allow_all_nodes":false,"allow_auto_connected_services":false
    }));
    let issued = finish(mint).await.unwrap();
    assert_eq!(peer.count_route("GET", "/api/v1/user-services"), 2);
    assert_eq!(peer.count_route("POST", "/api/v1/api-keys/scope-plan"), 1);
    assert_eq!(peer.count_route("POST", "/api/v1/api-keys"), 1);
    assert_eq!(peer.count(), 6);
    assert_eq!(
        scope_body["selected_service_ids"],
        json!([selected]),
        "issuance must retain the exact connection that admission probed"
    );
    assert!(issued.matches_grant(&[selected.into()], &["contract-node".into()]));
}

fn organization(id: &str, allowed: bool) -> Value {
    json!({"id":id,"slug":"api-github","is_active":true,
        "credential_source":{"type":"org","allowed":allowed}})
}

#[tokio::test]
async fn organization_list_order_cannot_change_the_admitted_issuance_binding() {
    let rows = [organization("org-z-1", true), organization("org-a-1", true)];
    for admission_rows in [json!(rows), json!([rows[1], rows[0]])] {
        for issuance_rows in [json!(rows), json!([rows[1], rows[0]])] {
            assert_admission_and_issuance_select(
                admission_rows.clone(),
                issuance_rows,
                "api-github",
                "org-a-1",
            )
            .await;
        }
    }
}

#[tokio::test]
async fn exact_connection_id_precedes_another_connections_matching_slug() {
    let mut shadow = organization("org-a-shadow", true);
    shadow["slug"] = "org-z-1".into();
    let rows = json!([shadow, organization("org-z-1", true)]);
    assert_admission_and_issuance_select(rows.clone(), rows, "org-z-1", "org-z-1").await;
}

#[tokio::test]
async fn personal_priority_and_inactive_fallback_match_between_admission_and_issuance() {
    for active in [true, false] {
        let personal = json!({"id":"personal-connection","slug":"api-github",
            "is_active":active,"credential_source":{"type":"personal"}});
        let org = organization("org-connection", true);
        let expected = if active {
            "personal-connection"
        } else {
            "org-connection"
        };
        for rows in [json!([org, personal]), json!([personal, org])] {
            assert_admission_and_issuance_select(rows.clone(), rows, "api-github", expected).await;
        }
    }
}

#[tokio::test]
async fn ambiguous_connection_ids_are_unknown_without_credential_probes() {
    for other_slug in ["api-github", "another-service"] {
        let provider = Provider::start().await;
        provider.state.set_services_reply(Reply::json(json!([
            {"id":"same-connection","slug":"api-github","is_active":true},
            {"id":"same-connection","slug":other_slug,"is_active":true}
        ])));
        let requirements = broker(&provider, Tokens::available())
            .requirements(
                &SubjectId::new("visitor-one"),
                &[
                    ServiceId::new("same-connection"),
                    ServiceId::new("api-github"),
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            requirements.iter().map(|r| r.state).collect::<Vec<_>>(),
            vec![ServiceRequirementState::Unknown; 2]
        );
        assert!(provider.state.credential_reads.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn issuance_cannot_substitute_a_denied_organization_for_the_admitted_connection() {
    let rows = [
        organization("org-a-denied", false),
        organization("org-b-allowed", true),
    ];
    for issuance_rows in [json!(rows), json!([rows[1], rows[0]])] {
        assert_admission_and_issuance_select(
            json!(rows),
            issuance_rows,
            "api-github",
            "org-b-allowed",
        )
        .await;
    }
}

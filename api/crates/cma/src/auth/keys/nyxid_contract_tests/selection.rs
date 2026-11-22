use super::*;

async fn assert_preparation_refuses(rows: Value, wanted: &str) {
    let mut peer = Peer::start().await;
    let issuer = issuer(&peer.origin);
    let service = ServiceId::new(wanted);
    let requested = service.clone();
    let mut preparation = tokio::spawn(async move {
        issuer
            .prepare_mint(
                "sbx_selection",
                &layer2_core::SubjectId::new("contract-owner"),
                "contract-owner-bearer",
                &[requested],
            )
            .await
            .map(|_| ())
    });
    let listing = peer.next().await;
    assert_stage(&listing, 0);
    listing.json(rows);
    let result = tokio::select! {
        result = &mut preparation => result.unwrap(),
        unexpected = peer.next() => {
            unexpected.respond(403, "synthetic selection refused");
            finish(preparation).await
        }
    };
    assert_eq!(
        peer.count_route("POST", "/api/v1/api-keys/scope-plan"),
        0,
        "an unknown, denied, or ambiguous binding must stop before scope preparation"
    );
    assert_eq!(peer.count_route("POST", "/api/v1/api-keys"), 0);
    assert_eq!(peer.count(), 1);
    assert!(matches!(result, Err(KeyIssuerError::ServiceNotGranted(id)) if id == service));
}

#[tokio::test]
async fn unknown_exact_connection_refuses_before_scope_plan_and_mint() {
    assert_preparation_refuses(
        json!([{"id":"known-connection","slug":"api-github","is_active":true}]),
        "unknown-connection",
    )
    .await;
}

#[tokio::test]
async fn duplicate_exact_connections_refuse_before_scope_plan_and_mint() {
    for active in [true, false] {
        assert_preparation_refuses(
            json!([
                {"id":"same-connection","slug":"api-github","is_active":true},
                {"id":"same-connection","slug":"another-service","is_active":active}
            ]),
            "same-connection",
        )
        .await;
    }
}

#[tokio::test]
async fn a_slug_cannot_admit_a_duplicate_selected_connection_id() {
    for other_slug in ["api-github", "another-service"] {
        assert_preparation_refuses(
            json!([
                {"id":"same-connection","slug":"api-github","is_active":true},
                {"id":"same-connection","slug":other_slug,"is_active":true}
            ]),
            "api-github",
        )
        .await;
    }
}

#[tokio::test]
async fn denied_exact_connection_cannot_fall_back_to_an_allowed_connection() {
    assert_preparation_refuses(
        json!([
            {"id":"denied-connection","slug":"api-github","is_active":true,
                "credential_source":{"type":"org","allowed":false}},
            {"id":"allowed-connection","slug":"api-github","is_active":true,
                "credential_source":{"type":"org","allowed":true}}
        ]),
        "denied-connection",
    )
    .await;
}

#[tokio::test]
async fn inactive_or_unknown_activity_cannot_prepare_a_key() {
    for active in [json!(false), Value::Null] {
        assert_preparation_refuses(
            json!([{"id":"inactive-connection","slug":"api-github","is_active":active}]),
            "inactive-connection",
        )
        .await;
    }
}

#[tokio::test]
async fn legacy_id_only_connections_keep_their_exact_identity_in_supported_listings() {
    let rows = json!([{"id":"legacy-connection","is_active":true}]);
    for listing in [
        rows.clone(),
        json!({"services":rows}),
        json!({"user_services":rows}),
        json!({"items":rows}),
    ] {
        let mut peer = Peer::start().await;
        let issuer = issuer(&peer.origin);
        let task = tokio::spawn(async move {
            issuer
                .resolves_all(
                    "contract-owner-bearer",
                    &[ServiceId::new("legacy-connection")],
                )
                .await
        });
        let request = peer.next().await;
        assert_stage(&request, 0);
        request.json(listing);
        assert!(finish(task).await.unwrap());
        assert_eq!(peer.count(), 1);
    }
}

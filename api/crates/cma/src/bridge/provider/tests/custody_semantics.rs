use super::*;

async fn verify(f: &Fixture) -> Result<VerifiedProviderBinding, BridgeError> {
    f.provider
        .verify_secret(
            &personal(),
            &SecretValue::new(format!("nyxid_ag_{}", "a".repeat(64))),
            &f.connector,
            &SubjectId::new("person-one"),
            "fixture-human-oauth",
        )
        .await
}

fn possession_replies() -> ProviderReplies {
    let mut rows = replies("person-one", json!({"type":"personal"}));
    rows.insert(
        "/api/v1/users/me/authorization".into(),
        (StatusCode::OK, json!({"id":"person-one","is_active":true})),
    );
    rows.insert(
        "/api/v1/channel-relay/messages/route-one".into(),
        (
            StatusCode::OK,
            json!({"messages":[],"total":0,"page":1,"per_page":1}),
        ),
    );
    rows
}

#[tokio::test]
async fn possession_usage_advances_only_observed_version_and_rechecks_authority() {
    let f = fixture(possession_replies()).await;
    let first = verify(&f).await.unwrap();
    let second = verify(&f).await.unwrap();
    assert_eq!(first.key_state_version, 4);
    assert_eq!(second.key_state_version, 5);
    assert_eq!(first.key_authority, second.key_authority);
    let requests = f.state.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|(_, path, _)| path == "/api/v1/api-keys")
            .count(),
        4
    );
    assert!(requests.iter().all(|(method, _, _)| method == "GET"));
}

#[tokio::test]
async fn authority_changes_during_possession_are_rejected_even_with_increasing_version() {
    for patch in [
        json!({"scopes":"read"}),
        json!({"allowed_service_ids":[]}),
        json!({"allow_all_services":true}),
        json!({"allow_auto_connected_services":true}),
        json!({"allowed_node_ids":["another-node"]}),
        json!({"allow_all_nodes":true}),
        json!({"expires_at":"2099-01-01T00:00:00Z"}),
        json!({"rate_limit_per_second":10}),
        json!({"rate_limit_burst":20}),
        json!({"purpose":"scheduled_invocation"}),
        json!({"scheduled_write_enabled":true}),
        json!({"rotation_predecessor_id":"another-key"}),
        json!({"platform":null}),
        json!({"callback_url":"https://other.example/callback"}),
        json!({"credential_source":{"type":"org","org_id":"org-one","role":"admin","allowed":true}}),
        json!({"is_active":false}),
        json!({"bindings_count":1}),
        json!({"state_version":2}),
    ] {
        let f = fixture(possession_replies()).await;
        *f.state.key_patch_on_probe.lock().unwrap() = Some(patch.clone());
        assert!(verify(&f).await.is_err(), "{patch}");
    }
}

#[tokio::test]
async fn required_permission_fields_cannot_be_silently_defaulted() {
    for field in [
        "allow_auto_connected_services",
        "purpose",
        "scheduled_write_enabled",
    ] {
        let mut rows = possession_replies();
        rows.get_mut("/api/v1/api-keys").unwrap().1["keys"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        let f = fixture(rows).await;
        assert!(verify(&f).await.is_err(), "{field}");
    }
}

#[tokio::test]
async fn retained_verification_rejects_post_probe_changes_without_relisting_before_probe() {
    for patch in [
        json!({"scopes":"read"}),
        json!({"is_active":false}),
        json!({"state_version":1}),
        json!({"callback_url":"https://other.example/callback"}),
        json!({"expires_at":"2000-01-01T00:00:00Z"}),
    ] {
        let f = fixture(possession_replies()).await;
        let imported = verify(&f).await.unwrap();
        f.state.requests.lock().unwrap().clear();
        *f.state.key_patch_on_probe.lock().unwrap() = Some(patch.clone());
        let result = f
            .provider
            .verify_secret_with_authority(
                &personal(),
                &SecretValue::new(format!("nyxid_ag_{}", "a".repeat(64))),
                &f.connector,
                &SubjectId::new("person-one"),
                "fixture-human-oauth",
                Some((&imported.key_authority, imported.key_state_version)),
            )
            .await;
        assert!(result.is_err(), "{patch}");
        let requests = f.state.requests.lock().unwrap();
        assert_eq!(requests.len(), 5);
        let first_wave = &requests[..2];
        assert!(first_wave
            .iter()
            .any(|(_, uri, _)| uri == "/api/v1/users/me/authorization"));
        assert!(first_wave
            .iter()
            .any(|(_, uri, _)| uri.starts_with("/api/v1/channel-relay/messages/")));
    }
}

#[tokio::test]
async fn retained_verification_overlaps_reads_but_inventory_requires_both_to_succeed() {
    for denied_endpoint in [
        None,
        Some("/api/v1/users/me/authorization"),
        Some("/api/v1/channel-relay/messages/route-one"),
    ] {
        let f = fixture(possession_replies()).await;
        let imported = verify(&f).await.unwrap();
        f.state.requests.lock().unwrap().clear();
        if let Some(endpoint) = denied_endpoint {
            f.state.replies.lock().unwrap().get_mut(endpoint).unwrap().0 = StatusCode::FORBIDDEN;
        }
        // Neither response can finish until both requests have arrived. A
        // serial implementation cannot pass this barrier, regardless of IO speed.
        *f.state.verification_barrier.lock().unwrap() =
            Some(Arc::new(tokio::sync::Barrier::new(2)));
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            f.provider.verify_secret_with_authority(
                &personal(),
                &SecretValue::new(format!("nyxid_ag_{}", "a".repeat(64))),
                &f.connector,
                &SubjectId::new("person-one"),
                "fixture-human-oauth",
                Some((&imported.key_authority, imported.key_state_version)),
            ),
        )
        .await
        .expect("independent verification requests must start concurrently");
        assert_eq!(result.is_ok(), denied_endpoint.is_none());
        let requests = f.state.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            if denied_endpoint.is_none() { 5 } else { 2 }
        );
        assert!(requests[..2]
            .iter()
            .any(|(_, uri, _)| uri == "/api/v1/users/me/authorization"));
        assert!(requests[..2]
            .iter()
            .any(|(_, uri, _)| uri.starts_with("/api/v1/channel-relay/messages/")));
        if let Ok(binding) = result {
            assert_eq!(binding.key_authority, imported.key_authority);
            assert!(binding.key_state_version > imported.key_state_version);
        }
    }
}

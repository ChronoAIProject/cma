use super::*;

#[path = "budget_tests.rs"]
mod budget_tests;

const PATH: &str = "/api/v1/admin/settings";
const ADMIN: [Capability; 1] = [Capability::ConfigurePlatform];

#[tokio::test]
async fn measured_visibility_uses_admin_revision_cas_and_audit_on_both_transitions() {
    let fixture = fixture(false);
    let initial = response_json(send(&fixture.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(
        initial["settings"]["usage_reporting"]["measured_visible"],
        false
    );
    for (revision, visible) in [(0, true), (1, false)] {
        let body = json!({"expected_revision": revision, "usage_reporting": {"measured_visible": visible}});
        for grants in [
            vec![],
            vec![Capability::UseOwnSandboxes],
            vec![Capability::ReadAllSandboxes],
        ] {
            let denied = send(
                &fixture.app,
                Method::PATCH,
                PATH,
                Some(body.clone()),
                grants,
            )
            .await;
            assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        }
        let saved = send(&fixture.app, Method::PATCH, PATH, Some(body.clone()), ADMIN).await;
        assert_eq!(saved.status(), StatusCode::OK);
        let saved = response_json(saved).await;
        assert_eq!(saved["revision"], revision + 1);
        assert_eq!(
            saved["settings"]["usage_reporting"]["measured_visible"],
            visible
        );
        let conflict = send(&fixture.app, Method::PATCH, PATH, Some(body), ADMIN).await;
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(response_json(conflict).await["code"], "settings_conflict");
        let current = response_json(send(&fixture.app, Method::GET, PATH, None, ADMIN).await).await;
        assert_eq!(current, saved);
    }
    let records = fixture.activity.records.lock().unwrap();
    let writes: Vec<_> = records
        .iter()
        .filter(|record| record.action == ActivityAction::Configure)
        .collect();
    assert_eq!(writes.len(), 2);
    for (index, record) in writes.iter().enumerate() {
        assert_eq!(record.actor.as_ref(), Some(&SubjectId::new("operator-a")));
        assert_eq!(
            record.target,
            Some(ActivityTarget::Resource("platform_settings".into()))
        );
        assert_eq!(record.outcome, ActivityOutcome::Succeeded);
        assert_eq!(
            record.detail.get(layer2_core::DetailKey::Revision),
            Some((index + 1).to_string().as_str())
        );
        assert_eq!(
            record.detail.get(layer2_core::DetailKey::Setting),
            Some("platform_settings")
        );
    }
}

#[tokio::test]
async fn measured_visibility_full_replace_reset_and_legacy_replace_are_consistent() {
    let fixture = fixture(false);
    let initial = response_json(send(&fixture.app, Method::GET, PATH, None, ADMIN).await).await;
    let mut settings = initial["settings"].clone();
    for revision in [0, 2] {
        settings["usage_reporting"] = json!({"measured_visible": true});
        let body = json!({"expected_revision": revision, "settings": settings});
        let denied = send(&fixture.app, Method::PUT, PATH, Some(body.clone()), []).await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let saved = send(&fixture.app, Method::PUT, PATH, Some(body.clone()), ADMIN).await;
        assert_eq!(saved.status(), StatusCode::OK);
        assert_eq!(
            response_json(saved).await["settings"]["usage_reporting"]["measured_visible"],
            true
        );
        assert_eq!(
            send(&fixture.app, Method::PUT, PATH, Some(body), ADMIN)
                .await
                .status(),
            StatusCode::CONFLICT
        );
        let disabled = if revision == 0 {
            let reset_path = format!("{PATH}/reset");
            assert_eq!(
                send(
                    &fixture.app,
                    Method::POST,
                    &reset_path,
                    Some(json!({"expected_revision": 1})),
                    []
                )
                .await
                .status(),
                StatusCode::FORBIDDEN
            );
            send(
                &fixture.app,
                Method::POST,
                &reset_path,
                Some(json!({"expected_revision": 1})),
                ADMIN,
            )
            .await
        } else {
            settings.as_object_mut().unwrap().remove("usage_reporting");
            send(
                &fixture.app,
                Method::PUT,
                PATH,
                Some(json!({"expected_revision": 3, "settings": settings})),
                ADMIN,
            )
            .await
        };
        assert_eq!(disabled.status(), StatusCode::OK);
        let disabled = response_json(disabled).await;
        assert_eq!(disabled["revision"], revision + 2);
        assert_eq!(
            disabled["settings"]["usage_reporting"]["measured_visible"],
            false
        );
    }
}

#[tokio::test]
async fn measured_visibility_rejects_invalid_sections_without_writes() {
    let fixture = fixture(false);
    for value in [
        json!(null),
        json!({}),
        json!({"measured_visible": null}),
        json!({"measured_visible": "true"}),
        json!({"measured_visible": true, "unknown": true}),
    ] {
        let response = send(
            &fixture.app,
            Method::PATCH,
            PATH,
            Some(json!({"expected_revision": 0, "usage_reporting": value})),
            ADMIN,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response_json(response).await["code"], "invalid_request");
    }
    assert!(fixture.activity.records.lock().unwrap().is_empty());
    let current = response_json(send(&fixture.app, Method::GET, PATH, None, ADMIN).await).await;
    assert_eq!(current["revision"], 0);
    assert_eq!(
        current["settings"]["usage_reporting"]["measured_visible"],
        false
    );
}

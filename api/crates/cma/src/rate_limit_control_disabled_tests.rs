use super::*;

#[tokio::test]
async fn disabled_control_policy_preserves_prior_classification_and_auth_rules() {
    for inputs in [false, true] {
        let mut app = Router::new();
        for (template, _) in CONTROL_ROUTES {
            app = app.route(
                template,
                post(move |request: Request| async move {
                    request_tier(&request, inputs, false).name()
                }),
            );
        }
        for (_, path) in CONTROL_ROUTES {
            for binding in [false, true] {
                let mut call = request(Method::POST, path, "owner");
                if binding {
                    call.extensions_mut()
                        .insert(layer1_public::AuthMethod::BindingKey {
                            binding_id: "fixture-binding".into(),
                        });
                }
                let response = app.clone().oneshot(call).await.unwrap();
                let expected = if inputs
                    && !binding
                    && path.starts_with("/api/v2/")
                    && path.ends_with("/interrupt")
                {
                    "input"
                } else {
                    "create"
                };
                assert_eq!(
                    to_bytes(response.into_body(), 32).await.unwrap(),
                    expected,
                    "{path}, inputs={inputs}, binding={binding}"
                );
            }
        }
    }
}

#[tokio::test]
async fn disabled_control_policy_spends_the_original_create_budget() {
    let mut limits = local(false);
    Arc::get_mut(&mut limits).unwrap().control_tier_enabled = false;
    let app = router(limits);
    for _ in 0..10 {
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, "/api/v1/sandboxes", "owner"))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
    }
    refusal(
        app.oneshot(request(Method::POST, CONTROL_ROUTES[0].1, "owner"))
            .await
            .unwrap(),
    )
    .await;
}

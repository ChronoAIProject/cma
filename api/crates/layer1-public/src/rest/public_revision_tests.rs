use super::*;

fn consumer_request(path: &str, subject: &str) -> Request<Body> {
    let mut request = request(Method::GET, path, None, Some((subject, false)), None);
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::UseOwnSandboxes]));
    request
}

#[tokio::test]
async fn consumers_read_exact_public_revisions_without_accessing_foreign_drafts() {
    let router = fixture(true);
    let mut workspace = workspace_body("public-exact", "unused", 1);
    workspace["content"]["agent_profiles"] = json!([]);
    let bodies = [
        ("agent-profiles", agent_body("public-exact")),
        ("workspace-profiles", workspace),
        (
            "sandbox-profiles",
            sandbox_body("public-exact", None, vec![], None),
        ),
    ];
    for (kind, body) in bodies {
        let (status, profile) = mutate(
            &router,
            Method::POST,
            &format!("/api/v1/{kind}"),
            Some(body.clone()),
            "publisher",
            &format!("create-{kind}"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = profile["id"].as_str().unwrap();
        let base = format!("/api/v1/{kind}/{id}");
        let (status, first) = mutate(
            &router,
            Method::POST,
            &format!("{base}/revisions"),
            Some(json!({"draft_version": 1})),
            "publisher",
            &format!("publish-{kind}-one"),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let exact = format!("{base}/revisions/1");
        let hidden = json_response(&router, consumer_request(&exact, "consumer")).await;
        assert_eq!(hidden.0, StatusCode::NOT_FOUND);
        let own = json_response(&router, consumer_request(&exact, "publisher")).await;
        assert_eq!(own, (StatusCode::OK, first.clone()));
        let mut content = body["content"].clone();
        if kind == "agent-profiles" {
            content["system_prompt"] = "A later revision.".into();
        }
        if kind == "workspace-profiles" {
            content["setup_commands"] = json!(["git status"]);
        }
        if kind == "sandbox-profiles" {
            content["cpu_millis"] = 2000.into();
        }
        assert_eq!(
            mutate(
                &router,
                Method::PUT,
                &base,
                Some(json!({"content": content, "draft_version": 1})),
                "publisher",
                &format!("edit-{kind}")
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            mutate(
                &router,
                Method::POST,
                &format!("{base}/revisions"),
                Some(json!({"draft_version": 2})),
                "publisher",
                &format!("publish-{kind}-two")
            )
            .await
            .0,
            StatusCode::CREATED
        );
        assert_eq!(
            set_visibility(
                &router,
                "publisher",
                kind,
                id,
                "public",
                &format!("public-{kind}")
            )
            .await
            .0,
            StatusCode::OK
        );
        let read = json_response(&router, consumer_request(&exact, "consumer")).await;
        assert_eq!(read, (StatusCode::OK, first));
        assert_eq!(
            json_response(&router, consumer_request(&base, "consumer"))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        let missing = json_response(
            &router,
            consumer_request(&format!("{base}/revisions/99"), "consumer"),
        )
        .await;
        assert_eq!(missing, hidden);
        assert_eq!(
            mutate(
                &router,
                Method::POST,
                &format!("{base}/retire"),
                None,
                "publisher",
                &format!("retire-{kind}")
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            json_response(&router, consumer_request(&exact, "consumer")).await,
            hidden
        );
    }
}

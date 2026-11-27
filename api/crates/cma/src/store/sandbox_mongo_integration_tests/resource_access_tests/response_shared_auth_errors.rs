use super::*;

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn shared_responses_preserve_the_public_authentication_error() {
    let (f, id) = retained(100).await;
    let client = client();
    let snapshot = format!("/api/v2/responses/{id}");
    let paths = [
        (reqwest::Method::GET, snapshot.clone()),
        (reqwest::Method::GET, format!("{snapshot}/events")),
        (reqwest::Method::GET, format!("{snapshot}/output")),
        (reqwest::Method::POST, "/api/v2/responses".into()),
        (
            reqwest::Method::POST,
            format!("/api/v2/agents/{}/responses", f.agent.id),
        ),
    ];
    for enabled in [false, true] {
        let replicas = servers_mode(&f, enabled).await;
        let server = &replicas[0];
        for (method, path) in &paths {
            let mut request = client.request(method.clone(), format!("{}{path}", server.address));
            if method == reqwest::Method::POST {
                request = request.json(&serde_json::json!({}));
            }
            let response = request.send().await.unwrap();
            assert_eq!(
                response.status(),
                reqwest::StatusCode::UNAUTHORIZED,
                "{enabled}: {path}"
            );
            assert_eq!(response.headers()["content-type"], "application/json");
            let json: Value = response.json().await.unwrap();
            assert_eq!(json["error"]["code"], "authentication_required");
            assert_eq!(json["error"]["type"], "invalid_request_error");
            assert!(json["error"]["message"]
                .as_str()
                .is_some_and(|value| !value.is_empty()));
            assert!(json["error"].get("param").is_some_and(Value::is_null));
        }
        let authorized = request(&client, &f, server, &snapshot)
            .send()
            .await
            .unwrap();
        assert_eq!(authorized.status(), reqwest::StatusCode::OK);
    }
    f.client.database(&f.name).drop().await.unwrap();
}

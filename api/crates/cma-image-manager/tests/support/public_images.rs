//! Public catalog access through real HTTP handlers and the restricted store.
use axum::{routing::post, Form, Json, Router};
use cma_image_manager::{
    auth::NyxIdAuth,
    cloud::{BuildExecutor, Observation},
    http::{router, App},
    model::{Build, BuildInput, BuildPhase, CreateImage, Result},
    permissions as p,
    store::Store,
};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

struct NoCloud;
#[async_trait::async_trait]
impl BuildExecutor for NoCloud {
    async fn submit(&self, _: &Build) -> Result<()> {
        panic!("catalog must not submit builds")
    }
    async fn observe(&self, _: &Build) -> Result<Option<Observation>> {
        panic!("catalog must not query cloud")
    }
    async fn logs(&self, _: &Build) -> Result<Value> {
        panic!("public callers must not read logs")
    }
}

struct Api {
    http: Client,
    base: String,
}
impl Api {
    async fn call(
        &self,
        actor: &str,
        method: Method,
        path: &str,
        revision: i64,
        body: Value,
        status: u16,
    ) -> Value {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if !actor.is_empty() {
            request = request.bearer_auth(actor);
        }
        if revision > 0 {
            request = request.header("If-Match", revision);
        }
        if !body.is_null() {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status().as_u16(), status, "{actor} {path}");
        response.json().await.unwrap()
    }
    async fn read(&self, actor: &str, path: &str, status: u16) -> Value {
        self.call(actor, Method::GET, path, 0, Value::Null, status)
            .await
    }
}

pub async fn verify(store: Store) {
    let image = store
        .create_image(
            "publisher",
            "public-contract",
            CreateImage {
                name: "Public runtime".into(),
                description: "A reusable runtime".into(),
            },
        )
        .await
        .unwrap();
    let image = store
        .share("publisher", &image.id, image.revision, "recipient", true)
        .await
        .unwrap();
    let build = store
        .create_build(
            "publisher",
            &image.id,
            "public-build",
            BuildInput {
                repository: "https://github.com/example/runtime".into(),
                commit: "a".repeat(40),
                dockerfile: "Dockerfile".into(),
                context: ".".into(),
                dockerfile_content: Some("FROM scratch\nLABEL private-build-detail=true\n".into()),
            },
        )
        .await
        .unwrap();
    let claimed = store.claim_build().await.unwrap().unwrap();
    assert_eq!(claimed.id, build.id);
    store
        .observe_build(
            &claimed,
            "fixture-build",
            BuildPhase::Succeeded,
            Some(&format!("sha256:{}", "b".repeat(64))),
        )
        .await
        .unwrap();
    let image = store.image("publisher", &image.id, true).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let auth = NyxIdAuth::new(
        &issuer,
        &format!("{issuer}/api/v1/proxy/s/cma-image-manager"),
        "fixture-client",
        "unused".into(),
    )
    .unwrap();
    let identity = Router::new().route("/oauth/introspect", post(move |Form(form): Form<HashMap<String, String>>| {
        let issuer = issuer.clone();
        async move {
            let token = form.get("token").unwrap();
            let subject = if token == "publisher-readonly" { "publisher" } else { token };
            let permissions = match token.as_str() {
                "publisher-readonly" => vec![p::READ, p::USE],
                "no-permission" => vec![],
                _ => vec![p::ADMIN],
            };
            Json(json!({"active":true,"sub":subject,"iss":issuer,"exp":cma_image_manager::model::now_ms()/1000+3600,"token_type":"access","permissions":permissions}))
        }
    }));
    let identity_task = tokio::spawn(async move { axum::serve(listener, identity).await.unwrap() });
    let app = router(Arc::new(App {
        store,
        auth,
        executor: Arc::new(NoCloud),
        registry: cma_image_manager::model::ImageRegistry::parse(
            "us-docker.pkg.dev/example-project/images".into(),
        )
        .unwrap(),
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = Api {
        http: Client::new(),
        base: format!("http://{}", listener.local_addr().unwrap()),
    };
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let path = format!("/v1/images/{}", image.id);
    let versions = format!("{path}/versions");
    let resolve = format!("{path}/resolve");
    let visibility = format!("{path}/visibility");
    api.read("visitor", &path, 404).await;
    api.read("", "/v1/images?scope=public", 401).await;
    api.read("no-permission", "/v1/images?scope=public", 403)
        .await;
    for (actor, revision, status) in [
        ("publisher-readonly", image.revision, 403),
        ("visitor", image.revision, 404),
        ("publisher", 0, 400),
        ("publisher", image.revision - 1, 409),
    ] {
        api.call(
            actor,
            Method::PUT,
            &visibility,
            revision,
            json!({"visibility":"public"}),
            status,
        )
        .await;
    }
    let published = api
        .call(
            "publisher",
            Method::PUT,
            &visibility,
            image.revision,
            json!({"visibility":"public"}),
            200,
        )
        .await;
    assert_eq!(published["visibility"], "public");
    let visible = api.read("visitor", &path, 200).await;
    assert_eq!(visible["shares"], json!([]));
    assert_eq!(
        api.read("publisher", &path, 200).await["shares"],
        json!(["recipient"])
    );
    for scope in ["public", "all"] {
        let page = api
            .read("visitor", &format!("/v1/images?scope={scope}"), 200)
            .await;
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["id"], image.id);
        assert!(api
            .read(
                "visitor",
                &format!("/v1/images?scope={scope}&after={}", image.id),
                200
            )
            .await["items"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    assert!(
        api.read("publisher", "/v1/images?scope=private", 200).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        api.read("publisher", "/v1/images?scope=owned", 200).await["items"][0]["id"],
        image.id
    );
    assert_eq!(
        api.read("recipient", "/v1/images?scope=shared", 200).await["items"][0]["id"],
        image.id
    );
    let version = api
        .read("visitor", &format!("{versions}/{}", build.id), 200)
        .await;
    assert!(version.get("source").is_none());
    assert!(version.get("error").is_none());
    assert!(version["reference"].as_str().unwrap().contains("@sha256:"));
    assert_eq!(
        api.read("visitor", &versions, 200).await["items"][0],
        version
    );
    assert_eq!(
        api.call(
            "visitor",
            Method::POST,
            &resolve,
            0,
            json!({"version":build.id}),
            200
        )
        .await,
        version
    );
    for endpoint in [
        format!("{path}/builds"),
        format!("/v1/builds/{}", build.id),
        format!("/v1/builds/{}/logs", build.id),
    ] {
        api.read("visitor", &endpoint, 404).await;
    }
    api.call(
        "visitor",
        Method::PATCH,
        &path,
        published["revision"].as_i64().unwrap(),
        json!({"name":"forged"}),
        409,
    )
    .await;
    api.call(
        "visitor",
        Method::DELETE,
        &path,
        published["revision"].as_i64().unwrap(),
        Value::Null,
        409,
    )
    .await;
    let private = api
        .call(
            "publisher",
            Method::PUT,
            &visibility,
            published["revision"].as_i64().unwrap(),
            json!({"visibility":"private"}),
            200,
        )
        .await;
    api.read("visitor", &path, 404).await;
    api.read("visitor", &versions, 404).await;
    api.call(
        "visitor",
        Method::POST,
        &resolve,
        0,
        json!({"version":build.id}),
        404,
    )
    .await;
    api.call(
        "recipient",
        Method::POST,
        &resolve,
        0,
        json!({"version":build.id}),
        200,
    )
    .await;
    let public = api
        .call(
            "publisher",
            Method::PUT,
            &visibility,
            private["revision"].as_i64().unwrap(),
            json!({"visibility":"public"}),
            200,
        )
        .await;
    let retired = api
        .call(
            "publisher",
            Method::DELETE,
            &path,
            public["revision"].as_i64().unwrap(),
            Value::Null,
            200,
        )
        .await;
    assert!(
        api.read("visitor", "/v1/images?scope=public", 200).await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    api.read("visitor", &path, 404).await;
    api.call(
        "publisher",
        Method::PUT,
        &visibility,
        retired["revision"].as_i64().unwrap(),
        json!({"visibility":"public"}),
        409,
    )
    .await;
    api.call(
        "recipient",
        Method::POST,
        &resolve,
        0,
        json!({"version":build.id}),
        409,
    )
    .await;
    api.read("publisher", &format!("/v1/builds/{}", build.id), 200)
        .await;
    server.abort();
    identity_task.abort();
}

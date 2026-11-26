use super::*;
use crate::{http::shapes::*, model::*};
use std::collections::BTreeSet;

const EXPECTED: &[(&str, &[&str])] = &[
    ("/v1/permissions", &["get"]),
    ("/v1/images", &["get", "post"]),
    ("/v1/images/{id}", &["get", "patch", "delete"]),
    ("/v1/images/{id}/visibility", &["put"]),
    ("/v1/images/{id}/shares/{subject}", &["put", "delete"]),
    ("/v1/images/{id}/builds", &["get", "post"]),
    ("/v1/images/{id}/versions", &["get"]),
    ("/v1/images/{id}/versions/{version}", &["get"]),
    ("/v1/images/{id}/resolve", &["post"]),
    ("/v1/builds/{id}", &["get"]),
    ("/v1/builds/{id}/logs", &["get"]),
];

fn references(value: &Value, document: &Value) {
    match value {
        Value::Object(fields) => {
            if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
                assert!(reference.starts_with('#'));
                assert!(
                    document.pointer(&reference[1..]).is_some(),
                    "dangling {reference}"
                );
            }
            for value in fields.values() {
                references(value, document);
            }
        }
        Value::Array(values) => {
            for value in values {
                references(value, document);
            }
        }
        _ => {}
    }
}

#[test]
fn contract_is_the_actual_handler_registry_with_exact_public_scope() {
    let document = document();
    let registry = serde_json::to_value(crate::http::api_router().split_for_parts().1).unwrap();
    assert_eq!(document["paths"].as_object().unwrap().len(), EXPECTED.len());
    let mut ids = BTreeSet::new();
    for (path, methods) in EXPECTED {
        for method in *methods {
            let operation = &document["paths"][path][method];
            assert!(operation.is_object(), "{method} {path}");
            assert_eq!(
                operation["operationId"],
                registry["paths"][path][method]["operationId"]
            );
            assert!(ids.insert(operation["operationId"].as_str().unwrap()));
            assert_eq!(operation["security"].as_array().unwrap().len(), 2);
            assert!(operation["responses"].get("401").is_some());
        }
        assert_eq!(
            document["paths"][path].as_object().unwrap().len(),
            methods.len()
        );
    }
    assert_eq!(ids.len(), 16);
    assert_eq!(document["servers"][0]["url"], "/api/image-manager");
    references(&document, &document);
    for name in ["Image", "Build", "Store", "Principal"] {
        assert!(
            document["components"]["schemas"].get(name).is_none(),
            "persistence/auth schema exposed"
        );
    }
}

#[test]
fn concurrency_headers_and_request_schemas_match_current_handlers() {
    let document = document();
    for (path, method, header) in [
        ("/v1/images", "post", "Idempotency-Key"),
        ("/v1/images/{id}/builds", "post", "Idempotency-Key"),
        ("/v1/images/{id}", "patch", "If-Match"),
        ("/v1/images/{id}", "delete", "If-Match"),
        ("/v1/images/{id}/visibility", "put", "If-Match"),
        ("/v1/images/{id}/shares/{subject}", "put", "If-Match"),
        ("/v1/images/{id}/shares/{subject}", "delete", "If-Match"),
    ] {
        assert!(document["paths"][path][method]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == header && p["in"] == "header" && p["required"] == true));
    }
    let schemas = &document["components"]["schemas"];
    assert_eq!(schemas["CreateImage"]["additionalProperties"], false);
    assert_eq!(schemas["BuildInput"]["additionalProperties"], false);
    assert_eq!(schemas["CreateImage"]["required"], json!(["name"]));
    assert_eq!(
        schemas["BuildInput"]["required"],
        json!(["repository", "commit"])
    );
    assert_eq!(schemas["Resolve"]["required"], json!(["version"]));
    assert_eq!(schemas["SetVisibility"]["required"], json!(["visibility"]));
    for (path, schema) in [
        ("/v1/images", "PageView_ImageView"),
        ("/v1/images/{id}/builds", "PageView_BuildView"),
        ("/v1/images/{id}/versions", "PageView_VersionView"),
    ] {
        assert_eq!(
            document["paths"][path]["get"]["responses"]["200"]["content"]["application/json"]
                ["schema"]["$ref"],
            format!("#/components/schemas/{schema}")
        );
        assert!(schemas[schema]["properties"].get("next_cursor").is_some());
    }
}

fn image() -> Image {
    Image {
        id: "img_fixture".into(),
        schema_version: 1,
        owner: "owner".into(),
        request_key_hash: "private-key-hash".into(),
        request_sha256: "private-fingerprint".into(),
        spec: CreateImage {
            name: "Image".into(),
            description: "Description".into(),
        },
        shares: vec!["recipient".into()],
        visibility: ImageVisibility::Private,
        retired: false,
        revision: 3,
        created_at_ms: 1,
    }
}
fn build() -> Build {
    Build {
        id: "bld_fixture".into(),
        image_id: "img_fixture".into(),
        schema_version: 1,
        owner: "owner".into(),
        request_key_hash: "private-key-hash".into(),
        request_sha256: "private-fingerprint".into(),
        source: BuildInput {
            repository: "https://github.com/org/repo".into(),
            commit: "a".repeat(40),
            dockerfile: "Dockerfile".into(),
            context: ".".into(),
            dockerfile_content: None,
        },
        phase: BuildPhase::Succeeded,
        revision: 4,
        cloud_id: Some("private-cloud-id".into()),
        digest: Some(format!("sha256:{}", "b".repeat(64))),
        error: None,
        created_at_ms: 1,
        updated_at_ms: 2,
    }
}

#[test]
fn typed_projections_preserve_wire_fields_and_owner_only_visibility() {
    let owned = serde_json::to_value(ImageView::new(image(), "owner")).unwrap();
    assert_eq!(
        owned,
        json!({"id":"img_fixture","owner":"owner","name":"Image","description":"Description",
        "shares":["recipient"],"visibility":"private","retired":false,"revision":3,"created_at_ms":1})
    );
    let shared = serde_json::to_value(ImageView::new(image(), "recipient")).unwrap();
    assert_eq!(shared["shares"], json!([]));
    assert_eq!(shared["visibility"], "shared");
    let public = serde_json::to_value(ImageView::new(
        Image {
            visibility: ImageVisibility::Public,
            ..image()
        },
        "recipient",
    ))
    .unwrap();
    assert_eq!(public["visibility"], "public");
    assert_eq!(public["shares"], json!([]));
    let registry =
        crate::model::ImageRegistry::parse("us-docker.pkg.dev/example-project/images".into())
            .unwrap();
    let source = build();
    let expected = json!({"id":source.id,"image_id":source.image_id,"source":source.source,"phase":source.phase,
        "error":source.error,"revision":source.revision,"reference":registry.reference(&source),"digest":source.digest,
        "created_at_ms":source.created_at_ms,"updated_at_ms":source.updated_at_ms});
    assert_eq!(
        serde_json::to_value(BuildView::new(build(), &registry)).unwrap(),
        expected
    );
    let version = serde_json::to_value(VersionView::new(build(), &registry)).unwrap();
    assert_eq!(version.as_object().unwrap().len(), 6);
    assert!(version.get("source").is_none());
    assert!(version.get("error").is_none());
    assert_eq!(version["reference"], expected["reference"]);
    for name in [
        "schema_version",
        "request_key_hash",
        "request_sha256",
        "cloud_id",
    ] {
        assert!(expected.get(name).is_none());
        assert!(version.get(name).is_none());
        assert!(owned.get(name).is_none());
    }
}

#[tokio::test]
async fn service_local_and_canonical_documents_are_identical_and_anonymous() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router()).await.unwrap() });
    for path in ["/openapi.json", "/image-manager/openapi.json"] {
        let response = reqwest::Client::new()
            .get(format!("http://{address}{path}"))
            .header("cookie", "__Host-cma-image-manager=invalid-expired-session")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(response.json::<Value>().await.unwrap(), document());
    }
    server.abort();
}

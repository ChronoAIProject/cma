use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn legacy_binding_is_refused_without_panicking_or_exposing_it_to_another_owner() {
    use mongodb::bson::{doc, spec::BinarySubtype, Binary};
    let mut f = Fixture::new(true).await;
    let (app, binding) = super::bindings::pending(&f).await;
    let id = binding["binding_id"].as_str().unwrap();
    let legacy = json!({"binding_id":id,"app_ref":app["app_ref"],"app_id":42,"installation_id":99,"account_id":123,"status":"active","authority_revision":1,"verified_until":4102444800_i64,"verified_by":"legacy-operator","verification_ref":"legacy-proof","repository_ids":[1001]});
    let sealed = cmaeg_core::seal::seal_blob(
        &key_ring(1),
        &serde_json::to_vec(&json!({"id":id,"owner":"alice","kind":"binding","value":legacy}))
            .unwrap(),
    )
    .unwrap();
    // A historical storage fixture is injected; all observations use public HTTP.
    f.database
        .admin
        .database(&f.database.name)
        .collection::<mongodb::bson::Document>("eg_automation_records")
        .update_one(
            doc! {"_id":id},
            doc! {"$set":{"blob":Binary{subtype:BinarySubtype::Generic,bytes:sealed}}},
        )
        .await
        .unwrap();
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    f.base = f.replica().await;
    let path = format!("/v1/github/installation-bindings/{id}");
    assert_eq!(f.read(&f.base, &path, "token-bob").await.status(), 404);
    assert_eq!(f.read(&f.base, &path, "token-alice").await.status(), 503);
    let body=json!({"action":"opened","installation":{"id":99},"repository":{"id":1001,"owner":{"id":123}},"issue":{"id":10,"number":1}}).to_string();
    let webhook = f
        .webhook(
            &format!(
                "{}/v1/github/apps/{}/webhooks",
                f.base,
                app["app_ref"].as_str().unwrap()
            ),
            "fixture-webhook-secret",
            "legacy-event",
            "issues",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(webhook.status(), 503);
    assert_eq!(
        f.read(
            &f.base,
            &format!(
                "/v1/github/apps/{}/deliveries/legacy-event",
                app["app_ref"].as_str().unwrap()
            ),
            "token-alice"
        )
        .await
        .status(),
        404
    );
    assert_eq!(
        f.read(&f.base, "/v1/channels", "token-alice")
            .await
            .status(),
        200
    );
}

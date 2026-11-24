use super::*;
use cmaeg_core::channel::Platform;
use serde_json::json;

pub(super) fn binding() -> LarkResourceBinding {
    LarkResourceBinding {
        user_service_id: "connection-one".into(),
        catalog_service_id: "catalog-one".into(),
        app_id: "cli_source".into(),
    }
}
pub(super) fn metadata() -> (Value, Value, Value) {
    (
        json!({"services":[{"id":"connection-one","slug":"api-lark-bot","catalog_service_id":"catalog-one","endpoint_id":"endpoint-one","is_active":true,"auth_method":"token_exchange"}]}),
        json!({"endpoints":[{"id":"endpoint-one","catalog_service_id":"catalog-one","auto_connected":false,"url":"https://open.larksuite.com"}]}),
        json!({"id":"catalog-one","slug":"api-lark-bot","base_url":"https://open.larksuite.com","auth_method":"token_exchange","is_active":true}),
    )
}
#[test]
fn exact_service_catalog_scope_and_official_origin_are_required() {
    let (services, endpoints, catalog) = metadata();
    let check = |s: &Value, e: &Value, c: &Value| {
        authorize_metadata(
            &binding(),
            "api-lark-bot",
            "https://open.larksuite.com",
            s,
            e,
            c,
        )
    };
    check(&services, &endpoints, &catalog).unwrap();
    for (pointer, value) in [
        ("/services/0/id", json!("another")),
        ("/services/0/catalog_service_id", json!("another")),
        ("/services/0/slug", json!("api-feishu-bot")),
        ("/services/0/is_active", json!(false)),
        ("/services/0/auth_method", json!("none")),
    ] {
        let mut changed = services.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            check(&changed, &endpoints, &catalog),
            Err(FetchError::Unauthorized)
        );
    }
    let mut changed = services.clone();
    changed["services"][0]["node_id"] = json!("different-route");
    assert_eq!(
        check(&changed, &endpoints, &catalog),
        Err(FetchError::Unauthorized)
    );
    let mut changed = endpoints.clone();
    changed["endpoints"][0]["url"] = json!("https://unrelated.example");
    assert_eq!(
        check(&services, &changed, &catalog),
        Err(FetchError::Unauthorized)
    );
    let mut changed = catalog.clone();
    changed["base_url"] = json!("https://open.feishu.cn");
    assert_eq!(
        check(&services, &endpoints, &changed),
        Err(FetchError::Unauthorized)
    );
    assert_eq!(
        check(&json!({"services":[]}), &endpoints, &catalog),
        Err(FetchError::Unauthorized)
    );
}
#[test]
fn proxy_routes_keep_exact_catalog_connection_and_resource_type() {
    let base = Url::parse("https://nyx.example").unwrap();
    let url = proxy_url(
        &base,
        &binding(),
        &[
            "open-apis",
            "im",
            "v1",
            "messages",
            "om_message",
            "resources",
            "img_resource",
        ],
        Some("image"),
    )
    .unwrap();
    assert_eq!(url.as_str(), "https://nyx.example/api/v1/proxy/catalog-one/open-apis/im/v1/messages/om_message/resources/img_resource?type=image&_nyxid_via=connection-one");
    assert_eq!(
        lark_resource_service(Platform::Feishu),
        Some(("api-feishu-bot", "https://open.feishu.cn"))
    );
    assert_eq!(lark_resource_service(Platform::Whatsapp), None);
}
#[test]
fn source_revision_is_stable_but_changes_with_exact_authority() {
    let mut source: Source = serde_json::from_value(json!({"type":"nyxid_relay","issuer":"https://nyx.example","jwks_url":"https://nyx.example/.well-known/jwks.json","key_id":"fixture-key-id","route_ids":["route-one"],"platform":"lark","bot_id":"ou_source","lark_resources":binding(),"admission":{"type":"all"}})).unwrap();
    let key = Secret::new("resource-read-fixture");
    let original = revision(&source, &key).unwrap();
    assert_eq!(original, revision(&source, &key).unwrap());
    assert_ne!(
        original,
        revision(&source, &Secret::new("resource-revised-fixture")).unwrap()
    );
    if let Source::NyxidRelay {
        lark_resources: Some(binding),
        ..
    } = &mut source
    {
        binding.user_service_id = "another-connection".into();
    }
    assert_ne!(original, revision(&source, &key).unwrap());
}

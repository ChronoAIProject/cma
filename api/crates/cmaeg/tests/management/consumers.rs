//! Consumer creation, patch validation, provider selection and feature gates.
use super::{consumer_support::*, *};
use cmaeg::channels::{consumer_management, management, ChannelTarget};
use cmaeg_core::ids::{ConsumerId, ConversationId};
use serde_json::Value;

#[tokio::test]
async fn adding_two_providers_publishes_pending_then_ready_without_exposing_credentials() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|_| {});
    for (version, slug, name) in [(1, "cma_codex", "reviewer"), (3, "nyxbot", "helper")] {
        let response = gateway.add(&id, &body(version, slug, name)).await;
        assert_eq!(response.status(), 201);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let response: Value = response.json().await.unwrap();
        let consumer = &response["consumer"];
        let consumer_id = consumer["id"].as_str().unwrap();
        assert!(ConsumerId::parse(consumer_id).is_ok());
        assert_eq!(consumer["pending"], false);
        assert_eq!(consumer["level"], "l3");
        assert_eq!(consumer["label"], "Reviewer");
        assert_eq!(consumer["non_human_effective"], slug == "nyxbot");
        assert_eq!(consumer["honoured"], json!(["profile"]));
        assert_eq!(response["channel"]["version"], version + 2);
        assert!(!response.to_string().contains("fixture-key-"));
        let calls = if slug == "nyxbot" {
            gateway.other.requests()
        } else {
            gateway.provider.requests()
        };
        let binding: Value = serde_json::from_str(&calls[0].body).unwrap();
        assert_eq!(binding["agent_key"], format!("fixture-key-{name}"));
        assert_eq!(
            binding["tools"]["native_mcp_url"],
            "http://127.0.0.1:8090/mcp"
        );
        let (stored, _) = gateway.stored(&record);
        assert_eq!(
            stored
                .consumer(Some(&ConsumerId::parse(consumer_id).unwrap()))
                .unwrap()
                .profile_digest,
            consumer["profile_digest"]
        );
    }
    assert_eq!(*gateway.index.changed.lock().unwrap(), [2, 3, 4, 5]);
    gateway.close().await;
}

#[tokio::test]
async fn a_failed_bind_stays_pending_and_an_update_completes_it() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|_| {});
    gateway.provider.reject_next_bind();
    assert_eq!(
        gateway
            .add(&id, &body(1, "cma_codex", "reviewer"))
            .await
            .status(),
        502
    );
    let (stored, version) = gateway.stored(&record);
    assert_eq!(version, 2);
    let consumer = &stored.routing.as_ref().unwrap().consumers[0];
    assert!(consumer.pending);
    assert_eq!(
        stored.routing.as_ref().unwrap().default_targets,
        [ConsumerId::default_consumer()]
    );
    let response = gateway
        .update(
            &id,
            consumer.id.as_str(),
            &json!({"expected_version":2,"name":"reviewer"}),
        )
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["consumer"]["pending"],
        false
    );
    assert!(!gateway.stored(&record).0.routing.unwrap().consumers[0].pending);
    assert_eq!(*gateway.index.changed.lock().unwrap(), [2, 3]);
    gateway.close().await;
}

#[tokio::test]
async fn consumer_refusals_name_request_fields_and_write_nothing() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|_| {});
    let cases = [
        (
            "/credentials/agent_key",
            json!("nyx_fixture_default"),
            "/credentials/agent_key: consumer_key_reused",
        ),
        (
            "/name",
            json!("everyone"),
            "/name: must be a unique, unreserved consumer name",
        ),
        (
            "/name",
            json!("lark_bot"),
            "/name: must be a unique, unreserved consumer name",
        ),
        (
            "/profile/metadata/owner/subject",
            json!("mallory"),
            "/profile/metadata/owner/subject: must match the authenticated channel owner",
        ),
        (
            "/provider/slug",
            json!("unknown_provider"),
            "unknown_provider",
        ),
        (
            "/credentials/agent_key",
            json!(""),
            "/credentials/agent_key: must contain 1..=4096 printable ASCII bytes without spaces",
        ),
    ];
    for (pointer, value, expected) in cases {
        let mut request = body(1, "cma_codex", "reviewer");
        *request.pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            detail(gateway.add(&id, &request).await, 400).await,
            expected
        );
    }
    let mut borrowed = body(1, "cma_codex", "reviewer");
    borrowed["provider"]["target"] =
        json!({"type":"existing_agent","agent_id":"00000000-0000-4000-8000-000000000001"});
    assert!(detail(gateway.add(&id, &borrowed).await, 400)
        .await
        .starts_with("/publish/enabled:"));
    assert_eq!(gateway.stored(&record).1, 1);
    assert_eq!(gateway.calls(), 0);
    assert!(gateway.index.changed.lock().unwrap().is_empty());
    gateway.close().await;
}

#[tokio::test]
async fn duplicate_existing_agent_and_extended_size_are_refused_before_pending_write() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|value| {
        value["provider"]["target"] =
            json!({"type":"existing_agent","agent_id":"00000000-0000-4000-8000-000000000001"});
        value["profile"]["context"]["system_prompt"] = "x".repeat(32_000).into();
        let profile: cmaeg_core::profile::EventWorkerProfile =
            serde_json::from_value(value["profile"].clone()).unwrap();
        value["profile_digest"] = profile.digest().into();
    });
    let mut request = body(1, "cma_codex", "reviewer");
    request["provider"]["target"] =
        json!({"type":"existing_agent","agent_id":"00000000-0000-4000-8000-000000000001"});
    request["publish"] = json!({"enabled":false,"targets":[]});
    assert_eq!(
        detail(gateway.add(&id, &request).await, 400).await,
        "/provider/target: duplicate_agent_target"
    );
    let mut request = body(1, "cma_codex", "reviewer");
    request["profile"]["context"]["system_prompt"] = "x".repeat(32_000).into();
    let error = detail(gateway.add(&id, &request).await, 400).await;
    assert!(
        error.starts_with("/: sealed channel exceeds the record size limit bytes="),
        "{error}"
    );
    assert!(
        error.ends_with(&format!(" limit={}", cmaeg_core::seal::MAX_BLOB_BYTES)),
        "{error}"
    );
    assert_eq!(gateway.stored(&record).1, 1);
    assert_eq!(gateway.calls(), 0);
    gateway.close().await;
}

#[tokio::test]
async fn nullable_patches_retain_or_clear_and_default_rejects_binding_field_presence() {
    let gateway = Gateway::start(true).await;
    let (id, record, _) = gateway.seed(|_| {});
    let response = added(&gateway, &id, 1, "reviewer").await;
    let consumer = response["consumer"]["id"].as_str().unwrap();
    let request = json!({"expected_version":3,"name":"reviewer","non_human":true});
    assert_eq!(gateway.update(&id, consumer, &request).await.status(), 200);
    let request = json!({"expected_version":4,"name":"reviewer"});
    assert_eq!(gateway.update(&id, consumer, &request).await.status(), 200);
    let stored = gateway.stored(&record).0;
    assert_eq!(
        stored.routing.as_ref().unwrap().consumers[0]
            .label
            .as_deref(),
        Some("Reviewer")
    );
    assert_eq!(
        stored.routing.as_ref().unwrap().consumers[0].non_human,
        Some(true)
    );
    let request = json!({"expected_version":5,"name":"reviewer","label":null,"non_human":null});
    let response = gateway.update(&id, consumer, &request).await;
    assert_eq!(response.status(), 200);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["consumer"]["label"], "reviewer");
    assert!(response["consumer"]["non_human"].is_null());
    for field in ["profile", "credentials"] {
        let mut request = json!({"expected_version":6,"name":"lead"});
        request[field] = Value::Null;
        assert_eq!(
            detail(gateway.update(&id, "default", &request).await, 400).await,
            format!("/{field}: default consumer binding changes require PUT /v1/channels")
        );
    }
    let calls = gateway.calls();
    let response = gateway.update(&id,"default",&json!({"expected_version":6,"name":"main","label":"Main","mentionable":false,"non_human":false,"reply":{"auto_final":true},"publish":{"enabled":false}})).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        gateway.calls(),
        calls,
        "default presentation does not rebind"
    );
    gateway.close().await;
}

#[tokio::test]
async fn gated_operations_refuse_with_switch_off_and_record_targets_enforce_owner() {
    let gateway = Gateway::start(false).await;
    let (id, record, _) = gateway.seed(|_| {});
    assert_eq!(
        detail(
            gateway.add(&id, &body(1, "cma_codex", "reviewer")).await,
            400
        )
        .await,
        "multi_consumer_disabled"
    );
    assert_eq!(
        detail(
            gateway
                .update(&id, "default", &json!({"expected_version":1,"name":"lead"}))
                .await,
            400
        )
        .await,
        "multi_consumer_disabled"
    );
    let response = gateway
        .client
        .post(format!(
            "{}/v1/channels/{id}/test?consumer=csm_0123456789ab",
            gateway.base
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(detail(response, 400).await, "multi_consumer_disabled");
    let mut caller = creator();
    caller.subject = "mallory".into();
    let result = consumer_management::add(
        &gateway.state,
        &caller,
        &ChannelTarget::Record(record.clone()),
        serde_json::from_value(body(1, "cma_codex", "reviewer")).unwrap(),
    )
    .await;
    assert!(matches!(result, Err(management::ManagementError::NotFound)));
    assert_eq!(gateway.calls(), 0);
    gateway.close().await;
    let gateway = Gateway::start(true).await;
    let (id, _, _) = gateway.seed(|value| {
        value.as_object_mut().unwrap().remove("routing");
    });
    assert_eq!(
        detail(
            gateway.add(&id, &body(1, "cma_codex", "reviewer")).await,
            400
        )
        .await,
        "routing_required"
    );
    gateway.close().await;
}

#[tokio::test]
async fn test_channel_selects_the_consumer_and_default_keeps_its_conversation() {
    let gateway = Gateway::start(true).await;
    let (id, _, definition) = gateway.seed(|_| {});
    let response = gateway.add(&id, &body(1, "nyxbot", "helper")).await;
    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.unwrap();
    let consumer = ConsumerId::parse(body["consumer"]["id"].as_str().unwrap()).unwrap();
    for query in [
        "".to_owned(),
        "?consumer=default".into(),
        "?legacy_unused=value".into(),
        format!("?consumer={consumer}"),
    ] {
        let response = gateway
            .client
            .post(format!("{}/v1/channels/{id}/test{query}", gateway.base))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    let legacy = ConversationId::derive(&key_ring(1), &id, definition.partition.version, "test");
    let additional = ConversationId::derive_for_consumer(
        &key_ring(1),
        &id,
        definition.partition.version,
        &consumer,
        "test",
    );
    let paths: Vec<_> = gateway
        .provider
        .requests()
        .iter()
        .filter(|call| call.path.contains("/conversations/"))
        .map(|call| call.path.clone())
        .collect();
    assert_eq!(paths.len(), 3);
    assert!(paths.iter().all(|path| path.ends_with(legacy.as_str())));
    assert!(gateway
        .other
        .requests()
        .iter()
        .any(|call| call.path.ends_with(additional.as_str())
            && call.authorization.as_deref() == Some("Bearer fixture-key-helper")));
    gateway.close().await;
}

use super::*;
use layer2_core::resource_access::ResourceAccessService;
use std::{collections::BTreeMap, path::PathBuf};

#[tokio::test]
async fn shared_agent_delegations_contract_replays_real_cma_broker_and_org_discovery() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../layer1-public/tests/fixtures/public-contract/auth-agent-delegations.json");
    let mut corpus: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(corpus["version"], 1);
    let record = ["CMA_RECORD_PUBLIC_CONTRACT", "CMA_UPDATE_PUBLIC_CONTRACT"]
        .iter()
        .any(|key| std::env::var(key).as_deref() == Ok("1"));
    for case in corpus["cases"].as_array_mut().unwrap() {
        let f = Fixture::new().await;
        f.activate().await;
        let resources = match case["scenario"].as_str().unwrap() {
            "delegation_secret_echo" => Router::new().route(
                "/api/v1/organizations",
                axum::routing::get(
                    |Extension(token): Extension<DelegatedCredential>| async move {
                        Json(json!({"echo":token.value()}))
                    },
                ),
            ),
            "delegation_linked" => layer1_public::rest::resource_access_router(Arc::new(
                ResourceAccessService::new(f.store.clone(), f.authority.auth.clone()),
            )),
            other => panic!("unknown delegation fixture {other}"),
        };
        assert_eq!(case["request"]["method"], "POST");
        assert_eq!(
            case["operation"],
            format!("POST {}", case["request"]["path"].as_str().unwrap())
        );
        assert_eq!(
            case["request"]["headers"],
            json!({"content-type":"application/json"})
        );
        let response = request(
            &f,
            resources,
            case["request"]["path"].as_str().unwrap(),
            case["request"]["body"].clone(),
        )
        .await;
        assert_eq!(
            response.status().as_u16(),
            case["response"]["status"].as_u64().unwrap() as u16,
            "{}",
            case["id"]
        );
        let mut headers = BTreeMap::new();
        for (name, value) in response.headers() {
            assert!(headers
                .insert(name.as_str().to_owned(), value.to_str().unwrap().to_owned())
                .is_none());
        }
        headers.remove("content-length");
        let mut body = json_body(response).await;
        for (name, pointer) in case["captures"].as_object().unwrap() {
            assert_eq!(name, "grant");
            assert_eq!(pointer, "/grant_id");
            let actual = body.pointer(pointer.as_str().unwrap()).unwrap();
            assert!(actual.as_str().is_some_and(|value| value.len() == 32));
            assert_eq!(case["response"]["body"]["grant_id"], "grant_fixture");
            body["grant_id"] = "grant_fixture".into();
        }
        if record {
            case["response"]["headers"] = serde_json::to_value(headers).unwrap();
            case["response"]["body"] = body;
        } else {
            assert_eq!(
                case["response"]["headers"],
                serde_json::to_value(headers).unwrap(),
                "{} headers",
                case["id"]
            );
            assert_eq!(case["response"]["body"], body, "{} body", case["id"]);
        }
    }
    if record {
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&corpus).unwrap()),
        )
        .unwrap();
    }
}

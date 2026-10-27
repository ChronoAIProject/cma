use super::{
    compile,
    management::{AssetFetch, ManagementClient, Manager},
    test_fixture::message,
};
use crate::{
    net::exchange::{HttpReply, TransportError},
    whatsapp::{Client, Error, Transport},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, Url};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
struct Graph(Mutex<VecDeque<Value>>);
impl Transport for Graph {
    fn exchange(
        &self,
        _: Method,
        url: Url,
        _: HeaderMap,
        _: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(url.host_str(), Some("graph.facebook.com"));
        let value = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected graph call");
        Box::pin(async move {
            Ok(HttpReply {
                status: reqwest::StatusCode::OK,
                headers: HeaderMap::new(),
                body: Ok(serde_json::to_vec(&value).unwrap()),
            })
        })
    }
}
struct Asset(Vec<u8>);
impl AssetFetch for Asset {
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Vec<u8>, Error>> {
        assert_eq!(url, "https://scontent.xx.fbcdn.net/flow.json");
        Box::pin(async move { Ok(self.0.clone()) })
    }
}
fn source() -> cmaeg_core::channel::Source {
    let mut source = crate::whatsapp::tests::source();
    if let cmaeg_core::channel::Source::Whatsapp { forms_enabled, .. } = &mut source {
        *forms_enabled = true;
    }
    source
}
fn manager(answers: Vec<Value>, asset: Vec<u8>) -> ManagementClient {
    ManagementClient::new(
        Client::production()
            .unwrap()
            .with_transport(Arc::new(Graph(Mutex::new(answers.into())))),
    )
    .with_fetch(Arc::new(Asset(asset)))
}
fn info() -> Value {
    json!({"id":"99999","name":format!("cmaeg_static_v1_{}","a".repeat(64)),"status":"PUBLISHED","validation_errors":[],"json_version":"7.3","whatsapp_business_account":{"id":"23456"},"application":{"id":"12345"},"health_status":{"can_send_message":"AVAILABLE"}})
}
fn assets() -> Value {
    json!({"data":[{"asset_type":"FLOW_JSON","download_url":"https://scontent.xx.fbcdn.net/flow.json"}]})
}
#[tokio::test]
async fn inspection_proves_exact_account_app_health_version_and_downloaded_schema() {
    let schema = compile::compile(&message()).unwrap();
    let name = info()["name"].as_str().unwrap().to_owned();
    let m = manager(
        vec![info(), assets()],
        serde_json::to_vec(&schema.json).unwrap(),
    );
    let proof = m
        .inspect(
            &source(),
            &crate::whatsapp::tests::credentials(),
            "99999",
            &name,
        )
        .await
        .unwrap();
    assert_eq!(proof.digest, schema.digest);
    for field in [
        "application",
        "whatsapp_business_account",
        "json_version",
        "health_status",
        "validation_errors",
    ] {
        let mut bad = info();
        bad[field] = Value::Null;
        let m = manager(vec![bad], vec![]);
        assert!(m
            .inspect(
                &source(),
                &crate::whatsapp::tests::credentials(),
                "99999",
                &name
            )
            .await
            .is_err());
    }
    let m = manager(
        vec![info(), assets()],
        br#"{"version":"7.3","version":"3.1"}"#.to_vec(),
    );
    assert!(m
        .inspect(
            &source(),
            &crate::whatsapp::tests::credentials(),
            "99999",
            &name
        )
        .await
        .is_err());
    let mut evil = assets();
    evil["data"][0]["download_url"] = "https://127.0.0.1/flow.json".into();
    let m = manager(vec![info(), evil], vec![]);
    assert!(m
        .inspect(
            &source(),
            &crate::whatsapp::tests::credentials(),
            "99999",
            &name
        )
        .await
        .is_err());
}
#[tokio::test]
async fn malformed_create_success_is_uncertain_and_explicit_validation_failure_declines() {
    let schema = compile::compile(&message()).unwrap();
    let name = info()["name"].as_str().unwrap().to_owned();
    for answer in [
        json!({"id":"99999"}),
        json!({"success":true,"validation_errors":[]}),
        json!({"success":true,"id":"99999"}),
    ] {
        assert_eq!(
            manager(vec![answer], vec![])
                .create(
                    &source(),
                    &crate::whatsapp::tests::credentials(),
                    &name,
                    &schema.json
                )
                .await
                .unwrap_err(),
            Error::Unconfirmed
        );
    }
    assert_eq!(
        manager(
            vec![json!({"success":true,"id":"99999","validation_errors":[{"error":"bad"}]})],
            vec![]
        )
        .create(
            &source(),
            &crate::whatsapp::tests::credentials(),
            &name,
            &schema.json
        )
        .await
        .unwrap_err(),
        Error::Rejected
    );
}

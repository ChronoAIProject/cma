use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, Capability,
    GatewayRecord, GatewayRecordHead, GatewayRecordService, GatewayRecordStore, Page, StoreError,
    SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{gateway_connection_record_router, gateway_record_router};
use crate::AuthenticatedSubject;

#[derive(Default)]
struct MemoryRecords(Mutex<BTreeMap<String, GatewayRecord>>);

#[async_trait]
impl GatewayRecordStore for MemoryRecords {
    async fn put(
        &self,
        id: &str,
        owner: &SubjectId,
        blob: Vec<u8>,
        expected: Option<u64>,
    ) -> Result<GatewayRecord, StoreError> {
        let mut records = self.0.lock().unwrap();
        let current = records.get(id).cloned();
        match (current, expected) {
            (None, None) => {
                let record = GatewayRecord {
                    record_id: id.into(),
                    owner: owner.clone(),
                    blob,
                    version: 1,
                    created_at: "now".into(),
                    updated_at: "now".into(),
                };
                records.insert(id.into(), record.clone());
                Ok(record)
            }
            (Some(mut record), Some(expected)) if record.version == expected => {
                record.owner = owner.clone();
                record.blob = blob;
                record.version += 1;
                records.insert(id.into(), record.clone());
                Ok(record)
            }
            (Some(record), expected) => Err(StoreError::RevisionConflict {
                expected: expected.unwrap_or(0),
                actual: record.version,
            }),
            (None, Some(expected)) => Err(StoreError::RevisionConflict {
                expected,
                actual: 0,
            }),
        }
    }

    async fn get(&self, id: &str) -> Result<Option<GatewayRecord>, StoreError> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }

    async fn delete(&self, id: &str, expected: Option<u64>) -> Result<bool, StoreError> {
        let mut records = self.0.lock().unwrap();
        let Some(record) = records.get(id) else {
            return Ok(false);
        };
        if expected.is_some_and(|version| version != record.version) {
            return Err(StoreError::RevisionConflict {
                expected: expected.unwrap(),
                actual: record.version,
            });
        }
        records.remove(id);
        Ok(true)
    }

    async fn list(
        &self,
        owner: Option<&SubjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<GatewayRecordHead>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|record| owner.is_none_or(|owner| owner == &record.owner))
            .filter(|record| after.is_none_or(|after| record.record_id.as_str() > after))
            .take(limit)
            .map(|record| GatewayRecordHead {
                record_id: record.record_id.clone(),
                owner: record.owner.clone(),
                version: record.version,
                updated_at: record.updated_at.clone(),
            })
            .collect())
    }
}

struct MemoryActivity;

#[async_trait]
impl ActivityStore for MemoryActivity {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(Vec::new(), None))
    }
}

fn app() -> Router {
    let records = Arc::new(MemoryRecords::default());
    let activity = ActivityService::new(Arc::new(MemoryActivity));
    gateway_record_router(GatewayRecordService::new(records, activity))
}

fn namespaced_app() -> Router {
    app().merge(gateway_connection_record_router(GatewayRecordService::new(
        Arc::new(MemoryRecords::default()),
        ActivityService::new(Arc::new(MemoryActivity)),
    )))
}

fn request(
    path: &str,
    method: &str,
    body: Option<Value>,
    authorization: Option<Authorization>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let mut request = builder
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    if let Some(authorization) = authorization {
        request.extensions_mut().insert(authorization);
    }
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId::new("subject")));
    request
}

#[tokio::test]
async fn gateway_routes_require_authentication_and_capability() {
    let id = uuid::Uuid::new_v4();
    let response = app()
        .oneshot(
            Request::get(format!("/api/v1/gateway/channel-records/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL),
        Some(&"no-store".parse().unwrap())
    );
    let response = app()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}"),
            "GET",
            None,
            Some(Authorization::none()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn gateway_routes_round_trip_and_report_cas_and_size_errors() {
    let app = app();
    let id = "123e4567-e89b-42d3-a456-426614174001";
    let second_id = "123e4567-e89b-42d3-a456-426614174002";
    let authority = Authorization::new([Capability::ManageGatewayRecords]);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}"),
            "PUT",
            Some(
                json!({"owner":"subject","blob":STANDARD.encode([1,2,3]),"expected_version":null}),
            ),
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}"),
            "GET",
            None,
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}"),
            "PUT",
            Some(json!({"owner":"subject","blob":STANDARD.encode([4]),"expected_version":1})),
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}"),
            "PUT",
            Some(json!({"owner":"subject","blob":STANDARD.encode([1]),"expected_version":9})),
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{second_id}"),
            "PUT",
            Some(json!({"owner":"subject","blob":STANDARD.encode([9]),"expected_version":null})),
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            "/api/v1/gateway/channel-records?owner=subject&limit=1",
            "GET",
            None,
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}?expected_version=2"),
            "DELETE",
            None,
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}"),
            "GET",
            None,
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{second_id}?expected_version=9"),
            "DELETE",
            None,
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{second_id}"),
            "DELETE",
            None,
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{second_id}"),
            "DELETE",
            None,
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}"),
            "PUT",
            Some(json!({"owner":"subject","blob":STANDARD.encode(vec![0_u8; 65_537]),"expected_version":null})),
            Some(authority),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn connection_record_routes_require_authenticated_gateway_capability_for_every_operation() {
    let app = namespaced_app();
    let id = "123e4567-e89b-42d3-a456-426614174001";
    let record = format!("/api/v1/gateway/connection-records/{id}");
    for (method, path) in [
        ("GET", "/api/v1/gateway/connection-records"),
        ("GET", record.as_str()),
        ("PUT", record.as_str()),
        ("DELETE", record.as_str()),
    ] {
        let body = (method == "PUT").then(
            || json!({"owner":"subject","blob":STANDARD.encode([7]),"expected_version":null}),
        );
        for authenticated in [false, true] {
            let mut request = request(
                path,
                method,
                body.clone(),
                authenticated.then(|| Authorization::new([Capability::ManageOwnBots])),
            );
            if !authenticated {
                request.extensions_mut().remove::<AuthenticatedSubject>();
            }
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                if authenticated {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::UNAUTHORIZED
                },
                "{method} {path}"
            );
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }
    let response = app
        .oneshot(request(
            &record,
            "GET",
            None,
            Some(Authorization::new([Capability::ManageGatewayRecords])),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "refused PUT did not persist a record"
    );
}

#[tokio::test]
async fn connection_and_channel_routes_keep_same_uuid_isolated_with_independent_cas() {
    let app = namespaced_app();
    let id = "123e4567-e89b-42d3-a456-426614174001";
    let authority = Authorization::new([Capability::ManageGatewayRecords]);
    for (namespace, owner, blob) in [
        ("channel-records", "channel-owner", [1_u8]),
        ("connection-records", "connection-owner", [2_u8]),
    ] {
        let response = app
            .clone()
            .oneshot(request(
                &format!("/api/v1/gateway/{namespace}/{id}"),
                "PUT",
                Some(json!({"owner":owner,"blob":STANDARD.encode(blob),"expected_version":null})),
                Some(authority.clone()),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let connection = format!("/api/v1/gateway/connection-records/{id}");
    let response = app.clone().oneshot(request(&connection, "PUT",
        Some(json!({"owner":"connection-owner","blob":STANDARD.encode([3_u8]),"expected_version":1})), Some(authority.clone()))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(request(
            &connection,
            "DELETE",
            None,
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = app
        .clone()
        .oneshot(request(&connection, "GET", None, Some(authority.clone())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = app
        .clone()
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}"),
            "GET",
            None,
            Some(authority.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(value["owner"], "channel-owner");
    assert_eq!(value["blob"], STANDARD.encode([1_u8]));
    assert_eq!(value["version"], 1);
    let response = app
        .oneshot(request(
            &format!("/api/v1/gateway/channel-records/{id}?expected_version=2"),
            "DELETE",
            None,
            Some(authority),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "connection updates did not advance channel CAS"
    );
}

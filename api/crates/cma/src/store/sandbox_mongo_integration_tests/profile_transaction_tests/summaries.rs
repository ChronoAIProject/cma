use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer2_core::profiles::{ProfileCapacityBounds, SandboxProfile};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

mod fixture;
mod policy;
mod query;
mod snapshot;
use fixture::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn profile_summary_routes_bound_commands_and_match_individual_views() {
    let (client, name, store, meter) = measured_store().await;
    for kind in KINDS {
        seed(&store, kind, 201, 64, false).await;
    }
    let router = routes(store.clone());
    for kind in KINDS {
        meter.start();
        let (status, page) = get(
            &router,
            "owner",
            &format!("/api/v1/{kind}-profiles?limit=200"),
        )
        .await;
        let counts = meter.finish();
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["items"].as_array().unwrap().len(), 200);
        assert_eq!(page["next"], id(kind, 199));
        for item in page["items"].as_array().unwrap() {
            let (status, individual) = get(
                &router,
                "owner",
                &format!("/api/v1/{kind}-profiles/{}", item["id"].as_str().unwrap()),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(*item, individual);
            assert_eq!(item["latest_revision"]["revision"], 64);
        }
        println!("profile-summary kind={kind} profiles=200 revisions=64 commands={} reply_bson_bytes={} body_sha256={:x}", counts.commands, counts.bytes, Sha256::digest(serde_json::to_vec(&page).unwrap()));
        assert_eq!(counts.failed, 0);
        assert!(counts.commands <= 3, "{} commands", counts.commands);
        let (status, last) = get(
            &router,
            "owner",
            &format!(
                "/api/v1/{kind}-profiles?limit=200&cursor={}",
                page["next"].as_str().unwrap()
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(last["items"].as_array().unwrap().len(), 1);
        assert_eq!(last["items"][0]["id"], id(kind, 200));
        assert!(last["next"].is_null());
    }
    client.database(&name).drop().await.unwrap();
}

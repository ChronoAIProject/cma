#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{
    Error,
    bridges::{BridgePlacementAvailability, BridgeSharedPlacementKind},
};
use serde_json::{Value, json};

fn expect(server: &wire::WireServer, placements: Option<Value>) {
    let mut value = json!({"version":1,"source_revision":null,"connection_validation_available":false,"connectors":[]});
    if let Some(placements) = placements {
        value["placements"] = placements;
    }
    server.expect(wire::json(
        "GET",
        "/api/v1/bridge-connectors/capabilities",
        None,
        200,
        value,
        true,
    ));
}

#[tokio::test]
async fn placement_catalog_is_additive_and_retains_typed_unavailability() {
    let server = wire::WireServer::new().await;
    let _ = (&server.anonymous, &server.base_url, server.request_count());
    expect(&server, None);
    assert!(
        server
            .client
            .bridges()
            .capabilities(None)
            .await
            .unwrap()
            .placements
            .is_empty()
    );
    expect(
        &server,
        Some(json!([
            {"kind":"workspace_per_partition","availability":"continuity_unverified"},
            {"kind":"worktree_agent_per_partition","availability":"runtime_unavailable"}
        ])),
    );
    let catalog = server.client.bridges().capabilities(None).await.unwrap();
    assert_eq!(
        catalog.placements[0].kind,
        BridgeSharedPlacementKind::WorkspacePerPartition
    );
    assert_eq!(
        catalog.placements[0].availability,
        BridgePlacementAvailability::ContinuityUnverified
    );
    assert_eq!(
        catalog.placements[1].availability,
        BridgePlacementAvailability::RuntimeUnavailable
    );
    server.assert_drained();
}

#[tokio::test]
async fn unknown_malformed_and_duplicate_placement_capabilities_are_not_usable() {
    let server = wire::WireServer::new().await;
    let value = json!({"kind":"workspace_per_partition","availability":"available"});
    for placements in [
        Value::Null,
        json!([value.clone(), value]),
        json!([{"kind":"future_placement","availability":"available"}]),
        json!([{"kind":"workspace_per_partition","availability":"probably_ready"}]),
        json!([{"kind":"workspace_per_partition"}]),
    ] {
        expect(&server, Some(placements));
        assert!(matches!(
            server.client.bridges().capabilities(None).await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
}

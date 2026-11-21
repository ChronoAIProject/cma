use super::*;
use layer2_core::sandbox::colors::{
    InMemorySandboxColorStore, SandboxColor, SandboxColorService, SandboxColorStore,
    SystemColorRandom,
};

fn colored(mut fixture: Fixture) -> (Fixture, Arc<InMemorySandboxColorStore>) {
    let palettes = Arc::new(InMemorySandboxColorStore::default());
    fixture.state = fixture.state.with_sandbox_colors(SandboxColorService::new(
        palettes.clone(),
        fixture.store.clone(),
        Arc::new(SystemColorRandom::new()),
    ));
    fixture.router = hierarchy_router(fixture.state.clone());
    (fixture, palettes)
}

fn color_of(value: &Value) -> String {
    let color = value["color"].as_str().expect("a colour").to_owned();
    SandboxColor::parse(&color).expect("a lowercase #rrggbb colour");
    color
}

#[tokio::test]
async fn owner_reads_carry_one_distinct_stable_colour_per_sandbox() {
    let (fixture, palettes) = colored(fixture(10));
    for sandbox in [
        stored_sandbox("sbx_1", "owner", SandboxStatus::Active, 4),
        stored_sandbox("sbx_2", "owner", SandboxStatus::Paused, 4),
        stored_sandbox("sbx_9", "foreign", SandboxStatus::Active, 4),
    ] {
        fixture.store.create_sandbox(sandbox).await.unwrap();
    }

    let (status, page) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/sandboxes", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first = color_of(&page["items"][0]);
    let second = color_of(&page["items"][1]);
    assert_ne!(first, second);

    let (status, detail) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/sandboxes/sbx_2", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(color_of(&detail), second);
    let (_, again) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/sandboxes", None, None),
    )
    .await;
    assert_eq!(color_of(&again["items"][0]), first);
    assert_eq!(color_of(&again["items"][1]), second);

    // Listing one owner's Sandboxes never paints another owner's.
    let foreign = palettes
        .owner_colors(&SubjectId::new("foreign"))
        .await
        .unwrap();
    assert_eq!(foreign.revision, 0);
}

#[tokio::test]
async fn created_sandbox_is_returned_wearing_its_colour() {
    let (fixture, _) = colored(fixture(10));
    let (profile_id, _) = publish_sandbox_profile(
        &fixture.profiles,
        "sbp_colours",
        "owner",
        ProfileVisibility::Public,
        vec![],
    )
    .await;
    let (status, created) = json_response(
        &fixture.router,
        authenticated(
            Method::POST,
            "/api/v1/sandboxes",
            Some(json!({
                "sandbox_profile": { "id": profile_id, "revision": 1 },
                "name": "painted-box"
            })),
            Some("create-painted"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let color = color_of(&created);
    let id = created["id"].as_str().unwrap();
    let (_, detail) = json_response(
        &fixture.router,
        authenticated(Method::GET, &format!("/api/v1/sandboxes/{id}"), None, None),
    )
    .await;
    assert_eq!(color_of(&detail), color);
}

#[tokio::test]
async fn deployments_without_colours_omit_the_field() {
    let fixture = fixture(10);
    fixture
        .store
        .create_sandbox(stored_sandbox("sbx_1", "owner", SandboxStatus::Active, 4))
        .await
        .unwrap();
    let (_, page) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/sandboxes", None, None),
    )
    .await;
    assert!(page["items"][0].get("color").is_none());
    let (_, detail) = json_response(
        &fixture.router,
        authenticated(Method::GET, "/api/v1/sandboxes/sbx_1", None, None),
    )
    .await;
    assert!(detail.get("color").is_none());
}

#[path = "sandbox_enrichment_tests.rs"]
mod enrichment;

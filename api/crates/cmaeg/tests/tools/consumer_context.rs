//! The context tools on a routed channel read and attest for the calling
//! consumer: through its own binding and credential, reporting its binding
//! and summary, so a CMA-style verifier accepts every consumer.
use super::{
    consumer_support::{call, handoff, issue, reviewer, reviewer_key, routed, REVIEWER_BINDING},
    support::App,
};
use cmaeg_core::{ids::ConsumerId, seal::EventRef};
use reqwest::Method;
use serde_json::{json, Value};

/// CMA's `verifyEventContext` proof check: the proof must be verified and
/// name the binding the call was made for (`event_routing.rs`).
fn cma_verifier(calling_binding: &str, proof: &Value) -> Result<(), &'static str> {
    if proof["activity_verified"] != true {
        return Err("activity not verified");
    }
    if proof["provider_binding_id"] != calling_binding {
        return Err("event_identity_invalid");
    }
    Ok(())
}

async fn read(app: &App, reference: &EventRef, key: &str) -> Value {
    let path = format!("/v1/events/{}/context", reference.as_str());
    let response = call(app, Method::GET, &path, key, None).await;
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

async fn verify(app: &App, reference: &EventRef, key: &str) -> Value {
    let path = format!("/v1/events/{}/context", reference.as_str());
    let activity = serde_json::to_value(&app.activity).unwrap();
    let response = call(app, Method::POST, &path, key, Some(activity)).await;
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

#[tokio::test]
async fn a_context_read_goes_through_the_calling_consumers_binding_and_credential() {
    let app = routed().await;
    let reference = issue(&app, &reviewer(), &reviewer_key(), None).await;
    let before = app.fake.log().len();
    let context = read(&app, &reference, &reviewer_key()).await;
    let reads: Vec<_> = app.fake.log()[before..]
        .iter()
        .filter(|request| request.method == "GET")
        .cloned()
        .collect();
    assert_eq!(reads.len(), 1, "one provider context read");
    assert!(
        reads[0].path.starts_with(&format!(
            "/api/v1/cmaeg/bindings/{REVIEWER_BINDING}/conversations/"
        )),
        "{}",
        reads[0].path
    );
    let expected = format!("Bearer {}", reviewer_key());
    assert_eq!(reads[0].authorization.as_deref(), Some(expected.as_str()));
    assert_eq!(context["provider_binding_id"], REVIEWER_BINDING);
    assert_eq!(context["activity_verified"], true);
    assert_eq!(
        context["consumer"],
        json!({"id": reviewer().as_str(), "name": "reviewer", "label": "reviewer"})
    );
}

#[tokio::test]
async fn a_cma_style_verifier_accepts_a_non_default_consumer_in_both_context_tools() {
    let app = routed().await;
    let reference = issue(&app, &reviewer(), &reviewer_key(), None).await;
    let channel = app
        .state
        .resolver
        .resolve(app.fixture.id.as_str())
        .await
        .unwrap();
    let default_binding = channel
        .consumer(None)
        .unwrap()
        .provider
        .binding_id
        .to_string();
    for proof in [
        read(&app, &reference, &reviewer_key()).await,
        verify(&app, &reference, &reviewer_key()).await,
    ] {
        assert_eq!(cma_verifier(REVIEWER_BINDING, &proof), Ok(()), "{proof}");
        // The proof is the reviewer's: one naming the default's binding, as
        // every proof did before, fails the verifier for this call.
        assert_eq!(
            cma_verifier(&default_binding, &proof),
            Err("event_identity_invalid")
        );
        assert_eq!(proof["consumer"]["id"], reviewer().as_str());
    }
}

#[tokio::test]
async fn the_default_consumer_of_a_routed_channel_is_summarized_as_presented() {
    let app = routed().await;
    let default = ConsumerId::default_consumer();
    let reference = issue(&app, &default, &app.key, None).await;
    for context in [
        read(&app, &reference, &app.key).await,
        verify(&app, &reference, &app.key).await,
    ] {
        assert_eq!(
            context["consumer"],
            json!({"id": "default", "name": "lead", "label": "lead"})
        );
        assert_ne!(context["provider_binding_id"], REVIEWER_BINDING);
    }
}

#[tokio::test]
async fn a_hand_off_projects_no_buttons_or_cards() {
    let app = routed().await;
    let plain = issue(&app, &reviewer(), &reviewer_key(), None).await;
    let lineage = handoff(&app);
    let handed = issue(&app, &reviewer(), &reviewer_key(), Some(lineage)).await;
    let capabilities = |context: &Value| {
        (
            context["capabilities"]["buttons"].clone(),
            context["capabilities"]["cards"].clone(),
        )
    };
    let ordinary = read(&app, &plain, &reviewer_key()).await;
    // This fixture keeps named-control issuance disabled. Plain events still
    // render choices as text; hand-offs must remove even that capability.
    assert!(!app.state.consumer_interactions_enabled);
    assert_eq!(
        capabilities(&ordinary),
        (json!("numbered_text"), json!("text"))
    );
    for context in [
        read(&app, &handed, &reviewer_key()).await,
        verify(&app, &handed, &reviewer_key()).await,
    ] {
        assert_eq!(capabilities(&context), (json!("none"), json!("none")));
        // Everything else about the capabilities is unchanged.
        assert_eq!(
            context["capabilities"]["edit"],
            ordinary["capabilities"]["edit"]
        );
        assert_eq!(
            context["capabilities"]["inbound_media"],
            ordinary["capabilities"]["inbound_media"]
        );
    }
}

#[tokio::test]
async fn an_unrouted_channel_reports_no_consumer() {
    let app = App::new().await;
    let path = format!("/v1/events/{}/context", app.event_ref.as_str());
    let response = call(&app, Method::GET, &path, &app.key, None).await;
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert!(!body.contains("\"consumer\""), "{body}");
}

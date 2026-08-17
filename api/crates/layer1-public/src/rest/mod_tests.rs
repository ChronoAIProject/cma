//! Tests for the REST facade.
//!
//! Kept beside the module rather than inside it so that file stays within
//! the project's size limit while the behaviour remains covered.

use super::*;

#[test]
fn rest_projects_every_core_operation() {
    // REST has no structural constraint, so a missing operation here means
    // the facade has drifted from the core rather than that the protocol
    // cannot express it.
    assert_eq!(RestInterface.operations().len(), 9);
}

/// Everything a client can observe about a refusal.
async fn rendered(e: CoreError) -> (StatusCode, serde_json::Value) {
    let response = Problem(e).into_response();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("problem documents are small");
    (
        status,
        serde_json::from_slice(&bytes).expect("a problem document is json"),
    )
}

#[tokio::test]
async fn unauthorized_is_indistinguishable_from_not_found() {
    // Not merely the same status. A body that still says "forbidden"
    // confirms the identifier exists, which is the one thing this refusal
    // is shaped to withhold — so every field a client can read must match.
    let (forbidden_status, forbidden_body) = rendered(CoreError::Forbidden).await;
    let (missing_status, missing_body) = rendered(CoreError::SessionNotFound).await;

    assert_eq!(forbidden_status, StatusCode::NOT_FOUND);
    assert_eq!(missing_status, StatusCode::NOT_FOUND);
    assert_eq!(
        forbidden_body, missing_body,
        "a refusal that reads differently is an existence oracle"
    );
}

#[test]
fn zero_geometry_is_unprocessable_not_a_server_error() {
    let err = TerminalGeometry::new(0, 24).expect_err("zero width fails");
    let response = Problem(CoreError::InvalidGeometry(err)).into_response();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[test]
fn an_unreachable_runtime_is_service_unavailable() {
    let response = Problem(CoreError::RuntimeUnavailable("down".into())).into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[test]
fn a_session_declares_only_the_classes_it_was_granted() {
    // The interface-wide list says what REST can project for some session.
    // This says what it projects for this one, and the two are different
    // claims: a client told a screenless session conforms to the terminal
    // class finds out by asking for a screen and getting an error.
    let terminal = conformance_for(&SessionCapabilities {
        terminal: true,
        snapshot: true,
        ..Default::default()
    });
    assert!(terminal.iter().any(|c| c.ends_with("/conf/core")));
    assert!(terminal.iter().any(|c| c.ends_with("/conf/terminal")));
    assert!(
        !terminal.iter().any(|c| c.ends_with("/conf/conversation")),
        "a terminal session must not claim the conversation class: {terminal:?}"
    );

    let conversation = conformance_for(&SessionCapabilities {
        conversation: true,
        steer: true,
        usage: true,
        ..Default::default()
    });
    assert!(conversation
        .iter()
        .any(|c| c.ends_with("/conf/conversation")));
    assert!(conversation.iter().any(|c| c.ends_with("/conf/steering")));
    assert!(conversation.iter().any(|c| c.ends_with("/conf/usage")));
    assert!(
        !conversation.iter().any(|c| c.ends_with("/conf/terminal")),
        "a conversation session must not claim the terminal class: {conversation:?}"
    );
}

#[test]
fn every_session_declares_the_core_class_whatever_it_can_do() {
    // A session that was granted nothing still has a lifecycle, a log and
    // a status, and a client needs to be told it can read those.
    let none = conformance_for(&SessionCapabilities::default());
    assert_eq!(none.len(), 1);
    assert!(none[0].ends_with("/conf/core"));
}

#[test]
fn a_missing_ability_is_unprocessable_rather_than_missing() {
    // 404 would send the caller looking for a spelling mistake. The route
    // exists, the request is well formed, and this session cannot honour
    // it — which is a different thing and has to read as one.
    let response = Problem(CoreError::CapabilityUnsupported("conversation".into())).into_response();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[test]
fn a_view_carries_geometry_as_plain_integers() {
    let s = Session {
        id: SessionId("sesn_1".into()),
        owner: layer2_core::SubjectId("subject-a".into()),
        kernel: layer2_core::KernelId::new("example-runtime"),
        status: layer2_core::SessionStatus::Ready,
        recovery_state: layer2_core::RecoveryState::Normal,
        stop_reason: None,
        options: layer2_core::AgentOptions::default(),
        environment_profile: None,
        geometry: TerminalGeometry::new(120, 40).expect("valid"),
        last_seq: 3,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        created_at: "2026-08-12T00:00:00Z".into(),
    };
    let view: SessionView = s.into();
    let json = serde_json::to_value(&view).expect("serializes");
    assert_eq!(json["cols"], 120);
    assert_eq!(json["rows"], 40);
    assert!(json.get("stop_reason").is_none(), "absent reason omitted");
}

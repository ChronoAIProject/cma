//! Tests for the REST facade.
//!
//! Kept beside the module rather than inside it so that file stays within
//! the project's size limit while the behaviour remains covered.

use super::*;

const SDK_CREATE_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/create_session_request.json");
const SDK_SESSION_FIXTURE: &str = include_str!("../../tests/fixtures/rest-sdk/session.json");
const SDK_SESSIONS_FIXTURE: &str = include_str!("../../tests/fixtures/rest-sdk/sessions.json");
const SDK_PROBLEM_FIXTURE: &str =
    include_str!("../../tests/fixtures/rest-sdk/capability_refused.json");
const SDK_SCREEN_FIXTURE: &str = include_str!("../../tests/fixtures/rest-sdk/screen.json");

fn sdk_fixture(source: &str) -> serde_json::Value {
    serde_json::from_str(source).expect("the checked-in SDK fixture is JSON")
}

fn sdk_session_view() -> SessionView {
    SessionView {
        id: "sesn_fixture".into(),
        status: layer2_core::SessionStatus::Ready,
        recovery_state: layer2_core::RecoveryState::Normal,
        stop_reason: None,
        kernel: "codex".into(),
        agent: AgentView {
            model: Some("gpt-5".into()),
            permission: layer2_core::PermissionMode::Guarded,
            reasoning_effort: layer2_core::ReasoningEffort::High,
            credential: layer2_core::CredentialMode::Delegated,
            size: Some(
                layer2_core::SessionSize::new(1_000, 2_048).expect("the fixture size is non-zero"),
            ),
            skills: Some(vec![SkillView {
                id: "review".into(),
                version: Some("1.2.3".into()),
                name: Some("Repository review".into()),
                description: Some("Review a repository without changing it".into()),
            }]),
        },
        cols: 120,
        rows: 40,
        last_seq: 7,
        created_at: "2026-08-24T08:00:00Z".into(),
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        creator: Some(layer2_core::PublicIdentity {
            display_name: Some("Fixture User".into()),
            avatar_url: Some("https://images.example.test/fixture.png".into()),
        }),
        conforms_to: vec![
            format!("{CONF_BASE}/core"),
            format!("{CONF_BASE}/conversation"),
            format!("{CONF_BASE}/interrupt"),
            format!("{CONF_BASE}/usage"),
        ],
        reach: vec![ReachView {
            id: "github".into(),
            name: Some("GitHub".into()),
            description: Some("Source repositories".into()),
            mandated: false,
        }],
        pricing: Some(SessionPricingView {
            rate_card: "rate_fixture".into(),
            currency: "USD",
            micros_per_hour: "45000".into(),
            display: "$0.045".into(),
            quoted_at: "2026-08-24T07:59:00Z".into(),
        }),
    }
}

#[test]
fn sdk_create_fixture_is_parsed_by_the_server_request_type() {
    let request: CreateSessionRequest =
        serde_json::from_str(SDK_CREATE_FIXTURE).expect("server reads the fixture");
    let parts = request
        .into_parts()
        .expect("the fixture is a valid request");
    let geometry = parts.geometry.expect("the fixture names geometry");
    assert_eq!((geometry.columns(), geometry.rows()), (120, 40));
    assert_eq!(parts.environment_profile.as_deref(), Some("envp_fixture"));
    assert_eq!(parts.quoted_rate_card.as_deref(), Some("rate_fixture"));
}

#[test]
fn sdk_session_fixtures_are_produced_by_the_server_response_type() {
    let view = sdk_session_view();
    assert_eq!(
        serde_json::to_value(&view).expect("session view serializes"),
        sdk_fixture(SDK_SESSION_FIXTURE)
    );
    assert_eq!(
        serde_json::to_value([&view]).expect("session list serializes"),
        sdk_fixture(SDK_SESSIONS_FIXTURE)
    );
}

#[test]
fn sdk_screen_fixture_is_produced_by_the_server_response_type() {
    let screen = ScreenView {
        cols: 8,
        rows: 3,
        lines: vec!["hello   ".into(), "CMA     ".into(), "        ".into()],
    };
    assert_eq!(
        serde_json::to_value(screen).expect("screen serializes"),
        sdk_fixture(SDK_SCREEN_FIXTURE)
    );
}

#[tokio::test]
async fn sdk_capability_fixture_is_produced_by_the_problem_serializer() {
    let (_, body) = rendered(CoreError::CapabilityUnsupported("steer".into())).await;
    assert_eq!(body, sdk_fixture(SDK_PROBLEM_FIXTURE));
}

#[test]
fn rest_projects_every_core_and_trigger_operation() {
    // REST has no structural constraint, so a missing operation here means
    // the facade has drifted from the core rather than that the protocol
    // cannot express it.
    assert_eq!(RestInterface.operations().len(), 23);
    for operation in [
        Operation::ReadTriggerPreview,
        Operation::ManageTriggers,
        Operation::ManageTriggerConnections,
        Operation::CreateLaunch,
        Operation::ReadLaunch,
        Operation::UpdateLaunchDraft,
        Operation::StartLaunch,
        Operation::CancelLaunch,
        Operation::ReplayLaunchProgress,
        Operation::ReadLaunchDiff,
        Operation::PublishLaunchContribution,
        Operation::ManageLaunchPullRequest,
    ] {
        assert!(RestInterface.operations().contains(&operation));
    }
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

#[tokio::test]
async fn a_zero_size_is_unprocessable_and_named() {
    let error = layer2_core::SessionSize::new(0, 512).expect_err("zero CPU fails");
    let (status, body) = rendered(CoreError::InvalidSessionSize(error)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "invalid_session_size");
    assert!(body["type"]
        .as_str()
        .expect("a problem type")
        .ends_with("/invalid-session-size"));
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
        resources: Some(true),
        ..Default::default()
    });
    assert!(conversation
        .iter()
        .any(|c| c.ends_with("/conf/conversation")));
    assert!(conversation.iter().any(|c| c.ends_with("/conf/steering")));
    assert!(conversation.iter().any(|c| c.ends_with("/conf/usage")));
    assert!(conversation.iter().any(|c| c.ends_with("/conf/resources")));
    assert!(
        !conversation.iter().any(|c| c.ends_with("/conf/terminal")),
        "a conversation session must not claim the terminal class: {conversation:?}"
    );
}

#[test]
fn an_unrecorded_or_refused_resource_grant_declares_no_resource_class() {
    for capabilities in [
        SessionCapabilities::default(),
        SessionCapabilities {
            conversation: true,
            resources: Some(false),
            ..Default::default()
        },
    ] {
        let classes = conformance_for(&capabilities);
        assert!(
            !classes
                .iter()
                .any(|class| class.ends_with("/conf/resources")),
            "got {classes:?}"
        );
    }
}

#[tokio::test]
async fn an_ungranted_resource_read_is_a_named_422_not_a_404() {
    let (status, body) = rendered(CoreError::CapabilityUnsupported("resources".into())).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "capability_unsupported");
    assert_eq!(body["capability"], "resources");
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

/// What a client actually receives for a body this interface could not take.
///
/// Drives axum's real extractor rather than serde directly. The bug behind all
/// of this was end to end — a 201 on the wrong runtime — and a test that stops
/// at `serde_json::from_str` would pass even if the rejection rendered as a
/// 500.
async fn body_rejection(content_type: &str, body: &'static str) -> (StatusCode, serde_json::Value) {
    use axum::extract::FromRequest;
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/v1/sessions")
        .header("content-type", content_type)
        .body(axum::body::Body::from(body))
        .expect("request builds");
    let rejection = Json::<CreateSessionRequest>::from_request(request, &())
        .await
        .expect_err("this body must be rejected");
    let response = BodyProblem::from(rejection).into_response();
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
async fn an_unknown_field_is_refused_without_reflecting_the_body() {
    // The shape of request that produced #298: accepted, discarded, and
    // answered 201 for a session that was not what was asked for.
    let (status, body) = body_rejection(
        "application/json",
        r#"{"cols":120,"rows":32,"nonsense":"claude_code"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "invalid_request");
    assert_eq!(
        body["detail"],
        "The JSON body does not match this operation's request schema."
    );
    let rendered = serde_json::to_string(&body).expect("problem serializes");
    assert!(!rendered.contains("claude_code"));
}

#[tokio::test]
async fn a_misspelled_profile_no_longer_starts_the_wrong_session() {
    // The silent case nobody would have reported. `environmentProfile` is the
    // camelCase spelling this project's own SDK and its AG-UI interface both
    // use, and it used to start a session on the deployment's default profile
    // instead of the named one — a working session, quietly not the one asked
    // for.
    let (status, body) = body_rejection(
        "application/json",
        r#"{"cols":120,"rows":32,"environmentProfile":"envp_1"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "invalid_request");
    assert_eq!(
        body["detail"],
        "The JSON body does not match this operation's request schema."
    );
}

#[tokio::test]
async fn a_bad_body_keeps_the_status_that_says_what_is_wrong() {
    // Three conditions, three statuses, because each sends the reader
    // somewhere different. Answering all of them 422 would send somebody whose
    // header is wrong to go and stare at their JSON.
    let (status, body) = body_rejection("application/json", r#"{"cols":"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "malformed json is 400");
    assert_eq!(body["code"], "malformed_json");

    let (status, body) = body_rejection("text/plain", r#"{"cols":120}"#).await;
    assert_eq!(
        status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "a wrong content type is 415"
    );
    assert_eq!(body["code"], "unsupported_media_type");
}

#[tokio::test]
async fn every_failure_renders_the_same_document_shape() {
    // A caller writing one parser for this API's failures must not meet a
    // second shape because the wrong half of their request was at fault.
    let (_, from_body) = body_rejection("application/json", r#"{"nope":1}"#).await;
    let (_, from_core) = rendered(CoreError::SessionTerminated).await;
    for field in ["type", "title", "status", "detail", "code"] {
        assert!(from_body.get(field).is_some(), "body problem lacks {field}");
        assert!(from_core.get(field).is_some(), "core problem lacks {field}");
    }
}

#[test]
fn a_named_kernel_is_refused_and_told_where_to_put_it() {
    // Guards the branch, not the parse. The test this replaces asserted that
    // `kernel` deserialises — the opposite of refused — so deleting the whole
    // refusal left 135 tests green and #298 reopened in silence. Verified by
    // deleting it: nothing failed.
    let request: CreateSessionRequest =
        serde_json::from_str(r#"{"cols":120,"rows":32,"kernel":"claude_code"}"#)
            .expect("a declared field parses; the refusal is the handler's job");

    let refusal = request
        .into_parts()
        .expect_err("a named kernel must not reach the core");
    assert_eq!(refusal.code(), "invalid_request");

    // §3's bar is a refusal in words a caller can act on, which means naming
    // the remedy and not only the mistake.
    let said = refusal.to_string();
    for expected in [
        "kernel",
        "environment profile",
        "environment_profile",
        "agent.kernel",
    ] {
        assert!(
            said.contains(expected),
            "the refusal must mention {expected}: {said}"
        );
    }
}

#[test]
fn a_request_without_a_kernel_passes_through_checked() {
    // The other half. A gate that refuses everything is not a gate, and the
    // geometry has to survive the trip.
    let parts = serde_json::from_str::<CreateSessionRequest>(
        r#"{"cols":120,"rows":32,"environment_profile":"envp_1","quoted_rate_card":"rate_1"}"#,
    )
    .expect("the declared fields are accepted")
    .into_parts()
    .expect("nothing here is unhonourable");
    assert_eq!(parts.environment_profile.as_deref(), Some("envp_1"));
    assert_eq!(parts.quoted_rate_card.as_deref(), Some("rate_1"));
    assert!(parts.geometry.is_some());

    // Half a geometry is not a size, so it is dropped rather than guessed at.
    let half = serde_json::from_str::<CreateSessionRequest>(r#"{"cols":120}"#)
        .expect("cols alone parses")
        .into_parts()
        .expect("and is not itself an error");
    assert!(half.geometry.is_none());
}

#[test]
fn a_well_formed_create_body_still_parses() {
    // The other half. A gate that refuses everything is not a gate.
    let body = serde_json::from_str::<CreateSessionRequest>(
        r#"{"cols":120,"rows":32,"environment_profile":"envp_1"}"#,
    )
    .expect("the declared fields are still accepted");
    assert_eq!(body.cols, Some(120));
    assert_eq!(body.environment_profile.as_deref(), Some("envp_1"));

    // And every field is optional, which is what an unqualified session is.
    let bare = serde_json::from_str::<CreateSessionRequest>("{}").expect("a bare create is valid");
    assert!(bare.cols.is_none() && bare.environment_profile.is_none());
}

#[test]
fn priced_and_unpriced_quote_responses_have_distinct_exact_shapes() {
    let priced = PricingQuoteView::from(Some(layer2_core::Quote {
        rate_card: layer2_core::RateCardId("rate_exact".into()),
        rate: layer2_core::Rate {
            cpu_micros_per_cpu_hour: 40_000,
            memory_micros_per_gib_hour: 5_000,
        },
        cpu_millis: 1_000,
        memory_mib: 1_024,
        micros_per_hour: 45_000,
        display: "$0.045".into(),
        currency: layer2_core::CURRENCY,
        quoted_at: "2026-08-23T05:00:00Z".into(),
    }));
    assert_eq!(
        serde_json::to_value(priced).expect("quote serializes"),
        serde_json::json!({
            "rate_card": "rate_exact",
            "currency": "USD",
            "cpu_millis": 1000,
            "memory_mib": 1024,
            "micros_per_hour": "45000",
            "display": "$0.045",
            "per": "hour",
            "quoted_at": "2026-08-23T05:00:00Z"
        })
    );

    assert_eq!(
        serde_json::to_value(PricingQuoteView::from(None)).expect("absence serializes"),
        serde_json::json!({"priced": false})
    );
}

#[tokio::test]
async fn a_refusal_names_the_missing_ability_as_a_field() {
    // §3 requires a refusal to name what is missing "in words a caller can act
    // on". The name used to reach a caller only inside `detail`, as the tail
    // of "capability not supported: conversation" — so acting on it meant
    // parsing an English sentence, which every client would do slightly
    // differently and which breaks the first time the wording is improved.
    let (status, body) = rendered(CoreError::CapabilityUnsupported("conversation".into())).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["capability"], "conversation");
    assert_eq!(body["code"], "capability_unsupported");
    // The prose keeps saying it too: a caller that only shows `detail` to a
    // person must not start showing them less.
    assert!(
        body["detail"]
            .as_str()
            .expect("detail is a string")
            .contains("conversation"),
        "the sentence should still name it: {}",
        body["detail"]
    );
}

#[tokio::test]
async fn a_failure_with_no_ability_to_name_carries_no_field() {
    // Absent rather than null. A field present on every failure invites a
    // caller to branch on its emptiness instead of on the code, and the two
    // disagree the first time a refusal arrives without a name.
    let (_, body) = rendered(CoreError::SessionTerminated).await;
    assert!(
        body.get("capability").is_none(),
        "nothing to name, so nothing should be there: {body}"
    );
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
        granted_services: None,
        granted_skills: None,
        pricing: None,
        created_at: "2026-08-12T00:00:00Z".into(),
    };
    let view: SessionView = s.into();
    let json = serde_json::to_value(&view).expect("serializes");
    assert_eq!(json["cols"], 120);
    assert_eq!(json["rows"], 40);
    assert!(json.get("stop_reason").is_none(), "absent reason omitted");
    assert!(json.get("pricing").is_none(), "absent price omitted");
}

#[test]
fn a_view_projects_the_pinned_price_without_rounding() {
    let s = Session {
        id: SessionId("sesn_priced".into()),
        owner: layer2_core::SubjectId("subject-a".into()),
        kernel: layer2_core::KernelId::new("example-runtime"),
        status: layer2_core::SessionStatus::Ready,
        recovery_state: layer2_core::RecoveryState::Normal,
        stop_reason: None,
        options: layer2_core::AgentOptions::default(),
        environment_profile: None,
        geometry: TerminalGeometry::default(),
        last_seq: 3,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: None,
        granted_skills: None,
        pricing: Some(layer2_core::SessionPricing {
            rate_card: layer2_core::RateCardId("rate_exact".into()),
            rate: layer2_core::Rate {
                cpu_micros_per_cpu_hour: 40_000,
                memory_micros_per_gib_hour: 5_000,
            },
            cpu_millis: 1_000,
            memory_mib: 1_024,
            micros_per_hour: 45_000,
            quoted_at: "2026-08-23T05:00:00Z".into(),
        }),
        created_at: "2026-08-23T05:00:01Z".into(),
    };

    let json = serde_json::to_value(SessionView::from(s)).expect("session serializes");
    assert_eq!(
        json["pricing"],
        serde_json::json!({
            "rate_card": "rate_exact",
            "currency": "USD",
            "micros_per_hour": "45000",
            "display": "$0.045",
            "quoted_at": "2026-08-23T05:00:00Z"
        })
    );
}

/// The exact document a client parses.
///
/// One test on each side of the wire, over the same JSON. This one asserts the
/// backend emits it; `web/src/sdk/sessions.test.ts` asserts the client reads
/// that document back into the model its surfaces draw.
///
/// Field-by-field assertions on one side and a hand-written fixture on the
/// other are how the two drift: each stays green while disagreeing about a
/// name. Sharing the document is what stops that.
#[test]
fn a_view_carries_the_agent_configuration_a_client_draws() {
    let s = Session {
        id: SessionId("sesn_1".into()),
        owner: layer2_core::SubjectId("subject-a".into()),
        kernel: layer2_core::KernelId::new("codex"),
        status: layer2_core::SessionStatus::Ready,
        recovery_state: layer2_core::RecoveryState::Normal,
        stop_reason: None,
        options: layer2_core::AgentOptions {
            model: Some("gpt-5-codex".into()),
            reasoning_effort: layer2_core::ReasoningEffort::XHigh,
            permission: layer2_core::PermissionMode::Unrestricted,
            credential: layer2_core::CredentialMode::Own,
            size: Some(layer2_core::SessionSize::new(1_500, 3_072).expect("valid size")),
            ..Default::default()
        },
        environment_profile: None,
        geometry: TerminalGeometry::new(120, 40).expect("valid"),
        last_seq: 3,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: None,
        granted_skills: Some(vec![layer2_core::GrantedSkill {
            id: layer2_core::SkillId::new("skl_nyxid"),
            version: Some("0.5".into()),
        }]),
        pricing: None,
        created_at: "2026-08-12T00:00:00Z".into(),
    };
    let json = serde_json::to_value(SessionView::from(s)).expect("serializes");

    assert_eq!(json["agent"]["model"], "gpt-5-codex");
    // Not `xhigh`. `as_str` spells it that way and serde does not, and only
    // serde reaches a client — see `option_wire_names_are_what_clients_read`.
    assert_eq!(json["agent"]["reasoning_effort"], "x_high");
    assert_eq!(json["agent"]["permission"], "unrestricted");
    assert_eq!(json["agent"]["credential"], "own");
    assert_eq!(json["agent"]["size"]["cpu_millis"], 1_500);
    assert_eq!(json["agent"]["size"]["memory_mib"], 3_072);
    assert_eq!(json["agent"]["skills"][0]["id"], "skl_nyxid");
    // The version that arrived, recorded when the pack was fetched. Asking the
    // registry now would answer with whatever is current, which is a different
    // fact and wrong for any session that has been running a while.
    assert_eq!(json["agent"]["skills"][0]["version"], "0.5");
    // Unnamed until `skilled` has asked the catalogue, and absent rather than
    // null so a client tells "no name" from "the name is empty".
    assert!(json["agent"]["skills"][0].get("name").is_none());
}

#[test]
fn a_session_with_no_recorded_skills_carries_no_skills_field() {
    /*
     * Absent and empty are different answers all the way to the wire.
     *
     * A session created before the grant was recorded has nothing to say;
     * one that was given no skills has said something. A client draws the
     * two differently, so collapsing them here would make that impossible.
     */
    let s = Session {
        id: SessionId("sesn_1".into()),
        owner: layer2_core::SubjectId("subject-a".into()),
        kernel: layer2_core::KernelId::new("codex"),
        status: layer2_core::SessionStatus::Ready,
        recovery_state: layer2_core::RecoveryState::Normal,
        stop_reason: None,
        options: layer2_core::AgentOptions::default(),
        environment_profile: None,
        geometry: TerminalGeometry::default(),
        last_seq: 0,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: None,
        granted_skills: None,
        pricing: None,
        created_at: "2026-08-12T00:00:00Z".into(),
    };
    let unrecorded = serde_json::to_value(SessionView::from(s.clone())).expect("serializes");
    assert!(
        unrecorded["agent"].get("skills").is_none(),
        "nothing recorded, so nothing on the wire: {unrecorded}"
    );

    let given_none = Session {
        granted_skills: Some(Vec::new()),
        ..s
    };
    let json = serde_json::to_value(SessionView::from(given_none)).expect("serializes");
    assert_eq!(
        json["agent"]["skills"],
        serde_json::json!([]),
        "given none is an answer and must be sent: {json}"
    );

    // And the defaults are named rather than omitted: a client that draws
    // "not reported" for an absent field would draw it for every session
    // nobody configured, which is most of them.
    assert_eq!(json["agent"]["permission"], "guarded");
    assert_eq!(json["agent"]["credential"], "delegated");
    assert_eq!(json["agent"]["reasoning_effort"], "high");
    assert!(
        json["agent"].get("model").is_none(),
        "no model chosen is absent, not empty: {json}"
    );
}

/// What a session reports it may reach — the other half of its capabilities.
mod reach {
    use super::*;
    use layer2_core::{
        Baseline, CatalogService, EnvironmentProfile, EnvironmentProfileId,
        EnvironmentProfileService, EnvironmentProfileSpec, EnvironmentProfileStore, Page,
        PageRequest, ServiceId, StoreError,
    };

    /// A store that holds nothing: these tests never read a profile, which is
    /// the whole point of the projection they cover.
    struct NoProfiles;

    #[async_trait::async_trait]
    impl EnvironmentProfileStore for NoProfiles {
        async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
            Ok(())
        }
        async fn replace_content(
            &self,
            _id: &EnvironmentProfileId,
            _content: &EnvironmentProfileSpec,
            _updated_at: &str,
        ) -> Result<Option<EnvironmentProfile>, StoreError> {
            Ok(None)
        }
        async fn get(
            &self,
            _id: &EnvironmentProfileId,
        ) -> Result<Option<EnvironmentProfile>, StoreError> {
            Ok(None)
        }
        async fn list_for(
            &self,
            _owner: &layer2_core::SubjectId,
            _page: &PageRequest,
        ) -> Result<Page<EnvironmentProfile>, StoreError> {
            Ok(Page {
                items: Vec::new(),
                next: None,
            })
        }
        async fn set_preferred(
            &self,
            _owner: &layer2_core::SubjectId,
            chosen: Option<&EnvironmentProfileId>,
        ) -> Result<(), StoreError> {
            if chosen.is_some() {
                return Err(StoreError::Malformed(
                    "an empty store has no selectable profile".into(),
                ));
            }
            Ok(())
        }
        async fn preferred_for(
            &self,
            _owner: &layer2_core::SubjectId,
        ) -> Result<Option<EnvironmentProfile>, StoreError> {
            Ok(None)
        }
        async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
            Ok(())
        }
    }

    fn session_with(granted: Option<Vec<ServiceId>>) -> Session {
        Session {
            granted_skills: None,
            pricing: None,
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
            granted_services: granted,
            created_at: "2026-08-12T00:00:00Z".into(),
        }
    }

    /// A catalogue that answers, or refuses, exactly as NyxID would.
    #[derive(Default)]
    struct Catalogue {
        services: Vec<CatalogService>,
        unavailable: bool,
    }

    #[async_trait::async_trait]
    impl layer2_core::ServiceCatalog for Catalogue {
        async fn available(
            &self,
            _subject: &layer2_core::SubjectId,
            _credential: Option<&str>,
        ) -> Result<Vec<CatalogService>, layer2_core::CatalogError> {
            if self.unavailable {
                return Err(layer2_core::CatalogError::Unavailable("down".into()));
            }
            Ok(self.services.clone())
        }
    }

    fn environments(catalogue: Catalogue) -> Arc<EnvironmentProfileService> {
        EnvironmentProfileService::new(
            Arc::new(NoProfiles),
            Baseline::new([], [], [ServiceId::new("nyxid")]),
            None,
            Some(Arc::new(catalogue)),
            Default::default(),
            Arc::new(layer2_core::PlatformSettingsService::unconfigured()),
        )
    }

    fn caller() -> Caller {
        Caller::new(
            layer2_core::SubjectId("subject-a".into()),
            layer2_core::Authorization::from_capabilities([
                layer2_core::Capability::ManageOwnEnvironmentProfiles,
            ]),
        )
    }

    /// A directory that knows nobody, which is all these need.
    struct NoDirectory;

    #[async_trait::async_trait]
    impl layer2_core::UserDirectoryStore for NoDirectory {
        async fn record_sign_in(
            &self,
            _user: &layer2_core::RegisteredUser,
        ) -> Result<(), layer2_core::StoreError> {
            Ok(())
        }
        async fn recent(
            &self,
            _query: &layer2_core::DirectoryQuery,
        ) -> Result<Vec<layer2_core::RegisteredUser>, layer2_core::StoreError> {
            Ok(Vec::new())
        }
        async fn by_subjects(
            &self,
            _subjects: &[layer2_core::SubjectId],
        ) -> Result<Vec<layer2_core::RegisteredUser>, layer2_core::StoreError> {
            Ok(Vec::new())
        }
    }

    /// The step a handler must not skip when answering with a session.
    ///
    /// `From<Session>` seeds `reach` empty for `reached` to fill, and an empty
    /// reach is a real answer here — it means the session reaches nothing. So
    /// a handler that answered before filling it said something false about
    /// every session it had just created, and both ends were reading their own
    /// contract correctly. `viewed` is the one path that cannot skip it.
    #[tokio::test]
    async fn rendering_a_session_reports_what_it_was_granted() {
        let views = super::viewed(
            &environments(Catalogue::default()),
            &layer2_core::UserDirectory::new(Arc::new(NoDirectory)),
            &caller(),
            None,
            vec![session_with(Some(vec![ServiceId::new("chrono-llm")]))],
        )
        .await;
        assert_eq!(views[0].reach.len(), 1, "the granted service is reported");
        assert_eq!(views[0].reach[0].id, "chrono-llm");
    }

    async fn rendered(session: Session, catalogue: Catalogue) -> serde_json::Value {
        let sessions = vec![session];
        let mut views: Vec<SessionView> = sessions.iter().cloned().map(SessionView::from).collect();
        reached(
            &environments(catalogue),
            &caller(),
            None,
            &sessions,
            &mut views,
        )
        .await;
        serde_json::to_value(&views[0]).expect("serializes")
    }

    #[tokio::test]
    async fn names_a_service_the_catalogue_knows() {
        // A raw service identifier is not something an owner recognises.
        let json = rendered(
            session_with(Some(vec![
                ServiceId::new("github"),
                ServiceId::new("nyxid"),
            ])),
            Catalogue {
                services: vec![CatalogService {
                    id: ServiceId::new("github"),
                    name: "GitHub".into(),
                    description: Some("Repositories and pull requests".into()),
                }],
                unavailable: false,
            },
        )
        .await;

        assert_eq!(json["reach"][0]["id"], "github");
        assert_eq!(json["reach"][0]["name"], "GitHub");
        assert_eq!(
            json["reach"][0]["description"],
            "Repositories and pull requests"
        );
    }

    #[tokio::test]
    async fn marks_what_the_platform_mandates() {
        // So a client renders it as fixed rather than guessing which entries
        // cannot be removed, exactly as the profile view already does.
        let json = rendered(
            session_with(Some(vec![
                ServiceId::new("github"),
                ServiceId::new("nyxid"),
            ])),
            Catalogue::default(),
        )
        .await;

        assert_eq!(json["reach"][0]["mandated"], false);
        assert_eq!(json["reach"][1]["mandated"], true);
    }

    #[tokio::test]
    async fn an_unreachable_catalogue_costs_the_names_and_not_the_answer() {
        // "We cannot ask right now" is not "you may reach nothing". Returning
        // an error here would report a transient outage as a session with no
        // reach, which is the confusion this whole projection exists to avoid.
        let json = rendered(
            session_with(Some(vec![ServiceId::new("github")])),
            Catalogue {
                services: Vec::new(),
                unavailable: true,
            },
        )
        .await;

        assert_eq!(json["reach"][0]["id"], "github");
        assert!(json["reach"][0].get("name").is_none());
    }

    #[tokio::test]
    async fn carries_no_credential_material() {
        // What a session may reach is a fact about its configuration; what it
        // holds is not. Nothing here may be usable to fingerprint a token.
        let json = rendered(
            session_with(Some(vec![ServiceId::new("github")])),
            Catalogue::default(),
        )
        .await;

        let fields: Vec<&str> = json["reach"][0]
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(fields, ["id", "mandated"]);
    }

    #[tokio::test]
    async fn a_session_recorded_before_the_grant_existed_reports_nothing_rather_than_guessing() {
        // The projection reports what the session recorded. A view that fell
        // back to the profile here would be the wrong answer dressed as the
        // right one — the core's own fallback exists for minting a credential,
        // where there is no other option, and not for telling somebody what a
        // session reaches.
        let json = rendered(session_with(None), Catalogue::default()).await;
        assert_eq!(json["reach"].as_array().expect("an array").len(), 0);
    }
}

/// A caller can tell the three service refusals apart from the response alone.
mod service_refusals {
    use super::*;

    #[tokio::test]
    async fn an_ungranted_service_is_unprocessable_and_never_unavailable() {
        // Not transient. Retrying will not grant it, and a 503 would tell a
        // client to keep trying something that can only be fixed in NyxID.
        let (status, body) = rendered(CoreError::ServiceNotGranted("api-github".into())).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "service_not_granted");
        assert!(
            body["detail"]
                .as_str()
                .expect("a detail")
                .contains("api-github"),
            "a refusal that does not name the service is one nobody can act on"
        );
    }

    #[tokio::test]
    async fn an_unreachable_catalogue_is_unavailable_and_never_unprocessable() {
        // Transient. The owner's services are fine and we could not list them,
        // so retrying is exactly the right thing to do.
        let (status, body) = rendered(CoreError::ServiceCatalogUnavailable("down".into())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "service_catalogue_unavailable");
    }

    #[tokio::test]
    async fn the_two_are_distinguishable_by_type_rather_than_by_string() {
        // #132's whole point. A client matching on prose is a client that
        // breaks when somebody rewords a sentence.
        let (granted_status, granted) =
            rendered(CoreError::ServiceNotGranted("api-github".into())).await;
        let (down_status, down) = rendered(CoreError::ServiceCatalogUnavailable("x".into())).await;

        assert_ne!(granted_status, down_status);
        assert_ne!(granted["code"], down["code"]);
        assert_ne!(granted["type"], down["type"]);
    }

    #[tokio::test]
    async fn neither_is_a_bare_not_found() {
        // A 404 sends the caller looking for a spelling mistake. Both routes
        // exist and both requests are well-formed.
        for refusal in [
            CoreError::ServiceNotGranted("api-github".into()),
            CoreError::ServiceCatalogUnavailable("down".into()),
        ] {
            let (status, _) = rendered(refusal).await;
            assert_ne!(status, StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn a_catalogue_outage_is_not_reported_as_the_sandbox_provider() {
        // A caller told "runtime unavailable" goes and looks at the sandbox
        // provider, which has nothing to do with it.
        let (_, catalogue) = rendered(CoreError::ServiceCatalogUnavailable("down".into())).await;
        let (_, runtime) = rendered(CoreError::RuntimeUnavailable("down".into())).await;
        assert_ne!(catalogue["code"], runtime["code"]);
    }
}

//! Tests for the operator surface's rendering and its refusals.
//!
//! What is asserted here is what reaches a client: which refusal shape a
//! caller sees, and — most importantly — that a provider nobody could ask is
//! never rendered as a provider running nothing.

use super::*;

use layer2_core::{
    ActivityAction, ActivityId, ActivityOutcome, ActivityTarget, DetailKey, EnvironmentProfileId,
    KernelId, ProviderInventory, RuntimeEntry, SessionStatus,
};

fn inventory(providers: Vec<ProviderInventory>) -> RuntimeInventory {
    RuntimeInventory { providers }
}

fn observed_provider() -> ProviderInventory {
    ProviderInventory {
        kernel: KernelId::new("example-runtime"),
        view: ProviderView::Observed(vec![RuntimeEntry {
            created_by: None,
            runtime: "runtime-1".into(),
            session: Some(SessionId("sesn_1".into())),
            owner: Some(SubjectId("subject-a".into())),
            status: Some(SessionStatus::Ready),
        }]),
    }
}

#[test]
fn a_provider_that_could_not_be_asked_renders_with_no_runtime_list_at_all() {
    // The single most important line in this file. If the unobservable case
    // rendered an empty array, a client would draw an idle namespace and an
    // operator would believe it.
    let view = InventoryView::render(inventory(vec![ProviderInventory {
        kernel: KernelId::new("example-runtime"),
        view: ProviderView::Unobservable("cannot enumerate".into()),
    }]));
    let json = serde_json::to_value(&view).expect("serialises");

    assert_eq!(json["providers"][0]["observable"], false);
    assert_eq!(json["providers"][0]["reason"], "cannot enumerate");
    assert!(
        json["providers"][0].get("runtimes").is_none(),
        "an empty array here would read as nothing running: {json}"
    );
    assert!(
        json.get("running").is_none(),
        "a total that omits a provider nobody could ask is not a total: {json}"
    );
    assert_eq!(json["fully_observed"], false);
}

#[test]
fn a_provider_running_nothing_renders_an_empty_list_and_a_total_of_zero() {
    // The other half: an answer of zero is an answer, and must be visibly
    // different from not having asked.
    let view = InventoryView::render(inventory(vec![ProviderInventory {
        kernel: KernelId::new("example-runtime"),
        view: ProviderView::Observed(Vec::new()),
    }]));
    let json = serde_json::to_value(&view).expect("serialises");

    assert_eq!(json["providers"][0]["observable"], true);
    assert!(json["providers"][0]["runtimes"]
        .as_array()
        .expect("an array")
        .is_empty());
    assert_eq!(json["running"], 0);
    assert_eq!(json["fully_observed"], true);
}

#[test]
fn a_running_runtime_renders_with_the_session_and_the_owner_it_belongs_to() {
    let view = InventoryView::render(inventory(vec![observed_provider()]));
    let json = serde_json::to_value(&view).expect("serialises");
    let runtime = &json["providers"][0]["runtimes"][0];

    assert_eq!(runtime["runtime"], "runtime-1");
    assert_eq!(runtime["session"], "sesn_1");
    assert_eq!(runtime["owner"], "subject-a");
    assert_eq!(runtime["status"], "ready");
    assert_eq!(runtime["unaccounted"], false);
    assert_eq!(json["running"], 1);
}

#[test]
fn a_runtime_nothing_accounts_for_says_so() {
    let view = InventoryView::render(inventory(vec![ProviderInventory {
        kernel: KernelId::new("example-runtime"),
        view: ProviderView::Observed(vec![RuntimeEntry {
            created_by: None,
            runtime: "runtime-stray".into(),
            session: Some(SessionId("sesn_forgotten".into())),
            owner: None,
            status: None,
        }]),
    }]));
    let json = serde_json::to_value(&view).expect("serialises");
    let runtime = &json["providers"][0]["runtimes"][0];

    assert_eq!(runtime["unaccounted"], true);
    assert!(runtime.get("owner").is_none(), "got {runtime}");
}

#[test]
fn a_person_renders_by_name_with_the_identifier_alongside_it() {
    let view = UserView::from(RegisteredUser {
        subject: SubjectId("76fe9d91".into()),
        display_name: Some("Ada Lovelace".into()),
        email: Some("ada@example.com".into()),
        avatar_url: None,
        first_seen: "2026-08-01T00:00:00Z".into(),
        last_seen: "2026-08-14T00:00:00Z".into(),
    });
    let json = serde_json::to_value(&view).expect("serialises");

    assert_eq!(json["label"], "Ada Lovelace");
    assert_eq!(json["email"], "ada@example.com");
    assert_eq!(
        json["subject"], "76fe9d91",
        "clients key on the identifier, they simply must not display it as a name"
    );
}

#[test]
fn a_person_the_provider_named_nothing_for_carries_no_label_to_render() {
    let view = UserView::from(RegisteredUser {
        subject: SubjectId("76fe9d91".into()),
        display_name: None,
        email: None,
        avatar_url: None,
        first_seen: "2026-08-01T00:00:00Z".into(),
        last_seen: "2026-08-14T00:00:00Z".into(),
    });
    let json = serde_json::to_value(&view).expect("serialises");
    assert!(json.get("label").is_none(), "got {json}");
    assert!(json.get("display_name").is_none(), "got {json}");
}

#[test]
fn a_session_renders_with_its_owner_named_where_the_directory_knows_one() {
    let mut names = HashMap::new();
    names.insert(SubjectId("subject-a".into()), "Ada Lovelace".to_string());
    let session = Session {
        id: SessionId("sesn_1".into()),
        owner: SubjectId("subject-a".into()),
        kernel: KernelId::new("example-runtime"),
        status: SessionStatus::Ready,
        recovery_state: layer2_core::RecoveryState::Normal,
        stop_reason: None,
        options: layer2_core::AgentOptions::default(),
        environment_profile: None,
        geometry: layer2_core::TerminalGeometry::default(),
        last_seq: 4,
        runtime_version: None,
        rotations: 0,
        rotated_at: None,
        capabilities: Default::default(),
        granted_services: None,
        granted_skills: None,
        created_at: "2026-08-14T00:00:00Z".into(),
    };

    let json = serde_json::to_value(AdminSessionView::render(session.clone(), &names))
        .expect("serialises");
    assert_eq!(json["owner"], "subject-a");
    assert_eq!(json["owner_label"], "Ada Lovelace");

    // A session whose owner the directory has never seen still renders; the
    // absence is what tells a client to fall back rather than draw a blank.
    let json = serde_json::to_value(AdminSessionView::render(session, &HashMap::new()))
        .expect("serialises");
    assert!(json.get("owner_label").is_none(), "got {json}");
    assert_eq!(json["owner"], "subject-a");
}

#[test]
fn a_recorded_action_renders_with_who_did_what_to_what_and_how_it_went() {
    let view = ActivityView::from(
        ActivityRecord {
            id: ActivityId("actv_1".into()),
            at: "2026-08-14T00:00:00Z".into(),
            actor: Some(SubjectId("operator".into())),
            action: ActivityAction::SessionTerminate,
            target: Some(ActivityTarget::Session(SessionId("sesn_1".into()))),
            outcome: ActivityOutcome::Succeeded,
            detail: layer2_core::ActivityDetail::new(),
        }
        .with(DetailKey::Runtime, "example-runtime"),
    );
    let json = serde_json::to_value(&view).expect("serialises");

    assert_eq!(json["actor"], "operator");
    assert_eq!(json["action"], "session_terminate");
    assert_eq!(json["target"]["kind"], "session");
    assert_eq!(json["target"]["id"], "sesn_1");
    assert_eq!(json["outcome"], "succeeded");
    assert_eq!(json["detail"]["runtime"], "example-runtime");
}

#[test]
fn an_unattributed_action_renders_without_an_actor_rather_than_a_null() {
    let view = ActivityView::from(ActivityRecord::new(
        ActivityAction::SessionList,
        ActivityOutcome::Denied,
    ));
    let json = serde_json::to_value(&view).expect("serialises");
    assert!(json.get("actor").is_none(), "got {json}");
    assert!(json.get("target").is_none(), "got {json}");
    assert_eq!(json["outcome"], "denied");
}

#[test]
fn a_target_of_any_kind_renders_with_the_kind_it_is() {
    let view = ActivityView::from(
        ActivityRecord::new(
            ActivityAction::EnvironmentProfileDelete,
            ActivityOutcome::Succeeded,
        )
        .on(ActivityTarget::EnvironmentProfile(EnvironmentProfileId(
            "envp_1".into(),
        ))),
    );
    let json = serde_json::to_value(&view).expect("serialises");
    assert_eq!(json["target"]["kind"], "environment_profile");
    assert_eq!(json["target"]["id"], "envp_1");
}

#[test]
fn a_caller_who_is_not_an_operator_is_told_so_rather_than_told_nothing_exists() {
    // These collections are addressed by nothing, so there is no identifier to
    // protect by pretending it is missing.
    let response = AdminProblem::from(CoreError::Forbidden).into_response();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[test]
fn an_unreachable_store_is_not_flattened_into_a_refusal() {
    // The failure a reader most needs to tell apart from an empty answer.
    let response =
        AdminProblem::from(CoreError::RuntimeUnavailable("store down".into())).into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn acting_on_a_session_keeps_the_session_surfaces_refusal() {
    // These routes do address an identifier, so a refusal must not reveal
    // whether the session exists — and "must not reveal" means the whole
    // document, not the status line. An operator control route is reachable
    // by any caller entitled to end their own session, so the caller who
    // would exploit a difference here is an ordinary one.
    let mut bodies = Vec::new();
    for error in [CoreError::Forbidden, CoreError::SessionNotFound] {
        let response = Problem(error).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("problem documents are small");
        bodies.push(bytes);
    }
    assert_eq!(
        bodies[0], bodies[1],
        "a refusal that reads differently confirms the identifier exists"
    );
}

#[test]
fn every_operator_route_is_under_one_segment_a_gateway_can_restrict() {
    // The reason these live at their own address rather than beside the
    // owner's own routes. Asserted against the list because a route added
    // outside the segment would quietly escape whatever protects it.
    for path in OPERATOR_ROUTES {
        assert!(
            path.starts_with("/api/v1/admin/"),
            "{path} escapes the segment the whole surface is restricted by"
        );
    }
}

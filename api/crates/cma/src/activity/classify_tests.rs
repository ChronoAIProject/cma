//! Tests for turning a request into the core's vocabulary.
//!
//! The most important test in this file is the last group: a request carrying
//! a caller's secrets must produce an entry that contains none of them. It is
//! written against the real translation, with realistic bodies, because the
//! rule is only worth anything if it holds for what callers actually send.

use super::*;

use layer2_core::{ActivityOutcome, ActivityRecord};

fn classified(method: Method, path: &str) -> Classified {
    classify(&method, path).unwrap_or_else(|| panic!("{method} {path} should be classified"))
}

#[test]
fn a_session_is_classified_with_the_identifier_it_names() {
    let c = classified(Method::POST, "/api/v1/sessions/sesn_1/terminate");
    assert_eq!(c.action, Action::SessionTerminate);
    assert_eq!(
        c.target,
        Some(ActivityTarget::Session(SessionId("sesn_1".into())))
    );
    assert!(!c.reads_body);
}

#[test]
fn every_session_verb_is_recognised() {
    for (method, verb, action) in [
        (Method::POST, "terminate", Action::SessionTerminate),
        (Method::POST, "stop", Action::SessionStop),
        (Method::POST, "revive", Action::SessionRevive),
        (Method::GET, "screen", Action::SessionScreenRead),
        (Method::GET, "transcript", Action::SessionTranscriptRead),
        (Method::GET, "events", Action::SessionEventsRead),
        (Method::GET, "terminal", Action::SessionTerminalAttach),
        (Method::POST, "plane", Action::SessionPlaneChange),
    ] {
        let c = classified(method.clone(), &format!("/api/v1/sessions/sesn_1/{verb}"));
        assert_eq!(c.action, action, "{method} {verb}");
    }
}

#[test]
fn a_collection_is_classified_without_a_target() {
    let c = classified(Method::GET, "/api/v1/sessions");
    assert_eq!(c.action, Action::SessionList);
    assert!(
        c.target.is_none(),
        "listing addresses no one thing, and inventing one would misdescribe it"
    );
}

#[test]
fn the_method_decides_between_two_actions_at_one_address() {
    assert_eq!(
        classified(Method::POST, "/api/v1/sessions").action,
        Action::SessionCreate
    );
    assert_eq!(
        classified(Method::GET, "/api/v1/sessions").action,
        Action::SessionList
    );
    assert_eq!(
        classified(Method::DELETE, "/api/v1/sessions/sesn_1").action,
        Action::SessionDelete
    );
}

#[test]
fn the_skill_catalogue_is_matched_ahead_of_the_identifier_beside_it() {
    // The router matches the static segment first, and this has to agree or a
    // catalogue read would be recorded as reading a bundle called "skills".
    let c = classified(Method::GET, "/api/v1/environment-profiles/skills");
    assert_eq!(c.action, Action::SkillCatalogRead);
    assert!(c.target.is_none());

    let c = classified(Method::GET, "/api/v1/environment-profiles/envp_1");
    assert_eq!(c.action, Action::EnvironmentProfileRead);
    assert_eq!(
        c.target,
        Some(ActivityTarget::EnvironmentProfile(EnvironmentProfileId(
            "envp_1".into()
        )))
    );
}

#[test]
fn the_operator_surface_is_classified_too() {
    assert_eq!(
        classified(Method::GET, "/api/v1/admin/users").action,
        Action::UserDirectoryRead
    );
    assert_eq!(
        classified(Method::GET, "/api/v1/admin/activity").action,
        Action::ActivityLogRead
    );
    assert_eq!(
        classified(Method::GET, "/api/v1/admin/runtimes").action,
        Action::RuntimeInventoryRead
    );
    // An operator ending somebody else's session is exactly what the record
    // exists for, so this must classify with the session it names.
    let c = classified(Method::POST, "/api/v1/admin/sessions/sesn_1/terminate");
    assert_eq!(c.action, Action::SessionTerminate);
    assert_eq!(
        c.target,
        Some(ActivityTarget::Session(SessionId("sesn_1".into())))
    );

    let list = classified(Method::GET, "/api/v1/admin/chat/hosts");
    assert_eq!(list.action, Action::ChatHostList);
    assert!(list.target.is_none());
    let retire = classified(Method::POST, "/api/v1/admin/chat/hosts/chho_example/retire");
    assert_eq!(retire.action, Action::ChatHostRetire);
    assert_eq!(
        retire.target,
        Some(ActivityTarget::ChatHost(ChatHostId("chho_example".into())))
    );
    assert!(!retire.reads_body);
}

#[test]
fn trigger_recovery_actions_are_distinct_and_target_the_launch() {
    let owner = classified(
        Method::POST,
        "/api/v1/launches/launch-recovery/recover-initial-prompt",
    );
    assert_eq!(owner.action, Action::TriggerLaunchPromptRecover);
    assert_eq!(
        owner.target,
        Some(ActivityTarget::Launch(LaunchId("launch-recovery".into())))
    );
    assert!(
        !owner.reads_body,
        "the retained prompt hash and risk acknowledgement are owner-private confirmation data"
    );

    let operator = classified(
        Method::POST,
        "/api/v1/admin/triggers/launches/launch-recovery/reconcile",
    );
    assert_eq!(operator.action, Action::TriggerEffectRecoveryAuthorize);
    assert_eq!(operator.target, owner.target);
    assert!(operator.reads_body);
}

#[test]
fn trigger_connection_revocation_is_targeted_without_recording_a_body() {
    let classified = classified(Method::DELETE, "/api/v1/trigger-connections/service-exact");
    assert_eq!(classified.action, Action::TriggerConnectionRevoke);
    assert_eq!(
        classified.target,
        Some(ActivityTarget::TriggerConnection("service-exact".into()))
    );
    assert!(
        !classified.reads_body,
        "the empty revocation command must not create an audit-body channel"
    );
}

#[test]
fn effect_recovery_audit_keeps_only_closed_evidence_and_never_provider_material() {
    let body = br#"{
        "expected_revision": 17,
        "effect": "pull_request",
        "intent_key": "must-not-be-retained",
        "expected_fence": 9,
        "evidence_correlation_id": "failure-correlation-9",
        "provider_body": "must-not-be-retained",
        "credential": "must-not-be-retained",
        "raw_object_reference": "must-not-be-retained"
    }"#;
    let detail = detail_from_body(Action::TriggerEffectRecoveryAuthorize, body);

    assert_eq!(detail.get(DetailKey::Effect), Some("pull_request"));
    assert_eq!(detail.get(DetailKey::FencingToken), Some("9"));
    assert_eq!(detail.get(DetailKey::CorrelationId), None);
    assert_eq!(detail.iter().count(), 2);
    let rendered = serde_json::to_string(&detail).unwrap();
    assert!(!rendered.contains("must-not-be-retained"), "got {rendered}");
    assert!(
        !rendered.contains("failure-correlation-9"),
        "got {rendered}"
    );
    assert!(!rendered.contains("expected_revision"), "got {rendered}");
}

#[test]
fn every_platform_settings_operation_is_classified_without_reading_its_body() {
    for (method, path, action) in [
        (
            Method::GET,
            "/api/v1/admin/settings",
            Action::PlatformSettingsRead,
        ),
        (
            Method::PATCH,
            "/api/v1/admin/settings",
            Action::PlatformSettingsWrite,
        ),
        (
            Method::PUT,
            "/api/v1/admin/settings",
            Action::PlatformSettingsWrite,
        ),
        (
            Method::POST,
            "/api/v1/admin/settings/reset",
            Action::PlatformSettingsReset,
        ),
    ] {
        let classified = classified(method, path);
        assert_eq!(classified.action, action);
        assert!(classified.target.is_none());
        assert!(
            !classified.reads_body,
            "administrator prose must never be buffered into the activity record"
        );
    }

    assert!(classify(&Method::POST, "/api/v1/admin/settings").is_none());
    assert!(classify(&Method::GET, "/api/v1/admin/settings/reset").is_none());
}

#[test]
fn a_probe_or_an_unserved_path_is_not_recorded() {
    // Probes arrive every few seconds and ask for nothing; a path this service
    // does not serve did nothing, whoever sent it. Recording either would bury
    // the entries that matter.
    for path in [
        "/livez",
        "/ready",
        "/",
        "/api/v1/conformance",
        "/api/v1/kernels",
    ] {
        assert!(
            classify(&Method::GET, path).is_none(),
            "{path} should not be recorded"
        );
    }
    assert!(classify(&Method::GET, "/wp-admin/setup.php").is_none());
    assert!(classify(&Method::PATCH, "/api/v1/sessions/sesn_1").is_none());
}

#[test]
fn signing_out_is_recorded_and_signing_in_is_left_to_the_flow() {
    // The request that starts a sign-in carries no identity yet, so the flow
    // that completes it records that one; there is nothing to attribute here.
    assert_eq!(
        classified(Method::GET, "/auth/logout").action,
        Action::SignOut
    );
    assert!(classify(&Method::GET, "/auth/login").is_none());
    assert!(classify(&Method::GET, "/auth/callback").is_none());
    // Polled constantly by any signed-in page, and it changes nothing.
    assert!(classify(&Method::GET, "/auth/session").is_none());
}

#[test]
fn a_creation_request_records_the_choices_that_shaped_the_session() {
    let body = br#"{"kernel":"example-runtime","cols":120,"rows":40,
                    "environment_profile":"envp_1",
                    "options":{"model":"some-model","permission":"unrestricted"}}"#;
    let detail = detail_from_body(Action::SessionCreate, body);

    assert_eq!(detail.get(DetailKey::Runtime), Some("example-runtime"));
    assert_eq!(detail.get(DetailKey::Columns), Some("120"));
    assert_eq!(detail.get(DetailKey::Rows), Some("40"));
    assert_eq!(detail.get(DetailKey::EnvironmentProfile), Some("envp_1"));
    assert_eq!(detail.get(DetailKey::Model), Some("some-model"));
    assert_eq!(
        detail.get(DetailKey::PermissionMode),
        Some("unrestricted"),
        "an agent allowed to act unattended is the thing somebody asks about afterwards"
    );
}

#[test]
fn a_bundle_records_how_many_entries_it_carried_and_never_which() {
    let body = br#"{"name":"build tools",
                    "variables":[{"key":"A","value":"1"},{"key":"B","value":"2"}],
                    "packages":[{"name":"ripgrep"}],
                    "skills":["s1","s2","s3"]}"#;
    let detail = detail_from_body(Action::EnvironmentProfileCreate, body);

    assert_eq!(detail.get(DetailKey::ResourceName), Some("build tools"));
    assert_eq!(detail.get(DetailKey::VariableCount), Some("2"));
    assert_eq!(detail.get(DetailKey::PackageCount), Some("1"));
    assert_eq!(detail.get(DetailKey::SkillCount), Some("3"));
}

#[test]
fn a_body_that_cannot_be_parsed_records_nothing_rather_than_failing() {
    // The request is about to be refused for the same reason; the entry saying
    // so is worth more than no entry at all.
    let detail = detail_from_body(Action::SessionCreate, b"not json at all");
    assert!(detail.is_empty());
}

#[test]
fn an_action_with_nothing_in_its_body_records_nothing_from_one() {
    let detail = detail_from_body(Action::SessionTerminate, br#"{"kernel":"anything"}"#);
    assert!(detail.is_empty());
}

#[test]
fn both_event_cursor_spellings_produce_the_same_activity_detail() {
    let detail_for = |key: &str| {
        let mut detail = ActivityDetail::new();
        detail_from_query(Some(&format!("{key}=1042")), &mut detail);
        detail
    };

    let canonical = detail_for("starting_after");
    let alias = detail_for("after");
    assert_eq!(canonical, alias);
    assert_eq!(canonical.get(DetailKey::Cursor), Some("1042"));
    assert_eq!(canonical.iter().count(), 1);
}

#[test]
fn a_cursor_and_a_limit_are_read_from_a_query_and_nothing_else_is() {
    let mut detail = ActivityDetail::new();
    detail_from_query(
        Some("starting_after=1042&limit=50&actor=76fe9d91&unexpected=should-never-appear"),
        &mut detail,
    );

    assert_eq!(detail.get(DetailKey::Cursor), Some("1042"));
    assert_eq!(detail.get(DetailKey::Limit), Some("50"));
    assert_eq!(detail.get(DetailKey::Subject), Some("76fe9d91"));
    let rendered = serde_json::to_string(&detail).expect("serialises");
    assert!(
        !rendered.contains("should-never-appear"),
        "a query parameter nobody allowlisted must not be kept: {rendered}"
    );
}

#[test]
fn a_query_value_that_is_not_a_number_where_one_is_expected_is_ignored() {
    for query in [
        "starting_after=yesterday&limit=all",
        "after=yesterday&limit=all",
    ] {
        let mut detail = ActivityDetail::new();
        detail_from_query(Some(query), &mut detail);
        assert!(detail.is_empty(), "for {query}");
    }
}

#[test]
fn a_request_carrying_a_secret_produces_an_entry_that_does_not_contain_it() {
    // The rule this whole design exists for, stated end to end. A bundle's
    // variable values are where a caller is invited to put a credential; the
    // entry that records the request must contain the shape of it and none of
    // the substance.
    // A value with no entropy and no meaning, so this fixture cannot be
    // mistaken for a real credential by anything reading the repository. What
    // is under test is that it does not travel, not what it is.
    let planted = "not-a-real-value-at-all";
    let body = format!(
        r#"{{"name":"deploy settings",
             "variables":[{{"key":"PRIVATE_SETTING","value":"{planted}"}},
                          {{"key":"ANOTHER_SETTING","value":"{planted}"}}],
             "packages":[],"skills":[]}}"#
    );

    let detail = detail_from_body(Action::EnvironmentProfileCreate, body.as_bytes());
    let entry = {
        let mut e =
            ActivityRecord::new(Action::EnvironmentProfileCreate, ActivityOutcome::Succeeded);
        e.detail = detail;
        e
    };

    // Checked against the serialised entry, because that is what is written
    // down and handed back: anything hiding in a field would show up here.
    let rendered = serde_json::to_string(&entry).expect("serialises");
    assert!(
        !rendered.contains(planted),
        "a value the caller may treat as a credential reached the record: {rendered}"
    );
    assert!(
        !rendered.contains("PRIVATE_SETTING"),
        "even the names of a caller's variables stay out: {rendered}"
    );
    assert_eq!(
        entry.detail.get(DetailKey::VariableCount),
        Some("2"),
        "the shape is kept, so an operator can still see what was done"
    );
    assert_eq!(
        entry.detail.get(DetailKey::ResourceName),
        Some("deploy settings")
    );
}

#[test]
fn a_replaced_bundle_leaks_no_more_than_a_created_one() {
    // The same body arrives at a different address on an edit, and the rule
    // has to hold there too.
    let planted = "a-different-value-of-no-meaning";
    let body = format!(r#"{{"name":"n","variables":[{{"key":"K","value":"{planted}"}}]}}"#);
    let detail = detail_from_body(Action::EnvironmentProfileReplace, body.as_bytes());
    let rendered = serde_json::to_string(&detail).expect("serialises");

    assert!(!rendered.contains(planted), "got {rendered}");
    assert_eq!(detail.get(DetailKey::VariableCount), Some("1"));
    assert!(
        classified(Method::PUT, "/api/v1/environment-profiles/envp_1").reads_body,
        "an edit has to be read for its shape to be recorded at all"
    );
}

#[test]
fn asking_for_an_attach_ticket_records_the_attach_it_is_for() {
    let c = classify(&Method::POST, "/api/v1/sessions/sesn_1/terminal-ticket").expect("classified");
    assert!(matches!(c.action, Action::SessionTerminalAttach));
    assert!(
        !c.reads_body,
        "the body decides only whether to supersede, and is nobody's business afterwards"
    );
}

#[test]
fn a_ticket_never_reaches_a_record() {
    // The property the whole ticket design rests on: a credential in a URL is
    // acceptable only while nothing writes it down.
    let mut detail = ActivityDetail::new();
    detail_from_query(
        Some("ticket=aaaaaaaabbbbbbbbccccccccdddddddd&starting_after=7"),
        &mut detail,
    );
    let rendered = format!("{detail:?}");
    assert!(
        !rendered.contains("aaaaaaaa"),
        "a ticket was recorded: {rendered}"
    );
    assert!(
        rendered.contains('7'),
        "the allowlisted cursor should still be kept: {rendered}"
    );
}

#[test]
fn every_operating_verb_is_recorded_as_its_own_action() {
    // Grouping them would make "somebody typed" and "somebody stopped the
    // agent" the same entry, which is the distinction anybody reading this log
    // after an incident is looking for.
    for (verb, expected) in [
        ("input", Action::SessionInput),
        ("interrupt", Action::SessionInterrupt),
        ("steer", Action::SessionSteer),
        ("resize", Action::SessionResize),
    ] {
        let classified = classify(&Method::POST, &format!("/api/v1/sessions/sesn_1/{verb}"))
            .unwrap_or_else(|| panic!("{verb} is served"));
        assert_eq!(classified.action, expected, "for {verb}");
        assert_eq!(
            classified.target,
            Some(ActivityTarget::Session(SessionId("sesn_1".into())))
        );
    }
}

#[test]
fn nothing_a_caller_sends_to_a_session_is_read_into_the_record() {
    // Keystrokes and prompts are the owner's own words to their own agent, and
    // an approval payload is a decision about their own work. The guarantee is
    // structural: these classifications read no body at all, so there is
    // nowhere for any of it to land however the request shape grows.
    for path in [
        "/api/v1/sessions/sesn_1/input",
        "/api/v1/sessions/sesn_1/steer",
        "/api/v1/sessions/sesn_1/interrupt",
        "/api/v1/sessions/sesn_1/resize",
        "/api/v1/sessions/sesn_1/approvals/req_1",
    ] {
        let classified =
            classify(&Method::POST, path).unwrap_or_else(|| panic!("{path} is served"));
        assert!(
            !classified.reads_body,
            "{path} must not read a body into the record"
        );
    }
}

#[test]
fn answering_an_approval_names_the_session_rather_than_the_request() {
    // Which request was answered is in the session's own event log, where it
    // belongs to the conversation. This log is about who touched what.
    let classified = classify(&Method::POST, "/api/v1/sessions/sesn_1/approvals/req_9")
        .expect("approvals are served");
    assert_eq!(classified.action, Action::SessionApprovalResolve);
    assert_eq!(
        classified.target,
        Some(ActivityTarget::Session(SessionId("sesn_1".into())))
    );
}

#[test]
fn reading_usage_is_a_read_rather_than_an_operation() {
    let classified =
        classify(&Method::GET, "/api/v1/sessions/sesn_1/usage").expect("usage is served");
    assert_eq!(classified.action, Action::SessionUsageRead);
}

#[test]
fn an_operating_verb_asked_for_with_the_wrong_method_is_not_served() {
    // The router would answer 405, and a record of an action that never
    // happened is worse than no record.
    assert!(classify(&Method::GET, "/api/v1/sessions/sesn_1/input").is_none());
    assert!(classify(&Method::POST, "/api/v1/sessions/sesn_1/usage").is_none());
}

#[test]
fn moving_a_session_between_planes_records_which_plane_and_nothing_else() {
    // The one session verb whose body says something worth keeping. Which
    // surface a session is driven on is exactly what somebody reading this
    // record needs to make sense of a handover that was asked for; everything
    // else a caller sends to a session is their own content.
    let c = classified(Method::POST, "/api/v1/sessions/sesn_1/plane");
    assert_eq!(c.action, Action::SessionPlaneChange);
    assert_eq!(
        c.target,
        Some(ActivityTarget::Session(SessionId("sesn_1".into())))
    );
    assert!(c.reads_body, "the plane is in the body");

    let detail = detail_from_body(
        Action::SessionPlaneChange,
        br#"{"plane":"conversation","note":"something the caller added later"}"#,
    );
    assert_eq!(detail.get(DetailKey::Plane), Some("conversation"));
    assert_eq!(
        detail.iter().count(),
        1,
        "a field added to this request later has nowhere here to land"
    );
}

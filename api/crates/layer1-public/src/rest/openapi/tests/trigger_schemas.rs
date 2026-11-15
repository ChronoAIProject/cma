use super::*;

#[test]
fn every_trigger_and_launch_wire_schema_is_registered() {
    let document = document();
    let schemas = &document.components.expect("components").schemas;
    for name in [
        "SignupOptions",
        "EmbedDescriptor",
        "EmbedSnippets",
        "TriggerButtonLabelValue",
        "TriggerThemeValue",
        "SandboxReusePolicyValue",
        "WorkspaceReusePolicyValue",
        "AgentReusePolicyValue",
        "TriggerPresentationBody",
        "TriggerTargetBody",
        "TriggerReuseBody",
        "TriggerConsentBody",
        "TriggerContentBody",
        "CreateTriggerRequest",
        "UpdateTriggerRequest",
        "PublishTriggerRequest",
        "CreatePlacementRequest",
        "CreateLaunchRequest",
        "StartLaunchRequest",
        "CreateServiceConnectionRequest",
        "TriggerAvailabilityValue",
        "TriggerStatsView",
        "PlacementView",
        "TriggerRevisionView",
        "PublishedTriggerRevisionView",
        "TriggerView",
        "TriggerPageView",
        "TriggerPlacementStatsView",
        "TriggerStatisticsView",
        "TriggerTrustDisclosureView",
        "TriggerTrustPreviewView",
        "TriggerSizePreviewView",
        "TriggerTechnicalPreviewView",
        "TriggerPreviewView",
        "ResolvedResourceView",
        "LaunchResolutionView",
        "ServiceRequirementView",
        "RequirementsSnapshotView",
        "ReviewNeedView",
        "ReviewRequirementsView",
        "LaunchTrustResourceValue",
        "LaunchTrustDisclosureView",
        "LaunchTrustManifestView",
        "LaunchConsentSurfaceValue",
        "LaunchConfirmationView",
        "LaunchView",
        "LaunchFirstMessageCommandView",
        "LaunchPageView",
        "LaunchEventPageView",
        "ServiceConnectionStateValue",
        "CreatedServiceConnectionView",
        "ServiceConnectionStatusView",
        "LaunchStateValue",
        "LaunchStepKeyValue",
        "LaunchStepStatusValue",
        "RepeatChoiceValue",
        "ServiceRequirementStateValue",
        "FirstMessageStateValue",
        "LaunchEffectKindValue",
        "LaunchStepView",
        "LaunchBlockedReasonView",
        "LaunchFailureView",
        "LaunchEventKindView",
        "LaunchEventView",
    ] {
        assert!(schemas.contains_key(name), "missing schema {name}");
    }
    let schemas = serde_json::to_value(schemas).expect("schemas serialize");
    let association = &schemas["LaunchFirstMessageCommandView"]["properties"];
    assert_eq!(association.as_object().unwrap().len(), 3);
    for field in ["agent_id", "command_id", "response_id"] {
        assert_eq!(association[field]["type"], "string");
    }
    assert!(!schemas["LaunchView"]["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("first_message_command")));
}

#[test]
fn inline_trigger_target_schema_exposes_image_agent_and_repository_contract() {
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    let schemas = &document["components"]["schemas"];
    for name in [
        "TriggerImageBody",
        "TriggerRepositoryBody",
        "TriggerInlineWorkspaceBody",
        "TriggerInlineAgentBody",
        "TriggerInlineTargetBody",
        "TriggerSkillBody",
    ] {
        assert!(
            schemas[name].is_object(),
            "missing inline trigger schema {name}"
        );
    }

    assert_eq!(
        schema_ref(&schemas["TriggerTargetBody"]["properties"]["inline"]),
        "#/components/schemas/TriggerInlineTargetBody"
    );
    assert!(schemas["TriggerInlineTargetBody"]["required"]
        .as_array()
        .expect("inline target required fields")
        .contains(&serde_json::json!("agent")));
    assert_eq!(
        schemas["TriggerInlineAgentBody"]["properties"]["reasoning_effort"]["$ref"],
        "#/components/schemas/ThinkingEffortValue"
    );
    assert_eq!(
        schemas["TriggerInlineAgentBody"]["properties"]["permission_mode"]["$ref"],
        "#/components/schemas/PermissionLevelValue"
    );
    let repository_reference_type = &schemas["TriggerRepositoryBody"]["properties"]["ref"]["type"];
    assert!(
        repository_reference_type == "string"
            || repository_reference_type
                .as_array()
                .is_some_and(|types| types.contains(&serde_json::json!("string")))
    );
    assert_eq!(
        schema_ref(&schemas["TriggerInlineWorkspaceBody"]["properties"]["repository"]),
        "#/components/schemas/TriggerRepositoryBody"
    );
}

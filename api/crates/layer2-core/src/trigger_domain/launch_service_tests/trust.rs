use super::*;

#[tokio::test]
async fn explicit_no_review_start_accepts_exact_foreign_configuration_without_extra_confirmation() {
    let fixture = fixture(default_reuse(), PUBLISHER, true, false, vec![]).await;
    let mut request = fixture.request("external-button");
    request.start_if_ready = true;
    let launch = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
        .await
        .unwrap()
        .launch;
    assert_eq!(launch.state, LaunchState::Resolving);
    assert!(launch.review.needs.is_empty());
    assert_eq!(launch.confirmations.len(), 1);
    assert_eq!(
        launch.confirmations[0].trust_confirmed_digest,
        launch.trust.confirmation_digest
    );
    assert!(launch.trust.confirmation_digest.is_some());
    launch.validate().unwrap();
}

#[tokio::test]
async fn review_needs_are_ordered_and_trust_is_needed_only_for_foreign_unsafe_creation() {
    let services = vec![
        ServiceId::new("github"),
        ServiceId::new("slack"),
        ServiceId::new("docs"),
    ];
    let foreign = fixture(default_reuse(), PUBLISHER, true, true, services).await;
    foreign.broker.set(vec![
        requirement("github", ServiceRequirementState::Missing),
        requirement("slack", ServiceRequirementState::Inactive),
        requirement("docs", ServiceRequirementState::Satisfied),
    ]);
    let launch = foreign
        .service
        .create_or_resume(
            &foreign.caller,
            &foreign.public.public_id,
            foreign.request("review"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(
        launch.review.needs,
        vec![
            ReviewNeed::Connection {
                service: ServiceId::new("github")
            },
            ReviewNeed::Connection {
                service: ServiceId::new("slack")
            },
            ReviewNeed::TrustConfirmation {
                commands_digest: foreign.resolved_profiles.sandbox.digest.clone()
            },
            ReviewNeed::OwnerRequiresReview,
        ]
    );

    foreign.set_context(ExistingContext::SandboxPaused);
    let reused = foreign
        .service
        .create_or_resume(
            &foreign.caller,
            &foreign.public.public_id,
            foreign.request("review-reused"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(
        reused.review.needs,
        vec![
            ReviewNeed::Connection {
                service: ServiceId::new("github")
            },
            ReviewNeed::Connection {
                service: ServiceId::new("slack")
            },
            ReviewNeed::OwnerRequiresReview,
        ]
    );

    for (owner, unsafe_profile) in [(VISITOR, true), (PUBLISHER, false)] {
        let fixture = fixture(default_reuse(), owner, unsafe_profile, false, vec![]).await;
        let launch = fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request(format!("trust-{owner}-{unsafe_profile}")),
            )
            .await
            .unwrap()
            .launch;
        assert!(launch.review.needs.is_empty());
    }

    let nested =
        fixture_with_workspace_setup(default_reuse(), VISITOR, false, false, vec![], true).await;
    let launch = nested
        .service
        .create_or_resume(
            &nested.caller,
            &nested.public.public_id,
            nested.request("nested-trust"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(
        launch.review.needs,
        vec![ReviewNeed::TrustConfirmation {
            commands_digest: nested.resolved_profiles.sandbox.digest.clone(),
        }]
    );

    let ad_hoc = composite_trust_fixture().await;
    let ad_hoc_launch = ad_hoc
        .service
        .create_or_resume(
            &ad_hoc.caller,
            &ad_hoc.public.public_id,
            ad_hoc.request("ad-hoc-composite-trust"),
        )
        .await
        .unwrap()
        .launch;
    let ReviewNeed::TrustConfirmation { commands_digest } = &ad_hoc_launch.review.needs[0] else {
        panic!("expected trust confirmation");
    };
    assert_ne!(commands_digest, &ad_hoc.resolved_profiles.sandbox.digest);
    assert_ne!(commands_digest, &ad_hoc.resolved_profiles.workspace.digest);
    assert_eq!(commands_digest.len(), 64);
    assert_eq!(
        ad_hoc_launch.trust.confirmation_digest.as_ref(),
        Some(commands_digest)
    );
    assert_eq!(ad_hoc_launch.trust.disclosures.len(), 2);
    assert_eq!(
        ad_hoc_launch.trust.disclosures[0].commands,
        vec!["install-tool"]
    );
    assert_eq!(
        ad_hoc_launch.trust.disclosures[1].commands,
        vec!["prepare-workspace"]
    );
    let started = ad_hoc
        .service
        .start(
            &ad_hoc.caller,
            &ad_hoc_launch.id,
            StartLaunch {
                trust_confirmed_digest: Some(commands_digest.clone()),
                acknowledgements: vec![],
            },
        )
        .await
        .unwrap();
    assert_eq!(started.state, LaunchState::Resolving);

    let reused_ad_hoc = fixture_with_workspace_configuration(
        ReusePolicy {
            sandbox: SandboxReusePolicy::ReuseExisting,
            workspace: WorkspaceReusePolicy::AlwaysNew,
            agent: AgentReusePolicy::AlwaysNew,
        },
        VISITOR,
        false,
        false,
        vec![],
        true,
        false,
        false,
    )
    .await;
    reused_ad_hoc.set_context(ExistingContext::SandboxPaused);
    let workspace_only = reused_ad_hoc
        .service
        .create_or_resume(
            &reused_ad_hoc.caller,
            &reused_ad_hoc.public.public_id,
            reused_ad_hoc.request("ad-hoc-workspace-trust"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(
        workspace_only.review.needs,
        vec![ReviewNeed::TrustConfirmation {
            commands_digest: reused_ad_hoc.resolved_profiles.workspace.digest.clone(),
        }]
    );
    assert_eq!(workspace_only.trust.disclosures.len(), 1);
    assert_eq!(
        workspace_only.trust.disclosures[0].commands,
        vec!["prepare-workspace"]
    );
}

#[tokio::test]
async fn profile_display_renames_do_not_strand_frozen_trust() {
    let fixture = composite_trust_fixture().await;
    let launch = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("rename-after-admission"),
        )
        .await
        .unwrap()
        .launch;
    let confirmed_digest = launch.trust.confirmation_digest.clone();

    let sandbox_id = &fixture.public.revision.content.target.sandbox_profile.id;
    let mut sandbox = fixture
        .profiles
        .sandbox_profile(sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let sandbox_version = sandbox.draft_version;
    sandbox.name = "Renamed Sandbox".into();
    sandbox.draft_version += 1;
    sandbox.updated_at = "2026-09-05T00:02:00Z".into();
    fixture
        .profiles
        .save_sandbox_draft(sandbox_version, sandbox)
        .await
        .unwrap();

    let workspace_id = &fixture.public.revision.content.target.workspace_profile.id;
    let mut workspace = fixture
        .profiles
        .workspace_profile(workspace_id)
        .await
        .unwrap()
        .unwrap();
    let workspace_version = workspace.draft_version;
    workspace.name = "Renamed Workspace".into();
    workspace.draft_version += 1;
    workspace.updated_at = "2026-09-05T00:02:00Z".into();
    fixture
        .profiles
        .save_workspace_draft(workspace_version, workspace)
        .await
        .unwrap();

    let started = fixture
        .service
        .start(
            &fixture.caller,
            &launch.id,
            StartLaunch {
                trust_confirmed_digest: confirmed_digest,
                acknowledgements: vec![],
            },
        )
        .await
        .unwrap();
    assert_eq!(started.state, LaunchState::Resolving);
    assert_eq!(started.trust, launch.trust);
}

#[tokio::test]
async fn repository_nested_workspace_uses_its_frozen_derived_name() {
    let fixture = fixture_with_workspace_configuration(
        default_reuse(),
        VISITOR,
        false,
        false,
        vec![],
        true,
        true,
        true,
    )
    .await;
    let launch = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("repository-nested-trust"),
        )
        .await
        .unwrap()
        .launch;
    assert_eq!(launch.trust.disclosures.len(), 1);
    assert_eq!(launch.trust.disclosures[0].name, "launch-repo");
    assert_eq!(
        launch.trust.disclosures[0].commands,
        vec!["prepare-workspace"]
    );
}

async fn composite_trust_fixture() -> Fixture {
    fixture_with_workspace_configuration(
        ReusePolicy {
            sandbox: SandboxReusePolicy::AlwaysNew,
            workspace: WorkspaceReusePolicy::AlwaysNew,
            agent: AgentReusePolicy::AlwaysNew,
        },
        PUBLISHER,
        true,
        false,
        vec![],
        true,
        false,
        false,
    )
    .await
}

use super::*;
use crate::store::agent_access_fixtures as fixtures;
use crate::store::agent_access_requests::AgentAccessProgress;
use layer2_core::resource_access::ResourceOwner;
use layer2_core::sandbox::agent_access_tool::{AgentAccessProposal, AgentAccessRequester};
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use layer2_core::sandbox::{AgentCreationProvenance, RuntimeBinding, SandboxWorkStep};
use layer2_core::ServiceId;

fn ready() -> (AgentAccessRecord, Agent, Sandbox) {
    let mut sandbox = fixtures::sandbox("notification");
    sandbox.runtime = Some(RuntimeBinding {
        runtime_id: "revived-runtime".into(),
        runtime_version: "pinned-image".into(),
        provision_generation: Some(2),
        boot_id: Some("b".repeat(32)),
        bound_at: fixtures::NOW.into(),
    });
    sandbox.credential.reaches = vec![ServiceId::new("github")];
    sandbox.credential.current.as_mut().unwrap().expires_at = layer2_core::rfc3339_at(
        std::time::SystemTime::now() + std::time::Duration::from_secs(3600),
    );
    let workspace = fixtures::workspace(&sandbox, "notification");
    let mut agent = fixtures::admission(&workspace, "notification")
        .materialize(&workspace)
        .unwrap()
        .agent;
    agent.conversation.conversation_id = Some("original-conversation".into());
    agent.conversation.runtime_generation = Some(2);
    let creation = AgentCreationProvenance::for_admission(
        &agent,
        &sandbox,
        ResourceOwner::Person {
            subject: sandbox.owner.clone(),
        },
        None,
    )
    .unwrap();
    let record = AgentAccessRecord {
        scope_mode: Default::default(),
        expected_scope_revision: None,
        id: "access-request-original".into(),
        requester: AgentAccessRequester {
            agent_id: agent.id.clone(),
            owner: agent.owner.clone(),
            sandbox_id: sandbox.id.clone(),
            conversation: "original-conversation".into(),
            runtime: sandbox.runtime.clone().unwrap(),
            creation,
        },
        proposal: AgentAccessProposal {
            retry_of: None,
            request_key: "original-key".into(),
            services: vec!["github".into()],
            reason: "Read repository".into(),
        },
        body_sha256: "a".repeat(64),
        phase: Phase::Notifying,
        decision: Some(AgentAccessDecision::Approved {
            command_key_hash: "b".repeat(64),
            services: vec![ServiceId::new("github")],
            connection: None,
            connection_binding: None,
            authorization: AgentAuthorizationRevision {
                replay_principal: None,
                authority_id: "personal:owner".into(),
                policy_revision: 1,
                grant_revision: None,
            },
        }),
        lease: None,
        progress: AgentAccessProgress {
            delivered_credential: sandbox.credential.current.clone(),
            delivered_runtime: sandbox.runtime.clone(),
            ..Default::default()
        },
        approved_at: Some(fixtures::NOW.into()),
        retry_at: 0,
        created_at: fixtures::NOW.into(),
        updated_at: fixtures::NOW.into(),
        revision: 1,
        error: None,
    };
    (record, agent, sandbox)
}

#[test]
fn notification_requires_live_delivered_scope_in_the_original_conversation() {
    let (record, agent, sandbox) = ready();
    // A normally active managed-key Sandbox waits for its next rotation.
    assert_eq!(
        sandbox.work.step,
        SandboxWorkStep::WaitForCredentialRotation
    );
    assert!(notification_ready(&record, &agent, &sandbox).unwrap());
    for case in 0..9 {
        let mut changed = sandbox.clone();
        let mut changed_agent = agent.clone();
        match case {
            0 => changed.credential.delivered = false,
            1 => changed.credential.pending = changed.credential.current.clone(),
            2 => changed.credential.reaches.clear(),
            3 => changed.credential.current.as_mut().unwrap().expires_at = fixtures::NOW.into(),
            4 => changed.status = SandboxStatus::Paused,
            5 => changed.runtime = None,
            6 => changed_agent.conversation.conversation_id = Some("replacement".into()),
            7 => changed_agent.conversation.runtime_generation = Some(1),
            _ => changed.work.step = SandboxWorkStep::DeliverRotatedCredential,
        }
        assert!(
            !notification_ready(&record, &changed_agent, &changed).unwrap(),
            "case {case}"
        );
    }
}

#[test]
fn ordinary_rotation_after_delivery_does_not_strand_the_owner_callback() {
    let (record, agent, mut sandbox) = ready();
    let current = sandbox.credential.current.as_mut().unwrap();
    current.provider_key_id = "replacement-key".into();
    assert!(!notification_ready(&record, &agent, &sandbox).unwrap());
    sandbox.credential.current.as_mut().unwrap().generation += 1;
    assert!(notification_ready(&record, &agent, &sandbox).unwrap());
}

#[test]
fn notification_replay_body_is_independent_of_runtime_and_worker_takeover() {
    let (record, _, _) = ready();
    let expected = notification(&record).unwrap();
    let mut recovered = record.clone();
    recovered.revision += 20;
    recovered
        .progress
        .delivered_runtime
        .as_mut()
        .unwrap()
        .boot_id = Some("c".repeat(32));
    recovered
        .progress
        .delivered_credential
        .as_mut()
        .unwrap()
        .provider_key_id = "secret-authority-id".into();
    recovered.proposal.reason =
        "Untrusted request text must not become a system instruction".into();
    assert_eq!(notification(&recovered).unwrap(), expected);
    assert!(!expected.contains("secret-authority-id"));
    assert!(!expected.contains(&recovered.proposal.reason));
    assert!(expected.contains("original task in this conversation"));
}

#[test]
fn hot_notification_requires_exact_scope_revision_and_complete_old_key_revocation() {
    use crate::store::agent_access_requests::AgentAccessHotUpdate;
    use layer2_core::sandbox::SandboxServiceScopeRevision;
    let (mut record, agent, mut sandbox) = ready();
    let floor = sandbox
        .credential
        .current
        .as_ref()
        .unwrap()
        .generation
        .saturating_sub(1);
    record.progress.hot_update = Some(AgentAccessHotUpdate {
        runtime: sandbox.runtime.clone().unwrap(),
        lifecycle_next_authority: sandbox.lifecycle.next_authority,
    });
    record.progress.grant_generation_floor = Some(floor);
    record.expected_scope_revision = Some(2);
    sandbox.credential.service_scope = Some(SandboxServiceScopeRevision {
        revision: 3,
        services: record.approved_services().unwrap().to_vec(),
        connection: None,
        authorized_by: record.requester.owner.clone(),
        authorized_at: fixtures::NOW.into(),
        credential_generation_floor: floor,
        runtime: sandbox.runtime.clone().unwrap(),
        runtime_services: sandbox.credential.effective_reaches(),
    });
    assert!(notification_ready(&record, &agent, &sandbox).unwrap());
    for case in 0..7 {
        let mut changed = sandbox.clone();
        match case {
            0 => changed
                .credential
                .queue_revoke("old-provider-key".into(), fixtures::NOW.into()),
            1 => changed.credential.service_scope.as_mut().unwrap().revision += 1,
            2 => changed
                .credential
                .service_scope
                .as_mut()
                .unwrap()
                .services
                .push(ServiceId::new("unapproved")),
            3 => {
                changed
                    .credential
                    .service_scope
                    .as_mut()
                    .unwrap()
                    .authorized_by = layer2_core::SubjectId::new("foreign-owner")
            }
            4 => {
                changed
                    .credential
                    .service_scope
                    .as_mut()
                    .unwrap()
                    .credential_generation_floor += 1
            }
            5 => changed.lifecycle.next_authority += 1,
            _ => changed.runtime.as_mut().unwrap().boot_id = Some("d".repeat(32)),
        }
        assert!(
            !notification_ready(&record, &agent, &changed).unwrap(),
            "case {case}"
        );
    }
    sandbox.credential.current.as_mut().unwrap().generation += 1;
    assert!(
        notification_ready(&record, &agent, &sandbox).unwrap(),
        "ordinary later credential rotation retains the exact approved scope"
    );
}

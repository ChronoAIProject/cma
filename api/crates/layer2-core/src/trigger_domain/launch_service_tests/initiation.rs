use super::*;
use crate::resource_access::{CallerContext, CredentialContext, ResourceActor, ResourceOwner};
use crate::sandbox::store::AgentAdmission;
use crate::sandbox::{AgentCreationOrigin, AgentCreationProvenance};

fn context(f: &Fixture, api: bool) -> CallerContext {
    CallerContext {
        caller: f.caller.clone(),
        credential: CredentialContext {
            issuer: "https://nyx.example".into(),
            subject: f.caller.subject.clone(),
            credential_id: "verified-request".into(),
        },
        actor: if api {
            ResourceActor::Api {
                subject: f.caller.subject.clone(),
                delegation_id: None,
            }
        } else {
            ResourceActor::Human {
                subject: f.caller.subject.clone(),
            }
        },
        memberships: vec![],
    }
}

#[tokio::test]
async fn verified_launch_initiator_is_distinct_from_trigger_creator_and_replay_retains_original_actor(
) {
    for api in [false, true] {
        let f = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
        let identity = context(&f, api);
        let created = f
            .service
            .create_or_resume_as(&identity, &f.public.public_id, f.request("actor-original"))
            .await
            .unwrap();
        let initiation = created.launch.initiation.as_ref().unwrap();
        assert_eq!(initiation.actor(), &identity.actor);
        let replay = f
            .service
            .create_or_resume_as(
                &context(&f, !api),
                &f.public.public_id,
                f.request("actor-original"),
            )
            .await
            .unwrap();
        assert!(!replay.created);
        assert_eq!(replay.launch, created.launch);
        assert!(!serde_json::to_value(&created.launch)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("initiation"));
        let parent = sandbox(&f.resolved_profiles, SandboxStatus::Active);
        let workspace = workspace(&f.resolved_profiles);
        let origin = AgentOrigin::Trigger {
            trigger_id: created.launch.trigger_id.to_string(),
            launch_id: created.launch.id.to_string(),
        };
        let executor = ResourceActor::Trigger {
            subject: f.caller.subject.clone(),
            launch_id: created.launch.id.clone(),
        };
        let request = AgentAdmission {
            created_by: Some(executor.clone()),
            owner: f.caller.subject.clone(),
            workspace_id: workspace.id.clone(),
            agent_id: AgentId::generate(),
            worktree_id: WorktreeId::generate(),
            profile: AgentProfileSnapshot::from(&f.resolved_profiles.agent),
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            title: None,
            first_message: None,
            creation_key: format!(
                "trigger:{}",
                crate::profiles::content_digest(&origin).unwrap()
            ),
            origin,
            event_at: crate::now_rfc3339(),
        };
        let creation = request.materialize(&workspace).unwrap();
        let provenance = AgentCreationProvenance::for_trigger_admission(
            &request,
            initiation,
            &creation.agent,
            &parent,
            ResourceOwner::Person {
                subject: f.caller.subject.clone(),
            },
        )
        .unwrap();
        assert_eq!(provenance.created_by, Some(executor));
        assert!(
            matches!(&provenance.creation_origin, AgentCreationOrigin::Trigger { launching_actor: Some(actor), .. } if actor == &identity.actor)
        );
        let mut wrong_agent = creation.agent.clone();
        wrong_agent.origin = AgentOrigin::Trigger {
            trigger_id: "trg_other".into(),
            launch_id: created.launch.id.to_string(),
        };
        assert!(AgentCreationProvenance::for_trigger_admission(
            &request,
            initiation,
            &wrong_agent,
            &parent,
            ResourceOwner::Person {
                subject: f.caller.subject.clone(),
            },
        )
        .is_err());
        let mut forged = request.clone();
        forged.origin = AgentOrigin::Manual;
        assert!(initiation.require_agent_admission(&forged).is_err());
        forged = request;
        forged.created_by = Some(identity.actor);
        assert!(initiation.require_agent_admission(&forged).is_err());
    }
}

#[tokio::test]
async fn legacy_launch_keeps_unknown_initiator_and_invalid_principal_never_admits() {
    let f = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let legacy = f
        .service
        .create_or_resume(&f.caller, &f.public.public_id, f.request("legacy"))
        .await
        .unwrap();
    assert!(legacy.launch.initiation.is_none());
    let replay = f
        .service
        .create_or_resume_as(&context(&f, true), &f.public.public_id, f.request("legacy"))
        .await
        .unwrap();
    assert_eq!(replay.launch, legacy.launch);
    let mut forged = context(&f, false);
    forged.actor = ResourceActor::Human {
        subject: SubjectId::new("foreign"),
    };
    assert_eq!(
        f.service
            .create_or_resume_as(&forged, &f.public.public_id, f.request("forged"))
            .await,
        Err(LaunchServiceError::Forbidden)
    );
    assert_eq!(f.stats().await.launches_total, 1);
}

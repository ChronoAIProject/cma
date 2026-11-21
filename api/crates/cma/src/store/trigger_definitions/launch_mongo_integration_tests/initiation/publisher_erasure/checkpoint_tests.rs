//! Publisher erasure must preserve already committed Agent effects.
use super::*;

#[derive(Clone, Copy)]
enum AdmissionCase {
    Created,
    CreatedWithProfileKey,
    SubstitutedOrigin,
    Reused,
    CorruptReuseReceipt,
}

pub(super) async fn admission_with_message(f: &Fixture, launch: &Launch) -> AgentAdmission {
    let mut request = f.admission(launch).await;
    request.first_message = Some(
        FirstMessageRecord::new(
            request.creation_key.clone(),
            f.public.revision.content.first_message.clone(),
            request.event_at.clone(),
        )
        .unwrap(),
    );
    request
}

async fn committed_agent_checkpoint(state: LaunchEffectState, case: AdmissionCase) {
    let f = Arc::new(continuity_fixture().await);
    let reused = matches!(
        case,
        AdmissionCase::Reused | AdmissionCase::CorruptReuseReceipt
    );
    let reuse_requested = reused || matches!(case, AdmissionCase::CreatedWithProfileKey);
    if reused {
        let prior = f
            .ready_for_agent(f.launch("previous-admitted-agent").await)
            .await;
        f.resources
            .admit_agent_with_trigger_initiator(
                admission_with_message(&f, &prior).await,
                prior.initiation.clone().unwrap(),
                false,
            )
            .await
            .unwrap();
    }
    let mut launch = f
        .ready_for_agent(f.launch("committed-agent-publisher-erasure").await)
        .await;
    let mut request = admission_with_message(&f, &launch).await;
    if reuse_requested {
        request.creation_key = request
            .creation_key
            .replacen("trigger:", "trigger-profile:", 1);
        request.first_message = Some(
            FirstMessageRecord::new(
                request.creation_key.clone(),
                f.public.revision.content.first_message.clone(),
                request.event_at.clone(),
            )
            .unwrap(),
        );
    }
    let creation_key = request.creation_key.clone();
    let admitted = f
        .resources
        .admit_agent_with_trigger_initiator(
            request,
            launch.initiation.clone().unwrap(),
            reuse_requested,
        )
        .await
        .unwrap();
    let mut agent = admitted.creation().agent.clone();
    let db = f.seed.client.database(&f.seed.database);
    let count = db
        .collection::<Document>("agents")
        .count_documents(doc! {})
        .await
        .unwrap();
    if matches!(case, AdmissionCase::SubstitutedOrigin) {
        agent.origin = AgentOrigin::Trigger {
            trigger_id: launch.trigger_id.to_string(),
            launch_id: layer2_core::LaunchId::generate().to_string(),
        };
        agent.creation_key = format!(
            "trigger:{}",
            layer2_core::profiles::content_digest(&agent.origin).unwrap()
        );
        agent.revision += 1;
        db.collection::<Document>("agents")
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
    }
    if matches!(case, AdmissionCase::CorruptReuseReceipt) {
        let receipt_id =
            layer2_core::profiles::content_digest(&(launch.owner.as_str(), creation_key.as_str()))
                .unwrap();
        assert_eq!(
            db.collection::<Document>("agent_reuse_bindings")
                .update_one(
                    doc! {"_id":receipt_id},
                    doc! {"$set":{"creation_key":"substituted-key"}}
                )
                .await
                .unwrap()
                .modified_count,
            1
        );
    }
    let expected_revision = launch.revision;
    let fence = launch.work.lease.as_ref().unwrap().fence;
    let at = layer2_core::now_rfc3339();
    let events = match state {
        LaunchEffectState::Applied => {
            launch
                .mark_worker_effect_applied(
                    "initiator-worker",
                    fence,
                    agent.id.to_string(),
                    at.clone(),
                )
                .unwrap();
            vec![]
        }
        LaunchEffectState::RecoveryRequired => launch
            .retry_worker_launch(
                "initiator-worker",
                fence,
                "dependency_unavailable".into(),
                "The Agent result checkpoint will be retried".into(),
                layer2_core::rfc3339_at(
                    std::time::SystemTime::now() + std::time::Duration::from_secs(1),
                ),
                at.clone(),
            )
            .unwrap(),
        LaunchEffectState::Executing => vec![],
        LaunchEffectState::Prepared => unreachable!("the Agent is already committed"),
    };
    if state != LaunchEffectState::Executing {
        launch = f
            .seed
            .store
            .commit_launch(LaunchCommit {
                expected_revision,
                launch,
                event_at: at,
                events,
            })
            .await
            .unwrap();
    }
    assert!(launch.resolution.agent.is_pending());
    assert_eq!(launch.work.effect.as_ref().unwrap().state, state);
    erase_publisher(&f).await;
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        Arc::new(Driver(f.clone())),
        LaunchWorkerConfig::new("committed-agent-erasure-worker"),
    );
    assert!(worker
        .run_one(
            &launch.id,
            std::time::SystemTime::now() + std::time::Duration::from_secs(61),
        )
        .await
        .unwrap());
    let settled = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    if matches!(
        case,
        AdmissionCase::SubstitutedOrigin | AdmissionCase::CorruptReuseReceipt
    ) {
        assert_eq!(settled.state, LaunchState::FailedTerminal);
        assert!(settled.resolution.agent.is_pending());
    } else {
        assert_eq!(settled.work.step, LaunchWorkStep::WaitForAgent);
        assert_eq!(settled.resolution.agent.id(), Some(&agent.id));
        assert_eq!(
            matches!(settled.resolution.agent, ResolvedResource::Reused(_)),
            reused
        );
        assert!(settled.failure.is_none());
    }
    assert_eq!(
        db.collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        count
    );
    assert_eq!(f.resources.get_agent(&agent.id).await.unwrap(), Some(agent));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_recovers_applied_agent_effect_before_resolution_checkpoint() {
    committed_agent_checkpoint(LaunchEffectState::Applied, AdmissionCase::Created).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_recovers_committed_agent_with_executing_effect() {
    committed_agent_checkpoint(LaunchEffectState::Executing, AdmissionCase::Created).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_recovers_committed_agent_with_recovery_required_effect() {
    committed_agent_checkpoint(LaunchEffectState::RecoveryRequired, AdmissionCase::Created).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_refuses_agent_effect_with_substituted_launch_origin() {
    committed_agent_checkpoint(LaunchEffectState::Applied, AdmissionCase::SubstitutedOrigin).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_recovers_exact_committed_agent_reuse_receipt() {
    committed_agent_checkpoint(LaunchEffectState::RecoveryRequired, AdmissionCase::Reused).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_recovers_new_agent_created_under_resume_existing_key() {
    committed_agent_checkpoint(
        LaunchEffectState::RecoveryRequired,
        AdmissionCase::CreatedWithProfileKey,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_refuses_substituted_committed_agent_reuse_receipt() {
    committed_agent_checkpoint(
        LaunchEffectState::RecoveryRequired,
        AdmissionCase::CorruptReuseReceipt,
    )
    .await;
}

//! Actual service admission from a full, certified format-2 segment.
use super::*;
use crate::store::agent_capacity_recovery::{records, RecoveryService};
use crate::store::agent_log_segments::{publication, writer_admission, writer_gate};
use layer2_core::sandbox::agent_capacity_recovery::{
    ports::{CommitOutcome, RecoveryError, RecoveryLookup},
    service::CapacityRecoveryFeatures,
    state::{RecoveryPhase, RecoveryRequest, RecoveryWork},
};
use layer2_core::sandbox::store::AgentLogPublicationIdentity;

mod faults;
mod support;
use support::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn segmented_capacity_admission_publishes_successor_and_preserves_witness() {
    let (client, name) = database("capacity_segmented_admit").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, caller, request) = exhausted(&store).await;
    let before = snapshot(&store, &request).await;
    let CommitOutcome::Recorded(work) = service(&store, true)
        .admit(&caller, &request)
        .await
        .unwrap()
    else {
        panic!("segmented capacity admission must commit")
    };
    let after = snapshot(&store, &request).await;
    assert_admitted(&before, &after, &work);
    assert_eq!(work.phase, RecoveryPhase::Admitted);
    assert_eq!(work.request, request);
    assert_eq!(work.attempts, 0);
    assert!(work.lease.is_none());
    let admission = read_admission(&store, &request).await;
    assert_eq!(
        admission.kind,
        writer_admission::AdmissionKind::CapacityRecovery
    );
    assert_eq!(admission.writer_gate.name, writer_gate::GateName::Segments);
    assert_eq!(
        admission.recovery_gate.unwrap().name,
        writer_gate::GateName::CapacityRecovery
    );
    assert_eq!(admission.expected_seq, 10_000);
    assert_eq!(admission.appended_at, request.requested_at);
    assert_eq!(
        admission.fingerprint.bytes,
        work.admission_identity().unwrap().fingerprint
    );

    // Retrying an already committed admission after closure returns the exact
    // original result. Neither rollout closure nor process restart republishes.
    close_gates(&store).await;
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        service(&reopened, false)
            .admit(&caller, &request)
            .await
            .unwrap(),
        CommitOutcome::Recorded(work.clone())
    );
    assert_eq!(snapshot(&reopened, &request).await, after);

    // A later sequence-only append must not replace the result witness with a
    // digest of today's Agent. The old admission still returns its original work.
    store
        .append_agent_log_keyed(
            &agent.id,
            &AgentLogPublicationIdentity {
                key: "after-storage-recovery".into(),
                fingerprint: [23; 32],
                appended_at: layer2_core::now_rfc3339(),
            },
            AgentLogKind::ConversationGap { frames: 1 },
        )
        .await
        .unwrap();
    let progressed = snapshot(&store, &request).await;
    assert_eq!(progressed.carrier, after.carrier);
    assert_eq!(progressed.publication, after.publication);
    assert_eq!(progressed.agent.revision, after.agent.revision);
    assert_eq!(
        progressed.agent.conversation.last_seq,
        after.agent.conversation.last_seq + 1
    );
    assert_eq!(
        service(&reopened, false)
            .admit(&caller, &request)
            .await
            .unwrap(),
        CommitOutcome::Recorded(work)
    );
    let mut foreign = request.clone();
    foreign.requested_at = NOW.into();
    assert_eq!(foreign.key().unwrap(), request.key().unwrap());
    assert_eq!(
        service(&reopened, true).admit(&caller, &foreign).await,
        Err(RecoveryError::Conflict)
    );
    assert_eq!(snapshot(&store, &request).await, progressed);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn segmented_capacity_admission_requires_both_enabled_matching_gates() {
    let (client, name) = database("capacity_segmented_gates").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, caller, request) = exhausted(&store).await;
    let before = snapshot(&store, &request).await;
    assert_eq!(
        service(&store, false).admit(&caller, &request).await,
        Err(RecoveryError::Disabled)
    );
    assert_eq!(snapshot(&store, &request).await, before);
    for gate in [
        writer_gate::GateName::Segments,
        writer_gate::GateName::CapacityRecovery,
    ] {
        for policy in [
            writer_gate::Policy::default(),
            writer_gate::Policy {
                enabled: true,
                agents: vec!["other-agent".into()],
                ..Default::default()
            },
        ] {
            set_gate(&store, gate, policy).await;
            assert_eq!(
                service(&store, true).admit(&caller, &request).await,
                Err(RecoveryError::Unavailable)
            );
            assert_eq!(snapshot(&store, &request).await, before);
            assert!(store
                .client
                .database(&name)
                .collection::<Document>(writer_admission::COLLECTION)
                .find_one(doc! {"publication_key":request.key().unwrap()})
                .await
                .unwrap()
                .is_none());
        }
        set_gate(
            &store,
            gate,
            writer_gate::Policy {
                enabled: true,
                agents: vec![agent.id.to_string()],
                ..Default::default()
            },
        )
        .await;
    }
    // Restoration proves the refusals were caused by policy, not a bad fixture.
    let CommitOutcome::Recorded(work) = service(&store, true)
        .admit(&caller, &request)
        .await
        .unwrap()
    else {
        panic!("restored gates must allow the original request")
    };
    assert_admitted(&before, &snapshot(&store, &request).await, &work);
    client.database(&name).drop().await.unwrap();
}

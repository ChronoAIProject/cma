use layer2_core::sandbox::store::{SandboxCheckpointAgentGuard, SandboxCheckpointMutation};
use layer2_core::sandbox::{SandboxCheckpointConversation, SandboxCheckpointRef};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn active_checkpoint_publication_preserves_the_exact_writer_resume_debt() {
    let (client, name) = database("active_checkpoint_publication").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, _, agent) = capture_fixture(&store, "publication").await;
    let request = begin(&parent);
    store
        .commit_sandbox_lifecycle(request.clone())
        .await
        .unwrap();
    let mut current = request.sandbox;
    let mut claimed = current.clone();
    claimed
        .claim_checkpoint_capture("capture".into(), NOW.into(), MINUTE.into())
        .unwrap();
    advance_capture(&store, &mut current, claimed).await;
    let debt = current
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .clone();
    let archive = b"checkpoint conversation archive".to_vec();
    let conversation = SandboxCheckpointConversation::new(
        agent.id.clone(),
        agent.conversation.conversation_id.as_deref().unwrap(),
        ConversationSnapshot {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            archive,
        },
    )
    .unwrap();
    let checkpoint = SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch: debt.checkpoint_epoch,
            object_reference: "checkpoints/capture/publication".into(),
            sha256: "b".repeat(64),
            writers: WriterState::Held,
        },
        &[conversation],
        NOW,
    )
    .unwrap();
    let mut next = current.clone();
    next.publish_captured_checkpoint(
        checkpoint.clone(),
        "capture",
        debt.work.lease.as_ref().unwrap().fence,
        NOW.into(),
    )
    .unwrap();
    store
        .publish_sandbox_checkpoint(SandboxCheckpointMutation {
            guard: SandboxMutationGuard::capture(&current),
            agents: vec![SandboxCheckpointAgentGuard::capture(&agent)],
            nonblocking: false,
            checkpoint: checkpoint.clone(),
            sandbox: next.clone(),
            appended_at: NOW.into(),
        })
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_sandbox(&parent.id).await.unwrap(),
        Some(next.clone())
    );
    assert_eq!(next.lifecycle.checkpoints.latest, Some(checkpoint));
    let release = next.lifecycle.checkpoint_capture.as_ref().unwrap();
    assert_eq!(release.authority, debt.authority);
    assert_eq!(
        release.phase,
        layer2_core::sandbox::SandboxCheckpointCapturePhase::ResumeWriters
    );
    assert_eq!(reopened.get_agent(&agent.id).await.unwrap(), Some(agent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn retirement_transfers_held_lifecycle_writer_authority_without_unblocking_claims() {
    let (client, name) = database("capture_retirement_transfer").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for transactional in [false, true] {
        let (parent, workspace, agent) =
            capture_fixture(&store, &format!("retire_{transactional}")).await;
        let mut pausing = parent.clone();
        pausing.begin_pause(NOW.into(), None).unwrap();
        store
            .save_sandbox(parent.revision, pausing.clone())
            .await
            .unwrap();
        let authority = pausing
            .lifecycle
            .operation
            .as_ref()
            .unwrap()
            .authority()
            .clone();
        let mut retiring = pausing.clone();
        retiring.begin_retirement(NOW.into()).unwrap();
        if transactional {
            store
                .commit(SandboxTransaction::SaveSandboxAndAppend {
                    command_revival: None,
                    released_runtime: None,
                    expected_revision: pausing.revision,
                    sandbox: Box::new(retiring.clone()),
                    event_at: NOW.into(),
                    event: SandboxEventKind::StatusChanged {
                        from: SandboxStatus::Pausing,
                        to: SandboxStatus::Retiring,
                    },
                })
                .await
                .unwrap();
        } else {
            store
                .save_sandbox(pausing.revision, retiring.clone())
                .await
                .unwrap();
        }
        assert_eq!(
            store.get_sandbox(&parent.id).await.unwrap(),
            Some(retiring.clone())
        );
        assert_eq!(
            retiring
                .lifecycle
                .checkpoint_capture
                .as_ref()
                .unwrap()
                .authority,
            authority
        );
        assert_no_writer_claims(&store, &retiring, &workspace, &agent).await;
    }
    client.database(&name).drop().await.unwrap();
}

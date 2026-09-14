use super::*;
use layer2_core::sandbox::SandboxCheckpointCapturePhase;

const FIVE: &str = "2026-09-05T00:05:00Z";
const SIX: &str = "2026-09-05T00:06:00Z";
const SEVEN: &str = "2026-09-05T00:07:00Z";
const TEN: &str = "2026-09-05T00:10:00Z";
const TWELVE: &str = "2026-09-05T00:12:00Z";

async fn claimed_capture(store: &MongoSandboxStore, suffix: &str) -> (Sandbox, Workspace) {
    let (parent, workspace, _) = capture_fixture(store, suffix).await;
    let request = begin(&parent);
    store
        .commit_sandbox_lifecycle(request.clone())
        .await
        .unwrap();
    let mut claimed = request.sandbox.clone();
    claimed
        .claim_checkpoint_capture("capture".into(), NOW.into(), FIVE.into())
        .unwrap();
    store
        .commit_sandbox_lifecycle(mutation(&request.sandbox, claimed.clone(), NOW))
        .await
        .unwrap();
    (claimed, workspace)
}

fn fence(sandbox: &Sandbox) -> u64 {
    sandbox
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .work
        .lease
        .as_ref()
        .unwrap()
        .fence
}

fn renewal(sandbox: &Sandbox, at: &str, until: &str) -> SandboxLifecycleMutation {
    let mut next = sandbox.clone();
    next.renew_checkpoint_capture("capture", fence(sandbox), at.into(), until.into())
        .unwrap();
    mutation(sandbox, next, at)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_capture_renewal_is_exact_and_replay_cannot_append_another_event() {
    let (client, name) = database("capture_renewal_exact").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (claimed, _) = claimed_capture(&store, "renewal_exact").await;
    let request = renewal(&claimed, MINUTE, SIX);
    for kind in 0..4 {
        let mut invalid = request.clone();
        let capture = invalid
            .sandbox
            .lifecycle
            .checkpoint_capture
            .as_mut()
            .unwrap();
        match kind {
            0 => capture.phase = SandboxCheckpointCapturePhase::ResumeWriters,
            1 => capture.authority.writer_resume_generation += 1,
            2 => capture.work.attempts += 1,
            _ => capture.runtime.runtime_id.push_str("-other"),
        }
        assert!(store.commit_sandbox_lifecycle(invalid).await.is_err());
        assert_eq!(
            store.get_sandbox(&claimed.id).await.unwrap(),
            Some(claimed.clone())
        );
    }
    store
        .commit_sandbox_lifecycle(request.clone())
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_sandbox(&claimed.id).await.unwrap(),
        Some(request.sandbox.clone())
    );
    assert_eq!(
        reopened
            .sandbox_events(&claimed.id, 0, 10)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        reopened.commit_sandbox_lifecycle(request.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        reopened
            .commit_sandbox_lifecycle(renewal(&claimed, TWO_MINUTES, SEVEN))
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        reopened.get_sandbox(&claimed.id).await.unwrap(),
        Some(request.sandbox.clone())
    );
    assert_eq!(
        reopened
            .sandbox_events(&claimed.id, 0, 10)
            .await
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        request
            .sandbox
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .authority,
        claimed
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .authority
    );
    assert_eq!(fence(&request.sandbox), fence(&claimed));
    assert_eq!(request.sandbox.runtime, claimed.runtime);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_capture_renewal_and_takeover_have_one_winner_and_preserve_resume_debt() {
    let (client, name) = database("capture_renewal_takeover").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for index in 0..8 {
        let (claimed, workspace) =
            claimed_capture(&store, &format!("renewal_takeover_{index}")).await;
        let request = renewal(&claimed, MINUTE, SIX);
        let mut successor = claimed.clone();
        successor
            .claim_checkpoint_capture("successor".into(), FIVE.into(), TEN.into())
            .unwrap();
        let takeover = mutation(&claimed, successor.clone(), FIVE);
        let (renewed, taken) = tokio::join!(
            store.commit_sandbox_lifecycle(request.clone()),
            store.commit_sandbox_lifecycle(takeover),
        );
        let current = match (renewed, taken) {
            (Ok(()), Err(SandboxStoreError::Conflict)) => request.sandbox,
            (Err(SandboxStoreError::Conflict), Ok(())) => successor,
            other => panic!("renewal and takeover must have one winner: {other:?}"),
        };
        assert_eq!(
            store.get_sandbox(&claimed.id).await.unwrap(),
            Some(current.clone())
        );
        let old_authority = &claimed
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .authority;
        assert_eq!(
            &current
                .lifecycle
                .checkpoint_capture
                .as_ref()
                .unwrap()
                .authority,
            old_authority
        );
        let lease = current
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .work
            .lease
            .as_ref()
            .unwrap();
        let mut debt = current.clone();
        debt.require_checkpoint_writer_resume(&lease.holder, lease.fence, SEVEN.into())
            .unwrap();
        store
            .commit_sandbox_lifecycle(mutation(&current, debt.clone(), SEVEN))
            .await
            .unwrap();
        let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
        assert_eq!(
            reopened
                .commit_sandbox_lifecycle(renewal(&claimed, TWO_MINUTES, SEVEN))
                .await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            reopened.get_sandbox(&claimed.id).await.unwrap(),
            Some(debt.clone())
        );
        let capture = debt.lifecycle.checkpoint_capture.as_ref().unwrap();
        assert_eq!(&capture.authority, old_authority);
        assert_eq!(capture.phase, SandboxCheckpointCapturePhase::ResumeWriters);
        assert_eq!(
            reopened
                .admit_agent(agent_admission(&workspace, "renewal-new", "renewal_new"))
                .await,
            Err(SandboxStoreError::SandboxNotReady)
        );
        let mut resuming = debt.clone();
        resuming
            .claim_checkpoint_capture("resume".into(), SEVEN.into(), TWELVE.into())
            .unwrap();
        reopened
            .commit_sandbox_lifecycle(mutation(&debt, resuming.clone(), SEVEN))
            .await
            .unwrap();
        let mut done = resuming.clone();
        done.complete_checkpoint_writer_resume("resume", fence(&resuming), SEVEN.into())
            .unwrap();
        reopened
            .commit_sandbox_lifecycle(mutation(&resuming, done.clone(), SEVEN))
            .await
            .unwrap();
        assert!(reopened
            .get_sandbox(&claimed.id)
            .await
            .unwrap()
            .unwrap()
            .lifecycle
            .checkpoint_capture
            .is_none());
        assert!(done.accepts_agent_and_workspace_admission());
        assert_eq!(
            reopened
                .sandbox_events(&claimed.id, 0, 10)
                .await
                .unwrap()
                .len(),
            6
        );
    }
    client.database(&name).drop().await.unwrap();
}

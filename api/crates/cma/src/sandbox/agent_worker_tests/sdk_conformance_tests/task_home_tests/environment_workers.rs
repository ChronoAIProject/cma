use super::repository_connection_tests::RepositoryRefusal;
use super::*;
use crate::sandbox::worker::SandboxWorker;

pub(super) async fn drive_environment(
    harness: &Harness<MongoSandboxStore>,
    agent_id: &AgentId,
    public_sandbox_id: &SandboxId,
    worker: &SandboxWorker,
    retry: &super::store_retry::StoreRetry,
    refusal: Option<RepositoryRefusal>,
    phase: &mut String,
) {
    let agent = harness.stored(agent_id).await;
    assert_public_sandbox(public_sandbox_id, &agent.sandbox_id);
    let scenario = refusal.map_or("successful preparation", |refusal| refusal.code);
    // The Sandbox provisioner drives its Workspace inline. A second driver
    // can claim the same child and delay the parent behind its lease.
    loop {
        let sandbox = harness
            .store
            .get_sandbox(public_sandbox_id)
            .await
            .unwrap()
            .unwrap();
        if matches!(
            sandbox.status,
            SandboxStatus::Active | SandboxStatus::Failed | SandboxStatus::Retiring
        ) {
            let expected = match refusal {
                Some(RepositoryRefusal { sibling: true, .. }) => SandboxStatus::Failed,
                Some(_) => SandboxStatus::Retiring,
                None => SandboxStatus::Active,
            };
            assert_eq!(
                sandbox.status, expected,
                "{scenario}: Sandbox terminal status"
            );
            break;
        }
        let lease = sandbox
            .work
            .lease
            .as_ref()
            .map(|lease| lease.expires_at.as_str());
        *phase = format!(
            "Sandbox {:?}, due {}, lease {lease:?}",
            sandbox.work.step, sandbox.work.ready_at
        );
        if let Some(refusal) = refusal.filter(|_| {
            matches!(
                sandbox.work.step,
                SandboxWorkStep::CreateProfileWorkspace(_)
            )
        }) {
            // Preserve the typed refusal if a protected commit needs replay.
            harness.backend.fail_next(
                WireOperation::PrepareWorkspace,
                refusal.code,
                "External failure detail must not leak",
            );
        }
        wait_for_due_work(&sandbox.work.ready_at, lease).await;
        let step = retry.sandbox(harness, worker, public_sandbox_id).await;
        assert!(
            !matches!(step, Some(false)),
            "{scenario}: {phase} did not progress"
        );
    }
    *phase = String::from("checking committed Workspace result");
    let workspace = harness
        .store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let expected = if refusal.is_some() {
        WorkspaceStatus::Failed
    } else {
        WorkspaceStatus::Ready
    };
    assert_eq!(
        workspace.status, expected,
        "{scenario}: Workspace terminal status"
    );
    assert_eq!(
        workspace
            .failed
            .as_ref()
            .map(|failure| failure.class.as_str()),
        refusal.map(|refusal| refusal.code),
    );
}

fn assert_public_sandbox(public: &SandboxId, stored: &SandboxId) {
    assert_eq!(public, stored, "public Agent reported a different Sandbox");
}

pub(super) async fn drive_source_failure(
    harness: &Harness<MongoSandboxStore>,
    agent_id: &AgentId,
    public_sandbox_id: &SandboxId,
    worker: &SandboxWorker,
    retry: &super::store_retry::StoreRetry,
    refusal: RepositoryRefusal,
    deadline: tokio::time::Instant,
) -> Agent {
    let mut phase = String::from("reading admitted Sandbox");
    if refusal.sibling {
        super::repository_connection_tests::retain_another_workspace(harness, agent_id).await;
    }
    let result = tokio::time::timeout_at(deadline, async {
        drive_environment(
            harness,
            agent_id,
            public_sandbox_id,
            worker,
            retry,
            Some(refusal),
            &mut phase,
        )
        .await;

        // The durable Sandbox teardown owns subsequent Agent cleanup.
        harness.stored(agent_id).await
    })
    .await;
    result.unwrap_or_else(|_| panic!("source case {}: timed out at {phase}", refusal.code))
}

async fn wait_for_due_work(ready_at: &str, lease_expires_at: Option<&str>) {
    // Protected Mongo claims use server time. Allow a recorded retry or lease to
    // become due without guessing the delay or supplying a future claim time.
    let stamp = lease_expires_at.map_or(ready_at, |lease| ready_at.max(lease));
    let due = SystemTime::UNIX_EPOCH
        + Duration::from_secs(
            layer2_core::seconds_from_rfc3339(stamp).expect("valid work timestamp"),
        );
    if let Ok(delay) = due.duration_since(SystemTime::now()) {
        tokio::time::sleep(delay).await;
    }
}

#[test]
fn public_sandbox_identity_accepts_the_admitted_sandbox() {
    let admitted = SandboxId("admitted".into());
    assert_public_sandbox(&admitted, &admitted);
}

#[test]
#[should_panic(expected = "public Agent reported a different Sandbox")]
fn public_sandbox_identity_rejects_a_different_sandbox() {
    assert_public_sandbox(
        &SandboxId("different".into()),
        &SandboxId("admitted".into()),
    );
}

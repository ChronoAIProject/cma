use super::*;
use crate::sandbox::worker::SandboxWorker;

pub(super) async fn drive_environment(
    harness: &Harness<MongoSandboxStore>,
    agent_id: &AgentId,
    public_sandbox_id: &SandboxId,
    worker: &SandboxWorker,
    retry: &super::store_retry::StoreRetry,
    refusal: Option<&str>,
    phase: &mut String,
) {
    let agent = harness.stored(agent_id).await;
    assert_public_sandbox(public_sandbox_id, &agent.sandbox_id);
    let scenario = refusal.unwrap_or("successful preparation");
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
            SandboxStatus::Active | SandboxStatus::Failed
        ) {
            let expected = if refusal.is_some() {
                SandboxStatus::Failed
            } else {
                SandboxStatus::Active
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
        if let Some(code) = refusal.filter(|_| {
            matches!(
                sandbox.work.step,
                SandboxWorkStep::CreateProfileWorkspace(_)
            )
        }) {
            // Preserve the typed refusal if a protected commit needs replay.
            harness.backend.fail_next(
                WireOperation::PrepareWorkspace,
                code,
                "External failure detail must not leak",
            );
        }
        wait_for_due_work(&sandbox.work.ready_at, lease).await;
        let step = worker.run_one(public_sandbox_id, SystemTime::now()).await;
        assert!(
            !matches!(step, Ok(false)),
            "{scenario}: {phase} did not progress"
        );
        retry.should_retry(step);
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
        refusal,
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
    code: &str,
    deadline: tokio::time::Instant,
) -> Agent {
    let mut phase = String::from("reading admitted Sandbox");
    let result = tokio::time::timeout_at(deadline, async {
        drive_environment(
            harness,
            agent_id,
            public_sandbox_id,
            worker,
            retry,
            Some(code),
            &mut phase,
        )
        .await;

        // Start Agent work after the committed failure, so it cannot spend its
        // readiness observation budget waiting for the fixture's parent work.
        loop {
            let agent = harness.stored(agent_id).await;
            if agent.status == AgentStatus::Failed {
                return agent;
            }
            let lease = agent
                .work
                .lease
                .as_ref()
                .map(|lease| lease.expires_at.as_str());
            phase = format!(
                "Agent {:?}, due {}, lease {lease:?}",
                agent.work.step, agent.work.ready_at
            );
            wait_for_due_work(&agent.work.ready_at, lease).await;
            let step = harness.worker.run_one(agent_id, SystemTime::now()).await;
            assert!(
                !matches!(step, Ok(false)),
                "source case {code}: {phase} did not progress"
            );
            retry.should_retry(step);
        }
    })
    .await;
    result.unwrap_or_else(|_| panic!("source case {code}: timed out at {phase}"))
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

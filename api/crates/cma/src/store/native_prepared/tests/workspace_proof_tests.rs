use super::super::{workspace_proof, NativePreparedWorkspaceProof};
use super::*;

fn prepared_record() -> Record {
    let plan = plan();
    ready(Record {
        payload: WarmSandbox::new(plan.warm.clone(), NOW.into()).unwrap(),
        plan,
        ownership: None,
    })
}

fn proof(record: &Record) -> NativePreparedWorkspaceProof {
    let (prepare, setup) = workspace_proof::requests(&record.payload).unwrap();
    let mut partial = record.payload.clone();
    partial.workspaces[0].setup_ready = false;
    let proof = NativePreparedWorkspaceProof::prepared(
        &partial,
        layer3_adapter::WorkspaceReceipt {
            operation_id: prepare.operation_id,
            request_sha256: prepare.request_sha256,
            workspace_id: prepare.workspace_id,
            name: prepare.name,
            path: "/home/cma/workspace".into(),
            resolved_commit: partial.workspaces[0].resolved_commit.clone().unwrap(),
        },
    )
    .unwrap();
    proof
        .completed(
            &record.payload,
            layer3_adapter::SetupReceipt {
                operation_id: setup.operation_id,
                request_sha256: setup.request_sha256,
                workspace_id: setup.workspace_id,
                commands: vec![],
            },
        )
        .unwrap()
}

#[test]
fn workspace_completion_envelope_is_optional_and_old_reader_compatible() {
    let record = prepared_record();
    let old = document(&record).unwrap();
    assert!(workspace_proof::read(&old, &record.payload)
        .unwrap()
        .is_none());
    let proof = proof(&record);
    let value = workspace_proof::document(&record, Some(&proof)).unwrap();
    let old_reader: Record = super::super::super::sandbox_mongo::read_record(&value).unwrap();
    assert_eq!(old_reader, record);
    assert_eq!(
        workspace_proof::read(&value, &record.payload).unwrap(),
        Some(proof)
    );
    // An old replace writer drops the optional field; it never synthesizes a receipt.
    assert!(
        workspace_proof::read(&document(&old_reader).unwrap(), &record.payload)
            .unwrap()
            .is_none()
    );
}

#[test]
fn workspace_completion_rejects_each_changed_receipt_and_contract_identity() {
    let record = prepared_record();
    let proof = proof(&record);
    for path in [
        "contract_version",
        "plan_sha256",
        "runtime.boot_id",
        "runtime.runtime_id",
        "runtime.runtime_version",
        "prepare.operation_id",
        "prepare.request_sha256",
        "prepare.workspace_id",
        "prepare.name",
        "prepare.path",
        "prepare.resolved_commit",
        "setup.operation_id",
        "setup.request_sha256",
        "setup.workspace_id",
    ] {
        let mut value = serde_json::to_value(&proof).unwrap();
        let parts: Vec<_> = path.split('.').collect();
        let mut field = &mut value;
        for part in parts {
            field = &mut field[part];
        }
        *field = if path == "contract_version" {
            serde_json::json!(2)
        } else {
            serde_json::json!("changed")
        };
        let changed: NativePreparedWorkspaceProof = serde_json::from_value(value).unwrap();
        assert!(changed.validate(&record.payload).is_err(), "{path}");
    }
    for case in 0..5 {
        let mut changed = record.payload.clone();
        match case {
            0 => changed.plan.execution.clone_depth += 1,
            1 => changed.plan.execution.command_timeout_seconds += 1,
            2 => {
                changed
                    .home
                    .as_mut()
                    .unwrap()
                    .receipt
                    .as_mut()
                    .unwrap()
                    .storage_generation = "different-pvc".into()
            }
            3 => changed.workspaces[0].resolved_commit = Some("b".repeat(40)),
            _ => changed.workspaces[0].setup_ready = false,
        }
        assert!(proof.validate(&changed).is_err(), "case {case}");
    }
}

fn owned(record: &mut Record) -> (Sandbox, Workspace) {
    let (sandbox_profile, workspace_profile) = actual_profiles(&record.plan);
    let mut parent = crate::store::agent_access_fixtures::sandbox("workspace-proof");
    parent.id = record.payload.id.clone();
    parent.origin = SandboxOrigin::Lifecycle {
        operation_id: "workspace-proof-operation".into(),
    };
    parent.profile = sandbox_profile.clone();
    parent.runtime = record.payload.runtime.clone();
    parent.lifecycle.home = record.payload.home.clone();
    parent.status = SandboxStatus::Installing;
    parent.credential.delivered = true;
    record.payload.state = WarmSandboxState::Claimed;
    record.ownership = Some(super::super::model::Ownership {
        owner: parent.owner.clone(),
        operation_id: "workspace-proof-operation".into(),
        sandbox: sandbox_profile,
        workspace: workspace_profile.clone(),
        agent_content: record.plan.warm.workspaces[0].agents[0].content.clone(),
        agent: None,
    });
    let mut workspace = crate::store::agent_access_fixtures::workspace(&parent, "proof");
    workspace.id = record.payload.workspaces[0].id.clone();
    workspace.profile = workspace_profile;
    workspace.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    workspace.name = "workspace".into();
    workspace.path = "/home/cma/workspace".into();
    workspace.status = WorkspaceStatus::Preparing;
    workspace.source = WorkspaceSource::Empty;
    workspace.resolved_commit = None;
    workspace.ready_at = None;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
    workspace.work.execution = Some(record.payload.plan.execution);
    workspace
        .work
        .claim("worker".into(), NOW, LATER.into())
        .unwrap();
    (parent, workspace)
}

#[test]
fn workspace_completion_requires_exact_owned_admission_and_parent() {
    let mut record = prepared_record();
    let proof = proof(&record);
    let (parent, workspace) = owned(&mut record);
    assert!(
        super::super::adoption::workspace_completion(&record, &proof, &parent, &workspace)
            .unwrap()
            .is_some()
    );
    for case in 0..16 {
        let (mut parent, mut workspace) = (parent.clone(), workspace.clone());
        match case {
            0 => parent.owner = SubjectId("another".into()),
            1 => {
                parent.origin = SandboxOrigin::Lifecycle {
                    operation_id: "another".into(),
                }
            }
            2 => parent.runtime.as_mut().unwrap().boot_id = Some("c".repeat(32)),
            3 => {
                parent
                    .lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .receipt
                    .as_mut()
                    .unwrap()
                    .storage_generation = "replacement-pvc".into()
            }
            4 => parent.credential.delivered = false,
            5 => parent.status = SandboxStatus::Retiring,
            6 => workspace.owner = SubjectId("another".into()),
            7 => workspace.origin = WorkspaceOrigin::Manual,
            8 => workspace.id = layer2_core::WorkspaceId("another".into()),
            9 => workspace.name = "another".into(),
            10 => workspace.path = "/home/cma/another".into(),
            11 => workspace.profile.revision += 1,
            12 => workspace.work.execution.as_mut().unwrap().clone_depth += 1,
            13 => workspace.work.lease = None,
            14 => workspace.work.step = WorkspaceWorkStep::Setup,
            _ => workspace.resolved_commit = Some("b".repeat(40)),
        }
        assert!(
            super::super::adoption::workspace_completion(&record, &proof, &parent, &workspace)
                .unwrap()
                .is_none(),
            "case {case}"
        );
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn workspace_completion_survives_health_and_claim_with_old_record_fallback() {
    let (client, name, store) = database().await;
    let plan = plan();
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(&store).await;
    let mut value = workspace_proof::document(&record, Some(&proof(&record))).unwrap();
    value.insert("lease_until", NOW);
    value.insert("check_after", NOW);
    store
        .native_prepared
        .replace_one(doc! {"_id":record.payload.id.as_str()}, value)
        .await
        .unwrap();
    let claim = store
        .claim_native_prepared_work(NOW, LATER)
        .await
        .unwrap()
        .unwrap();
    assert!(claim.workspace_proof.is_some());
    store
        .finish_native_prepared_work(&claim, claim.record.clone(), NOW, LATER)
        .await
        .unwrap();
    let admission = admission(&store, &plan, "proof-owner", "proof-claim").await;
    let SandboxAdmissionResult::Created(parent) = admit_at(&store, admission, NOW).await.unwrap()
    else {
        panic!("fresh");
    };
    assert_eq!(parent.id, record.payload.id);
    let value = store
        .native_prepared
        .find_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let owned = read(&value).unwrap();
    assert!(workspace_proof::read(&value, &owned.payload)
        .unwrap()
        .is_some());
    let mut parent = parent;
    let previous = parent.revision;
    parent.status = SandboxStatus::Installing;
    parent.credential = crate::store::agent_access_fixtures::sandbox("proof").credential;
    parent.work.step = SandboxWorkStep::CreateProfileWorkspace(0);
    parent.revision += 1;
    store.save_sandbox(previous, parent.clone()).await.unwrap();
    let mut candidate = crate::store::agent_access_fixtures::workspace(&parent, "proof");
    candidate.profile = actual_profiles(&plan).1;
    candidate.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    candidate.name = "workspace".into();
    candidate.path = "/home/cma/workspace".into();
    candidate.source = WorkspaceSource::Empty;
    candidate.status = WorkspaceStatus::Preparing;
    candidate.resolved_commit = None;
    candidate.ready_at = None;
    candidate.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
    let WorkspaceAdmissionResult::Created(workspace) = store
        .admit_workspace(WorkspaceAdmission {
            binding: None,
            workspace: candidate,
            kind: WorkspaceAdmissionKind::SandboxProfile,
            event_at: NOW.into(),
        })
        .await
        .unwrap()
    else {
        panic!("fresh workspace");
    };
    let claimed = store
        .claim_workspace_work(&workspace.id, "proof-worker", NOW.into(), LATER.into())
        .await
        .unwrap()
        .unwrap();
    let completed = store
        .prepared_workspace_completion(&parent, &claimed)
        .await
        .unwrap()
        .unwrap();
    let mut stale = claimed.clone();
    stale.revision += 1;
    assert!(store
        .prepared_workspace_completion(&parent, &stale)
        .await
        .unwrap()
        .is_none());
    store
        .native_prepared
        .update_one(
            doc! {"_id":parent.id.as_str()},
            doc! {"$unset":{"workspace_completion_v1":""}},
        )
        .await
        .unwrap();
    assert!(store
        .prepared_workspace_completion(&parent, &claimed)
        .await
        .unwrap()
        .is_none());
    let expected = claimed.revision;
    let mut next = claimed.clone();
    let fence = next.work.lease.as_ref().unwrap().fence;
    next.work
        .advance("proof-worker", fence, WorkspaceWorkStep::None, NOW.into())
        .unwrap();
    next.record_resolved_commit(completed.prepare.resolved_commit.clone())
        .unwrap();
    next.transition(WorkspaceStatus::Ready, NOW.into(), None)
        .unwrap();
    let transaction = SandboxTransaction::SaveWorkspaceAndAppend {
        expected_revision: expected,
        workspace: Box::new(next),
        event_at: NOW.into(),
        event: SandboxEventKind::WorkspaceReady {
            workspace_id: claimed.id.clone(),
            resolved_commit: completed.prepare.resolved_commit,
        },
    };
    let wake = store.agent_admitted.notified();
    store.commit(transaction.clone()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), wake)
        .await
        .unwrap();
    assert_eq!(
        store.commit(transaction).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .sandbox_events(&parent.id, 0, 100)
            .await
            .unwrap()
            .iter()
            .filter(|event| matches!(event.kind, SandboxEventKind::WorkspaceReady { .. }))
            .count(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn installation_transfer_digest_requires_exact_claimed_owner_and_runtime() {
    let (client, name, store) = database().await;
    let mut plan = plan();
    plan.warm.reaches = vec![ServiceId::new("scope-a"), ServiceId::new("scope-b")];
    store
        .reconcile_native_prepared_target(Some(&plan), 1, NOW)
        .await
        .unwrap();
    let record = publish_ready(&store).await;
    let admission = admission(&store, &plan, "owner-a", "installation-proof").await;
    assert!(store
        .completed_installation_digest(&admission.sandbox)
        .await
        .unwrap()
        .is_none());
    let SandboxAdmissionResult::Created(sandbox) = admit_at(&store, admission, NOW).await.unwrap()
    else {
        panic!("fresh admission")
    };
    assert_eq!(sandbox.id, record.payload.id);
    let request = layer3_adapter::InstallRequest::with_timeouts(
        format!("sandbox-install-v1:{}", sandbox.id),
        sandbox.profile.content.software_install_commands.clone(),
        sandbox.work.execution.unwrap().command_timeout_seconds,
        sandbox.work.execution.unwrap().total_timeout_seconds,
        layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
    );
    assert_eq!(
        store
            .completed_installation_digest(&sandbox)
            .await
            .unwrap()
            .as_deref(),
        Some(request.request_sha256.as_str())
    );
    let mut same_scope = sandbox.clone();
    same_scope.credential.reaches.reverse();
    same_scope
        .credential
        .reaches
        .push(ServiceId::new("scope-a"));
    assert_eq!(
        store
            .completed_installation_digest(&same_scope)
            .await
            .unwrap(),
        Some(request.request_sha256.clone())
    );
    for mismatch in 0..17 {
        let mut foreign = sandbox.clone();
        match mismatch {
            0 => foreign.owner = SubjectId::new("another-owner"),
            1 => {
                foreign.origin = SandboxOrigin::Lifecycle {
                    operation_id: "another-operation".into(),
                }
            }
            2 => foreign.profile.id = layer2_core::SandboxProfileId("another-profile".into()),
            3 => foreign.runtime.as_mut().unwrap().runtime_id = "another-runtime".into(),
            4 => foreign.runtime.as_mut().unwrap().provision_generation = Some(2),
            5 => foreign.runtime.as_mut().unwrap().runtime_version = "another-image".into(),
            6 => foreign.runtime.as_mut().unwrap().boot_id = Some("c".repeat(32)),
            7 => {
                foreign
                    .lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .receipt
                    .as_mut()
                    .unwrap()
                    .storage_generation = "replacement-pvc".into()
            }
            8 => {
                foreign
                    .work
                    .execution
                    .as_mut()
                    .unwrap()
                    .command_timeout_seconds += 1
            }
            9 => foreign
                .credential
                .reaches
                .push(ServiceId::new("another-scope")),
            10 => foreign.credential.key_generation = 1,
            11 => foreign.work.step = SandboxWorkStep::RunInstall,
            12 => foreign.lifecycle.home.as_mut().unwrap().native_pool = None,
            13 => {
                foreign
                    .lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .native_pool
                    .as_mut()
                    .unwrap()
                    .claim
                    .as_mut()
                    .unwrap()
                    .uid = "replacement-claim".into()
            }
            14 => foreign.lifecycle.home = None,
            15 => foreign.runtime = None,
            _ => foreign.origin = SandboxOrigin::Manual,
        }
        assert!(
            store
                .completed_installation_digest(&foreign)
                .await
                .unwrap()
                .is_none(),
            "mismatch {mismatch}"
        );
    }
    // Admission changes bound_at; it does not change the prepared process.
    let mut same_process = sandbox.clone();
    same_process.runtime.as_mut().unwrap().bound_at = LATER.into();
    assert_eq!(
        store
            .completed_installation_digest(&same_process)
            .await
            .unwrap(),
        Some(request.request_sha256)
    );
    // A new store/replica recovers the proof entirely from durable authority.
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .completed_installation_digest(&sandbox)
            .await
            .unwrap(),
        record.payload.installed_request_sha256
    );
    client.database(&name).drop().await.unwrap();
}

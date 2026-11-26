//! Real owner REST admission fixture; native proof is the only substituted port.
use super::*;
use layer2_core::sandbox::store::{AgentLogStore, SandboxStoreError};
use layer2_core::sandbox::*;
use layer3_adapter::{HomeVolumeMode, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeScope};
use layer3_adapter::{RuntimeImage, RUNTIME_CONTRACT_LABEL};

pub(crate) async fn reopen_fixture(mode: &str) -> Fixture {
    let mut f = fixture(3);
    let mut parent = stored_sandbox("sbx_update", "owner", SandboxStatus::Failed, 4);
    parent.failure = Some(TypedFailure {
        class: "restore_failed".into(),
        detail: "retained reopen failed".into(),
    });
    let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
    parent.profile.content.runtime_image = Some(image.clone());
    parent.runtime = Some(RuntimeBinding {
        runtime_id: "agent-sandbox:member:sandbox-uid".into(),
        runtime_version: image,
        provision_generation: Some(2),
        boot_id: Some("b".repeat(32)),
        bound_at: NOW.into(),
    });
    parent.lifecycle.home = Some(Box::new(home(&parent.id)));
    parent.lifecycle.runtime_generation_high_water = 2;
    parent.credential.key_generation = 1;
    parent.credential.delivered = true;
    parent.credential.current = Some(SandboxCredentialAuthority {
        generation: 1,
        provider_key_id: "fixture-credential-reference".into(),
        provider_state_version: 1,
        issued_at: NOW.into(),
        expires_at: "2026-09-06T00:00:00Z".into(),
        grant_fingerprint: "b".repeat(64),
    });
    parent.validate().unwrap();
    let workspace = stored_workspace(&parent, "wks_retained");
    let tree = stored_worktree(&workspace, "wkt_retained", "agt_retained");
    let mut agent = stored_agent(&workspace, &tree, "agt_retained");
    agent.status = AgentStatus::Paused;
    agent.ready_at = Some(NOW.into());
    agent.work.schedule(AgentWorkStep::None, NOW.into());
    agent.conversation.conversation_id = Some("retained-original-conversation".into());
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 3;
    insert_hierarchy(
        &f.store,
        &parent,
        Some(&workspace),
        Some(&tree),
        Some(&agent),
    )
    .await;
    for kind in [
        AgentLogKind::Created {
            workspace_id: workspace.id,
            worktree_id: tree.id,
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Ready,
            to: AgentStatus::Paused,
        },
    ] {
        f.store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    f.store
        .retained_reopen_snapshot(&parent.id)
        .await
        .unwrap()
        .validate()
        .unwrap();
    if mode != "unconfigured" {
        f.state = f
            .state
            .with_retained_reopening(Arc::new(RetainedReopenService::new(
                f.store.clone(),
                Arc::new(Verifier(mode.to_owned())),
                mode != "disabled",
            )));
    }
    f.router = hierarchy_router(f.state.clone());
    f
}

struct Verifier(String);
#[async_trait]
impl RetainedReopenVerifier for Verifier {
    async fn verify(
        &self,
        snapshot: &RetainedReopenSnapshot,
    ) -> Result<RetainedReopenProof, SandboxStoreError> {
        match self.0.as_str() {
            "proof_refused" => {
                return Err(SandboxStoreError::InvalidTransaction(
                    "native fixture refused",
                ))
            }
            "unavailable" => {
                return Err(SandboxStoreError::Unavailable(
                    "native fixture unavailable".into(),
                ))
            }
            _ => {}
        }
        let source = snapshot.retained.source().unwrap();
        let provider_plan = json!({"version":1,"idleReopen":true});
        Ok(RetainedReopenProof {
            expected_runtime: source.binding.clone(),
            observed_boot: source.expected_boot.into(),
            home_identity: source.home_identity,
            provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(
                &provider_plan,
            )
            .unwrap(),
            provider_plan,
            histories: snapshot
                .retained
                .agents
                .iter()
                .map(|agent| layer3_adapter::RecoveryConversationHistory {
                    agent_id: agent.id.to_string(),
                    conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                    history_sha256: "c".repeat(64),
                })
                .collect(),
        })
    }
}

pub(in crate::rest) fn home(id: &SandboxId) -> SandboxHome {
    use layer3_adapter::agent_sandbox::*;
    let policy = HomeVolumePolicy {
        mode: HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "standard-rwo".into(),
        capacity_gib: 8,
        template_name: String::new(),
        template_generation: String::new(),
    };
    let mut home = SandboxHome::reserve(id, policy.clone()).unwrap();
    home.request.scope = Some(
        HomeVolumeScope::from_authority(
            "native-vct-empty-v1",
            &["attachment-test", "default", "runtime"],
        )
        .unwrap(),
    );
    let request = NativePoolClaimRequest {
        pool: NativePoolBinding {
            spec: NativePoolSpec {
                cold_canary: false,
                image: RuntimeImage {
                    uri: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
                    labels: std::collections::BTreeMap::from([
                        (RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                        ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                        ("dev.chrono.cma.home-layout".into(), "2".into()),
                    ]),
                },
                cpu_millis: 1000,
                memory_mib: 2048,
                home: policy,
            },
            pod_policy: NativePoolPodPolicy {
                runtime_class: None,
                service_account: "runtime".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                fs_group_change_policy: None,
            },
            name: "pool".into(),
            uid: "pool-uid".into(),
            template_name: "template".into(),
            template_uid: "template-uid".into(),
            config_sha256: "a".repeat(64),
        },
        home: home.request.clone(),
        runtime_generation: 1,
    };
    let claim = NativePoolClaim {
        request: request.clone(),
        name: "claim".into(),
        uid: "claim-uid".into(),
        resource_version: "1".into(),
        sandbox_name: Some("member".into()),
    };
    home.native_pool = Some(Box::new(SandboxNativePool {
        request,
        claim: Some(claim.clone()),
        cleanup: None,
        cleaned: false,
    }));
    home.bind(HomeVolumeReceipt {
        native_authority: Some(Box::new(NativePoolHomeOrigin {
            claim,
            sandbox_uid: "sandbox-uid".into(),
            pvc_uid: "pvc-uid".into(),
        })),
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home-member".into(),
        storage_generation: "pvc-uid".into(),
    })
    .unwrap();
    home
}

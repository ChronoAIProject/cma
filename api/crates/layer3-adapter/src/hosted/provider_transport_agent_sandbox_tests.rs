//! Native persistent homes never acquire CMA archive authority or issue checkpoint commands.
use super::*;
use crate::agent_sandbox::{AgentSandboxClient, AgentSandboxConfig};
use crate::{
    CheckpointHoldRequest, CheckpointRequest, CheckpointWriterResumeRequest,
    RestoreBootstrapRequest, RestoreRequest,
};

pub(super) fn native_transport(address: std::net::SocketAddr) -> ProviderHostedControlTransport {
    let client = AgentSandboxClient::new(
        kube::Client::try_from(kube::Config::new(
            format!("http://{address}").parse().unwrap(),
        ))
        .unwrap(),
        AgentSandboxConfig {
            namespace: "cma-test".into(),
            deployment: "cma-test".into(),
            service_account: "cma-runtime".into(),
            runtime_class: None,
            image_pull_secrets: vec![],
            template_network_policy: None,
            template_fs_group_change_policy: None,
            native_claim_guard_enabled: false,
            runtime_startup_breaker_enabled: false,
            control_key: "fixture".repeat(8),
        },
        "fixture",
    )
    .unwrap();
    ProviderHostedControlTransport::for_agent_sandbox(client).unwrap()
}

#[tokio::test]
async fn capacity_recovery_proof_uses_direct_native_header_authentication() {
    let api = listener().await;
    let transport = native_transport(api.local_addr().unwrap());
    let runtime = listener().await;
    let endpoint = RuntimeEndpoint::parse(
        "http://runtime.invalid",
        &format!("http://{}", runtime.local_addr().unwrap()),
    )
    .unwrap()
    .with_control_token("fixture-runtime-control".into());
    let request = crate::agent_capacity_recovery::tests::request(None);
    let expected = crate::agent_capacity_recovery::CapacityRecoveryNativeProof {
        confirmation: crate::agent_capacity_recovery::tests::confirmation(&request),
        cwd: "/workspace".into(),
    };
    let served = spawn_http(runtime, json_response(&expected));
    assert_eq!(
        transport
            .capacity_recovery_proof(&endpoint, &request)
            .await
            .unwrap(),
        expected
    );
    let wire = request_text(served.await.unwrap());
    assert!(wire.starts_with("POST /runtime/agents/agent/capacity-recovery HTTP/1.1"));
    assert!(wire
        .to_ascii_lowercase()
        .contains("x-cma-runtime-token: fixture-runtime-control"));
    assert!(!wire
        .lines()
        .next()
        .unwrap()
        .contains("fixture-runtime-control"));
}

#[tokio::test]
async fn native_binding_and_control_work_without_an_archive_store() {
    let api = listener().await;
    let transport = native_transport(api.local_addr().unwrap());
    let handle = RuntimeHandle::new("agent-sandbox:runtime-one:original-uid");
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    assert!(matches!(
        transport.bind_runtime(&handle, "sandbox-b").await,
        Err(AdapterError::Rejected(_))
    ));
    let runtime = listener().await;
    let endpoint = RuntimeEndpoint::parse(
        "http://runtime.invalid",
        &format!("http://{}", runtime.local_addr().unwrap()),
    )
    .unwrap()
    .with_control_token("fixture-runtime-control".into());
    let response = ResponseFrame::Resources(ResourceReading::default());
    let served = spawn_http(runtime, json_response(&response));
    assert_eq!(
        transport
            .exchange_at(&handle, &endpoint, RequestFrame::Resources)
            .await
            .unwrap(),
        response
    );
    let request = request_text(served.await.unwrap());
    assert!(request.starts_with("POST /runtime/control HTTP/1.1"));
    assert!(request
        .to_ascii_lowercase()
        .contains("x-cma-runtime-token: fixture-runtime-control"));
    assert!(matches!(
        transport.archive(&handle).await,
        Err(AdapterError::Unsupported(_))
    ));
    transport.retire(&handle).await.unwrap();
    transport.bind_runtime(&handle, "sandbox-b").await.unwrap();
}

#[tokio::test]
async fn native_checkpoint_and_restore_are_rejected_before_any_provider_or_runtime_io() {
    let api = listener().await;
    let transport = native_transport(api.local_addr().unwrap());
    let handle = RuntimeHandle::new("agent-sandbox:runtime-one:original-uid");
    transport.bind_runtime(&handle, "sandbox-a").await.unwrap();
    let checkpoint = CheckpointRequest {
        epoch: 1,
        exclusions: vec![],
    };
    let held = CheckpointHoldRequest {
        checkpoint: checkpoint.clone(),
        writer_resume_token: "a".repeat(64),
    };
    let restore = RestoreRequest {
        object_reference: format!("home-{}.cma", "b".repeat(64)),
        sha256: "b".repeat(64),
    };
    for frame in [
        RequestFrame::Checkpoint(checkpoint),
        RequestFrame::CheckpointForTeardown(held.clone()),
        RequestFrame::ResumeCheckpointWriters(CheckpointWriterResumeRequest {
            writer_resume_token: "a".repeat(64),
        }),
        RequestFrame::BootstrapRestore(RestoreBootstrapRequest::new(
            "bootstrap-one",
            "sandbox-a",
            restore.clone(),
        )),
        RequestFrame::Restore(restore),
    ] {
        assert!(matches!(
            transport.exchange(&handle, frame).await,
            Err(AdapterError::Unsupported(_))
        ));
    }
    assert!(matches!(
        transport.verify_checkpoint_hold(&handle, &held).await,
        Err(AdapterError::Unsupported(_))
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), api.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn native_and_retained_constructors_pin_json_policy_over_renderer_preferences() {
    let api = listener().await;
    for enabled in [false, true] {
        let native = native_transport(api.local_addr().unwrap())
            .with_retained_json(enabled)
            .with_coordinated_ingress(enabled);
        let retained = ProviderHostedControlTransport::for_retained_opensandbox(
            SandboxClient::new(format!("http://{}", api.local_addr().unwrap()), "fixture"),
        )
        .unwrap()
        .with_retained_json(enabled)
        .with_coordinated_ingress(enabled);
        for transport in [native, retained] {
            let mut settings = HostedAgentOpenSettings {
                output_json_budget: false,
                shared_runtime_output: false,
                bounded_permission_state: false,
                coordinated_ingress: !enabled,
                retained_json: !enabled,
                bounded_raw_output: false,
                already_initialized_error: None,
                open_completion: None,
                context_refresh: None,
                permission_mode: None,
                own_credential: false,
                channel_tool: None,
                provider_tool: None,
                access_tool: None,
                recovered_prompt: None,
                recovered_context_sequence: None,
            };
            transport.pin_output_policy(&mut settings);
            assert_eq!(settings.retained_json, enabled);
            assert_eq!(settings.coordinated_ingress, enabled);
            assert!(!settings.bounded_raw_output && !settings.output_json_budget);
        }
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(30), api.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn open_reply_loss_matches_only_the_configured_bound_sandbox() {
    struct Claim;

    #[async_trait::async_trait]
    impl crate::hosted::HostedOpenReplyLoss for Claim {
        async fn claim(&self) -> bool {
            unreachable!("selecting a target must not consume the claim")
        }
    }

    let api = listener().await;
    let transport = native_transport(api.local_addr().unwrap());
    let target = RuntimeHandle::new("target-runtime");
    let other = RuntimeHandle::new("other-runtime");
    transport.bind_sandbox(&target, "target").await.unwrap();
    transport.bind_sandbox(&other, "target-2").await.unwrap();

    assert!(transport.open_reply_loss_for(&target).await.is_none());
    let transport = transport.with_open_reply_loss("target".into(), Arc::new(Claim));
    assert!(transport.open_reply_loss_for(&target).await.is_some());
    assert!(transport.open_reply_loss_for(&other).await.is_none());
    assert!(transport
        .open_reply_loss_for(&RuntimeHandle::new("unbound-runtime"))
        .await
        .is_none());
}

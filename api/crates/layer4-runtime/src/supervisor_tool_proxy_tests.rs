use super::*;
use layer3_adapter::{CredentialFrame, InstallRequest, RuntimeCredential, ToolProxyRequest};

#[tokio::test]
async fn tool_proxy_proof_requires_the_live_endpoint_and_current_custody_generation() {
    let home = std::env::temp_dir().join(format!("cma-tool-proof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut config = RuntimeConfig::for_test(home.clone());
    config.tool_proxy = Some(crate::tool_proxy::ToolProxyConfig {
        upstream: "https://gateway.example.test/mcp".parse().unwrap(),
    });
    // Capability discovery must also work for runtimes without a persistent home.
    assert!(config.home_identity.is_none());
    let mut request = ToolProxyRequest {
        sandbox_id: config.sandbox_id.clone(),
        generation: config.generation,
        endpoint: config.tool_proxy.as_ref().unwrap().upstream.to_string(),
        credential_generation: 1,
    };
    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    let observe = |request| RequestFrame::ToolProxyStatus(request);
    assert!(matches!(
        supervisor.dispatch(observe(request.clone())).await,
        ResponseFrame::Failure(_)
    ));
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "tool-proof-install",
                Vec::new(),
            )))
            .await,
        ResponseFrame::Install(_)
    ));
    for generation in [1, 2] {
        assert!(matches!(
            supervisor
                .dispatch(RequestFrame::Credential(CredentialFrame {
                    operation_id: format!("tool-proof-credential-{generation}"),
                    value: RuntimeCredential::new(format!("fixture-credential-{generation}")),
                    custody_generation: Some(generation),
                    github_route: None,
                }))
                .await,
            ResponseFrame::Credential(_)
        ));
        if generation == 2 {
            assert!(matches!(
                supervisor.dispatch(observe(request.clone())).await,
                ResponseFrame::Failure(_)
            ));
        }
        request.credential_generation = generation;
        let ResponseFrame::ToolProxyStatus(status) =
            supervisor.dispatch(observe(request.clone())).await
        else {
            panic!("the installed proxy must report its exact authority");
        };
        status.validate_for(&request).unwrap();
        assert_eq!(status.boot_id, supervisor.boot_id);
        for case in 0..4 {
            let mut changed = request.clone();
            match case {
                0 => changed.sandbox_id.push_str("-other"),
                1 => changed.generation += 1,
                2 => changed.endpoint.push_str("/other"),
                _ => changed.credential_generation += 1,
            }
            assert!(matches!(
                supervisor.dispatch(observe(changed)).await,
                ResponseFrame::Failure(_)
            ));
        }
    }
    // A restarted supervisor must deliver credentials and start its own proxy.
    let replacement = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    assert_ne!(replacement.boot_id, supervisor.boot_id);
    assert!(matches!(
        replacement.dispatch(observe(request.clone())).await,
        ResponseFrame::Failure(_)
    ));
    supervisor
        .credential
        .tool_proxy_serving
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(matches!(
        supervisor.dispatch(observe(request)).await,
        ResponseFrame::Failure(_)
    ));
    std::fs::remove_dir_all(&home).unwrap();
    let _ = std::fs::remove_dir_all(config.checkpoint_root);
    let _ = std::fs::remove_dir_all(config.state_root);
}

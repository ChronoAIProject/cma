use super::*;

#[path = "link_native_close_effect_fixture.rs"]
mod close_effect;
use crate::config::RuntimeConfig;
use layer3_adapter::wire::{
    AGENT_COMPUTER_STATUS_MEDIA_TYPE, AGENT_PROCESS_RESTART_STATUS_MEDIA_TYPE,
    AGENT_RESOURCES_STATUS_MEDIA_TYPE, AGENT_SKILL_UPGRADE_STATUS_MEDIA_TYPE,
    GITHUB_CREDENTIAL_ROUTE_STATUS_MEDIA_TYPE,
};
use layer3_adapter::RuntimeStatusRequest;

#[tokio::test]
async fn agent_restart_capability_requires_opt_in_and_an_enabled_runtime() {
    for enabled in [false, true] {
        let config = RuntimeConfig {
            home_identity: Some("a".repeat(43)),
            agent_process_restart_enabled: enabled,
            ..RuntimeConfig::for_test(root("restart-negotiation"))
        };
        std::fs::create_dir_all(&config.home).unwrap();
        let supervisor = Supervisor::new(
            config.clone(),
            crate::writer_fence::SupervisorBoundary::default(),
        )
        .unwrap();
        let identity = RuntimeStatusRequest {
            sandbox_id: config.sandbox_id.clone(),
            generation: config.generation,
            home_identity: config.home_identity.clone().unwrap(),
        };
        let (address, server) = serve(supervisor).await;
        let client = reqwest::Client::new();
        for (accept, frame, field_present) in [
            (
                "application/json",
                RequestFrame::RuntimeStatus(identity.clone()),
                false,
            ),
            (
                AGENT_PROCESS_RESTART_STATUS_MEDIA_TYPE,
                RequestFrame::RuntimeStatus(identity.clone()),
                enabled,
            ),
            (
                AGENT_PROCESS_RESTART_STATUS_MEDIA_TYPE,
                RequestFrame::WorkspaceRemovalSupport(identity.clone()),
                false,
            ),
        ] {
            let value: serde_json::Value = client
                .post(format!("http://{address}/runtime/control"))
                .header(header::ACCEPT, accept)
                .json(&frame)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let status = match frame {
                RequestFrame::RuntimeStatus(_) => {
                    assert_eq!(value["t"], "runtime_status");
                    &value
                }
                _ => {
                    assert_eq!(value["t"], "workspace_removal_support");
                    &value["runtime"]
                }
            };
            assert_eq!(
                status.get("agent_process_restart"),
                field_present.then_some(&serde_json::Value::Bool(true))
            );
        }
        server.abort();
        let _ = server.await;
        for path in [&config.home, &config.state_root, &config.checkpoint_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[tokio::test]
async fn agent_resource_status_extension_is_explicitly_negotiated_per_request() {
    let config = RuntimeConfig {
        home_identity: Some("a".repeat(43)),
        ..RuntimeConfig::for_test(root("resources-negotiation"))
    };
    std::fs::create_dir_all(&config.home).unwrap();
    let supervisor = Supervisor::new(
        config.clone(),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .unwrap();
    let request = RequestFrame::RuntimeStatus(RuntimeStatusRequest {
        sandbox_id: config.sandbox_id.clone(),
        generation: config.generation,
        home_identity: config.home_identity.clone().unwrap(),
    });
    let (address, server) = serve(supervisor).await;
    let client = reqwest::Client::new();
    let mut original = None;
    let combined =
        format!("{AGENT_RESOURCES_STATUS_MEDIA_TYPE}, {AGENT_COMPUTER_STATUS_MEDIA_TYPE}");
    let files_supported = crate::agent_attachment::supported(&config);
    for (accept, resources, computer, upgrade) in [
        (None, false, false, false),
        (Some(AGENT_RESOURCES_STATUS_MEDIA_TYPE), true, false, false),
        (Some(AGENT_COMPUTER_STATUS_MEDIA_TYPE), false, true, false),
        (Some(combined.as_str()), true, true, false),
        (Some("application/json"), false, false, false),
        (
            Some(AGENT_SKILL_UPGRADE_STATUS_MEDIA_TYPE),
            false,
            false,
            true,
        ),
        (None, false, false, false),
    ] {
        let mut http = client
            .post(format!("http://{address}/runtime/control"))
            .json(&request);
        if let Some(accept) = accept {
            http = http.header(header::ACCEPT, accept);
        }
        let mut value: serde_json::Value = http.send().await.unwrap().json().await.unwrap();
        assert_eq!(value["t"], "runtime_status");
        for (capability, supported) in [
            ("agent_browser", computer),
            ("agent_skill_upgrade", upgrade),
            ("agent_filesystem", files_supported),
            ("agent_attachment_upload", files_supported),
        ] {
            assert_eq!(
                value.get(capability),
                supported.then_some(&serde_json::Value::Bool(true)),
                "unexpected {capability} for {accept:?}"
            );
        }
        assert_eq!(value.get("agent_resource_preparation").is_some(), resources);
        let decoded: ResponseFrame = serde_json::from_value(value.clone()).unwrap();
        let ResponseFrame::RuntimeStatus(status) = decoded else {
            panic!("expected status")
        };
        assert_eq!(status.agent_resource_preparation, resources);
        assert_eq!(status.agent_browser, computer);
        assert_eq!(status.agent_skill_upgrade, upgrade);
        assert_eq!(status.agent_filesystem, files_supported);
        assert_eq!(status.agent_attachment_upload, files_supported);
        for capability in [
            "agent_resource_preparation",
            "agent_browser",
            "agent_skill_upgrade",
        ] {
            value.as_object_mut().unwrap().remove(capability);
        }
        if let Some(original) = &original {
            assert_eq!(&value, original);
        } else {
            original = Some(value);
        }
    }
    // Other contract-3 frames embed status and must stay readable by old APIs.
    for accept in [
        "application/json",
        AGENT_RESOURCES_STATUS_MEDIA_TYPE,
        AGENT_COMPUTER_STATUS_MEDIA_TYPE,
        AGENT_SKILL_UPGRADE_STATUS_MEDIA_TYPE,
        combined.as_str(),
    ] {
        let RequestFrame::RuntimeStatus(identity) = &request else {
            unreachable!()
        };
        let response: serde_json::Value = client
            .post(format!("http://{address}/runtime/control"))
            .header(header::ACCEPT, accept)
            .json(&RequestFrame::WorkspaceRemovalSupport(identity.clone()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["t"], "workspace_removal_support");
        assert!(response["runtime"].get("agent_browser").is_none());
        assert!(response["runtime"].get("agent_skill_upgrade").is_none());
        for capability in ["agent_filesystem", "agent_attachment_upload"] {
            assert_eq!(
                response["runtime"].get(capability),
                files_supported.then_some(&serde_json::Value::Bool(true))
            );
        }
        assert!(response["runtime"]
            .get("agent_resource_preparation")
            .is_none());
    }
    server.abort();
    let _ = server.await;
    for path in [&config.home, &config.state_root, &config.checkpoint_root] {
        let _ = std::fs::remove_dir_all(path);
    }
}

#[tokio::test]
async fn github_credential_route_status_requires_negotiation_and_a_configured_proxy() {
    for configured in [false, true] {
        let mut config = RuntimeConfig {
            home_identity: Some("a".repeat(43)),
            ..RuntimeConfig::for_test(root(if configured { "gh-status" } else { "gh-absent" }))
        };
        if configured {
            let platform = crate::config::ProxyConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                upstream: "https://provider.example".into(),
                role: "platform",
            };
            config.github_proxy = crate::github_proxy::configuration(
                &config.state_root,
                &mut config.process_env,
                Some(&platform),
            )
            .unwrap();
        }
        std::fs::create_dir_all(&config.home).unwrap();
        let supervisor = Supervisor::new(
            config.clone(),
            crate::writer_fence::SupervisorBoundary::default(),
        )
        .unwrap();
        let identity = RuntimeStatusRequest {
            sandbox_id: config.sandbox_id.clone(),
            generation: config.generation,
            home_identity: config.home_identity.clone().unwrap(),
        };
        let (address, server) = serve(supervisor).await;
        let client = reqwest::Client::new();
        for (accept, advertised) in [
            ("application/json", false),
            (GITHUB_CREDENTIAL_ROUTE_STATUS_MEDIA_TYPE, configured),
        ] {
            let frame: ResponseFrame = client
                .post(format!("http://{address}/runtime/control"))
                .header(header::ACCEPT, accept)
                .json(&RequestFrame::RuntimeStatus(identity.clone()))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let ResponseFrame::RuntimeStatus(status) = frame else {
                panic!("expected runtime status");
            };
            status.validate_for(&identity).unwrap();
            assert_eq!(status.exact_github_credential_route, advertised);
            assert_eq!(
                serde_json::to_value(status)
                    .unwrap()
                    .get("exact_github_credential_route")
                    .is_some(),
                advertised
            );
        }
        server.abort();
        let _ = server.await;
        for path in [&config.home, &config.state_root, &config.checkpoint_root] {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[tokio::test]
async fn selected_credential_can_only_be_installed_by_the_negotiated_receiver() {
    let platform = crate::config::ProxyConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        upstream: "https://provider.example".into(),
        role: "platform",
    };
    let mut supervisors = Vec::new();
    let mut cleanup = Vec::new();
    for name in ["gh-one", "gh-two"] {
        let mut config = RuntimeConfig {
            home_identity: Some("a".repeat(43)),
            ..RuntimeConfig::for_test(root(name))
        };
        config.github_proxy = crate::github_proxy::configuration(
            &config.state_root,
            &mut config.process_env,
            Some(&platform),
        )
        .unwrap();
        std::fs::create_dir_all(&config.home).unwrap();
        cleanup.extend([
            config.home.clone(),
            config.state_root.clone(),
            config.checkpoint_root.clone(),
        ]);
        let supervisor =
            Supervisor::new(config, crate::writer_fence::SupervisorBoundary::default()).unwrap();
        let response = supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "install",
                vec![],
            )))
            .await;
        assert!(
            matches!(response, ResponseFrame::Install(_)),
            "{response:?}"
        );
        supervisors.push(serve(supervisor).await);
    }
    let identity = RuntimeStatusRequest {
        sandbox_id: "sandbox-test".into(),
        generation: 1,
        home_identity: "a".repeat(43),
    };
    let client = reqwest::Client::new();
    let first = supervisors[0].0;
    let replacement = supervisors[1].0;
    let status: ResponseFrame = client
        .post(format!("http://{first}/runtime/control"))
        .header(header::ACCEPT, GITHUB_CREDENTIAL_ROUTE_STATUS_MEDIA_TYPE)
        .json(&RequestFrame::RuntimeStatus(identity.clone()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ResponseFrame::RuntimeStatus(status) = status else {
        panic!("expected runtime status");
    };
    assert!(status.installation_ready && status.exact_github_credential_route);
    let request = RequestFrame::CredentialForReceiver {
        receiver: identity.clone(),
        boot_id: status.boot_id,
        credential: CredentialFrame {
            operation_id: "selected-credential".into(),
            value: RuntimeCredential::new("synthetic-selected-credential"),
            custody_generation: Some(1),
            github_route: Some(layer3_adapter::GithubCredentialRoute {
                catalog_id: "github-catalog".into(),
                service_id: "selected-connection".into(),
                other_services: Vec::new(),
                repository_scope: None,
            }),
        },
    };
    let response = exchange(&client, replacement, &request).await;
    assert!(
        matches!(response, ResponseFrame::Failure(ref failure) if failure.code == "credential_receiver_changed")
    );
    let replacement_status =
        exchange(&client, replacement, &RequestFrame::RuntimeStatus(identity)).await;
    assert!(
        matches!(replacement_status, ResponseFrame::RuntimeStatus(status) if !status.credential_ready)
    );

    let response = exchange(&client, first, &request).await;
    assert!(
        matches!(response, ResponseFrame::Credential(receipt) if receipt.installed && receipt.operation_id == "selected-credential")
    );
    for (_, server) in supervisors {
        server.abort();
        let _ = server.await;
    }
    for path in cleanup {
        let _ = std::fs::remove_dir_all(path);
    }
}

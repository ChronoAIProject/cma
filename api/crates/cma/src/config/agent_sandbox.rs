use layer3_adapter::agent_sandbox::{
    AgentSandboxConfig, NativeFsGroupChangePolicy, NativeTemplateNetworkPolicy,
    RemoteKubernetesAuth, RemoteKubernetesConfig,
};

pub(super) fn transport_from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<layer3_adapter::hosted::NativeRuntimeTransport, String> {
    use layer3_adapter::hosted::NativeRuntimeTransport;
    match lookup("CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT")?.as_deref() {
        None | Some("direct") => Ok(NativeRuntimeTransport::Direct),
        Some("kubernetes-portforward") => Ok(NativeRuntimeTransport::KubernetesPortforward),
        Some("kubernetes-exec") => Ok(NativeRuntimeTransport::KubernetesExec),
        _ => Err(
            "CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT must be direct, kubernetes-portforward or kubernetes-exec"
                .into(),
        ),
    }
}

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    namespace: &str,
    allocate: bool,
) -> Result<Option<AgentSandboxConfig>, String> {
    let enabled = match lookup("CHRONO_AGENT_SANDBOX_ENABLED")?.as_deref() {
        Some("true") => true,
        Some("false") | None => allocate,
        _ => return Err("CHRONO_AGENT_SANDBOX_ENABLED must be true or false".into()),
    };
    if !enabled {
        return Ok(None);
    }
    let key_file = super::required(lookup, "CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE")?;
    let control_key = std::fs::read_to_string(&key_file)
        .map_err(|_| "agent-sandbox control key file is unavailable".to_string())?
        .trim()
        .to_owned();
    let value = AgentSandboxConfig {
        namespace: lookup("CHRONO_AGENT_SANDBOX_NAMESPACE")?
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| namespace.into()),
        deployment: super::runtime::deployment_id_from_lookup(lookup)?,
        runtime_class: lookup("CHRONO_AGENT_SANDBOX_RUNTIME_CLASS")?.filter(|s| !s.is_empty()),
        service_account: lookup("CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT")?
            .unwrap_or_else(|| "cma-runtime".into()),
        image_pull_secrets: lookup("CHRONO_AGENT_SANDBOX_IMAGE_PULL_SECRETS")?
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        template_network_policy: lookup("CHRONO_AGENT_SANDBOX_TEMPLATE_NETWORK_POLICY")?
            .filter(|value| !value.is_empty())
            .map(|value| {
                NativeTemplateNetworkPolicy::parse(&value).map_err(|error| error.to_string())
            })
            .transpose()?,
        // Persist this new-pool capability in its immutable policy. Retained
        // pools reconstruct the old policy rather than inheriting this default.
        template_fs_group_change_policy: Some(NativeFsGroupChangePolicy::OnRootMismatch),
        native_claim_guard_enabled: claim_guard_from_lookup(lookup)?,
        runtime_startup_breaker_enabled: startup_breaker_from_lookup(lookup)?,
        control_key,
    };
    value.validate().map_err(|e| e.to_string())?;
    Ok(Some(value))
}

pub(super) fn connection_from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<RemoteKubernetesConfig>, String> {
    let server = lookup("CHRONO_AGENT_SANDBOX_API_SERVER")?.filter(|value| !value.is_empty());
    let ca_file = lookup("CHRONO_AGENT_SANDBOX_CA_FILE")?.filter(|value| !value.is_empty());
    let token_file = lookup("CHRONO_AGENT_SANDBOX_TOKEN_FILE")?.filter(|value| !value.is_empty());
    let mode = lookup("CHRONO_AGENT_SANDBOX_AUTH_MODE")?;
    let google = match mode.as_deref() {
        None | Some("token-file") => false,
        Some("google-metadata") => true,
        _ => {
            return Err(
                "CHRONO_AGENT_SANDBOX_AUTH_MODE must be token-file or google-metadata".into(),
            )
        }
    };
    if server.is_none() && ca_file.is_none() && token_file.is_none() && !google {
        return Ok(None);
    }
    if google && token_file.is_some() {
        return Err(
            "CHRONO_AGENT_SANDBOX_TOKEN_FILE must be unset for google-metadata authentication"
                .into(),
        );
    }
    if server.is_none() || ca_file.is_none() || (!google && token_file.is_none()) {
        return Err("native API_SERVER and CA_FILE must be configured together with TOKEN_FILE for token-file authentication or AUTH_MODE=google-metadata".into());
    }
    let connection = RemoteKubernetesConfig {
        server: server.unwrap(),
        ca_file: ca_file.unwrap(),
        auth: if google {
            RemoteKubernetesAuth::GoogleMetadata
        } else {
            RemoteKubernetesAuth::TokenFile {
                path: token_file.unwrap(),
            }
        },
    };
    connection.validate().map_err(|error| error.to_string())?;
    Ok(Some(connection))
}

fn claim_guard_from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    match lookup("CHRONO_SANDBOX_NATIVE_CLAIM_GUARD_ENABLED")?.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err("CHRONO_SANDBOX_NATIVE_CLAIM_GUARD_ENABLED must be true or false".into()),
    }
}

#[cfg(test)]
mod claim_guard_tests {
    #[test]
    fn default_off_and_strict_boolean() {
        for (input, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            assert_eq!(
                super::claim_guard_from_lookup(&mut |_| Ok(input.map(str::to_owned))).unwrap(),
                expected
            );
        }
        for input in ["", "TRUE", "1", "yes", " true "] {
            assert!(super::claim_guard_from_lookup(&mut |_| Ok(Some(input.into()))).is_err());
        }
        assert!(super::claim_guard_from_lookup(&mut |_| Err("lookup failure".into())).is_err());
    }
}

fn startup_breaker_from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    match lookup("CHRONO_SANDBOX_RUNTIME_STARTUP_BREAKER_ENABLED")?.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err("CHRONO_SANDBOX_RUNTIME_STARTUP_BREAKER_ENABLED must be true or false".into()),
    }
}

#[cfg(test)]
mod startup_breaker_tests {
    #[test]
    fn default_off_and_strict_boolean() {
        for (input, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            assert_eq!(
                super::startup_breaker_from_lookup(&mut |key| {
                    assert_eq!(key, "CHRONO_SANDBOX_RUNTIME_STARTUP_BREAKER_ENABLED");
                    Ok(input.map(str::to_owned))
                })
                .unwrap(),
                expected
            );
        }
        for input in ["", "TRUE", "1", "yes", " true "] {
            assert!(super::startup_breaker_from_lookup(&mut |_| Ok(Some(input.into()))).is_err());
        }
        assert!(super::startup_breaker_from_lookup(&mut |_| Err("lookup failure".into())).is_err());
    }
}

pub(super) fn legacy_receiver_image(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<layer3_adapter::hosted::LegacyReceiverImage>, String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReviewedImage {
        manifest: String,
        config_digest: String,
        source_revision: String,
    }

    let Some(raw) = lookup("CHRONO_LEGACY_RECEIVER_IMAGE_PROOF")? else {
        return Ok(None);
    };
    let invalid = || {
        "CHRONO_LEGACY_RECEIVER_IMAGE_PROOF must contain the reviewed manifest, config_digest and source_revision".to_owned()
    };
    if raw.len() > 4096 {
        return Err(invalid());
    }
    let proof: ReviewedImage = serde_json::from_str(&raw).map_err(|_| invalid())?;
    layer3_adapter::hosted::LegacyReceiverImage::from_reviewed_mapping(
        proof.manifest,
        proof.config_digest,
        &proof.source_revision,
    )
    .map(Some)
    .map_err(|_| invalid())
}

#[cfg(test)]
mod legacy_receiver_tests {
    #[test]
    fn legacy_receiver_image_is_default_off_and_validates_private_input() {
        assert!(super::legacy_receiver_image(&mut |key| {
            assert_eq!(key, "CHRONO_LEGACY_RECEIVER_IMAGE_PROOF");
            Ok(None)
        })
        .unwrap()
        .is_none());
        assert!(super::legacy_receiver_image(&mut |_| Ok(Some("{}".into()))).is_err());
        let proof = serde_json::json!({
            "manifest": format!("ghcr.io/fixture/runtime@sha256:{}", "1".repeat(64)),
            "config_digest": format!("sha256:{}", "2".repeat(64)),
            "source_revision": "bc8cd8bf0f585446a37d3bd50a8e3df2d6d06e6e",
        });
        assert!(
            super::legacy_receiver_image(&mut |_| Ok(Some(proof.to_string())))
                .unwrap()
                .is_some()
        );
    }
}

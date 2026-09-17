use layer2_core::resource_access::ResourceShareStore;

use super::*;
use crate::bridge::{ManagedBridgeCredentials, NyxBridgeValidator};

/// Deployment configuration chooses issuer/JWKS and callback root. Callback
/// claims and URL parameters can never select an outbound identity endpoint.
pub(crate) fn configured(
    mode: &crate::auth::AuthMode,
    nyxid_public_url: Option<&str>,
    bridges: Arc<dyn BridgeStore>,
    credentials: Option<Arc<ManagedBridgeCredentials>>,
    resources: Arc<dyn ResourceShareStore>,
    inbox: Arc<dyn CallbackInboxStore>,
    flags: crate::config::BridgeFeatureFlags,
) -> Result<Option<CallbackIngress>, CallbackError> {
    if !flags.enabled(crate::config::bridge_flags::BridgeFlag::Ingress) {
        return Ok(None);
    }
    let Some(credentials) = credentials else {
        return Ok(None);
    };
    let Some(deployment) = deployment(mode, nyxid_public_url)? else {
        return Ok(None);
    };
    let connections = crate::bridge::feature_policy::configured(
        Arc::new(NyxBridgeValidator {
            provisioning: credentials.provisioning.clone(),
            custody: credentials,
            resources,
            flags,
        }),
        flags,
    );
    let mut ingress = CallbackIngress::new(
        bridges,
        connections,
        inbox,
        deployment.keys,
        deployment.policy,
        deployment.callback_root,
    )?;
    ingress.text_attachments =
        flags.enabled(crate::config::bridge_flags::BridgeFlag::DiscordTextAttachments);
    Ok(Some(ingress))
}

struct Deployment {
    keys: PinnedCallbackKeys,
    policy: CallbackTokenPolicy,
    callback_root: String,
}

fn deployment(
    mode: &crate::auth::AuthMode,
    nyxid_public_url: Option<&str>,
) -> Result<Option<Deployment>, CallbackError> {
    let crate::auth::AuthMode::Provider {
        base_url,
        public_url,
        ..
    } = mode
    else {
        return Ok(None);
    };
    // Bridge callbacks are an optional capability. Older deployments without
    // the public JWKS origin must keep starting and serving retained sessions.
    let Some(nyxid_public_url) = nyxid_public_url else {
        return Ok(None);
    };
    if ![base_url, public_url]
        .iter()
        .all(|value| reqwest::Url::parse(value).is_ok_and(|url| url.scheme() == "https"))
    {
        return Ok(None);
    }
    let issuer = base_url.trim_end_matches('/').to_owned();
    let endpoint = format!(
        "{}/.well-known/jwks.json",
        nyxid_public_url.trim_end_matches('/')
    );
    Ok(Some(Deployment {
        keys: PinnedCallbackKeys::new(issuer.clone(), endpoint)?,
        policy: CallbackTokenPolicy {
            issuer: issuer.clone(),
            relay_audience: issuer,
            callback_ttl_seconds: 300,
            relay_ttl_seconds: 300,
            reply_ttl_seconds: 1800,
            clock_skew_seconds: 60,
        },
        callback_root: format!(
            "{}/api/v1/bridge-callbacks",
            public_url.trim_end_matches('/')
        ),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(base: &str, public: &str) -> crate::auth::AuthMode {
        crate::auth::AuthMode::Provider {
            base_url: base.into(),
            public_url: public.into(),
            client_id: "fixture".into(),
            client_secret: "fixture".into(),
        }
    }

    #[test]
    fn missing_optional_endpoint_trusted_or_http_deployments_keep_callback_capability_unavailable()
    {
        let standard = provider("https://nyx.example", "https://cma.example");
        assert!(deployment(&standard, None).unwrap().is_none());
        assert!(deployment(
            &crate::auth::AuthMode::Trusted {
                subject: "fixture".into()
            },
            Some("https://nyx.example")
        )
        .unwrap()
        .is_none());
        for mode in [
            provider("http://nyx.internal", "https://cma.example"),
            provider("https://nyx.example", "http://localhost:8080"),
        ] {
            assert!(deployment(&mode, Some("https://nyx.example"))
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn explicit_valid_endpoint_pins_policy_and_invalid_supplied_endpoint_is_rejected() {
        let mode = provider("https://nyx.example/", "https://cma.example/");
        let value = deployment(&mode, Some("https://public.nyx.example/"))
            .unwrap()
            .unwrap();
        assert_eq!(value.policy.issuer, "https://nyx.example");
        assert_eq!(value.policy.relay_audience, "https://nyx.example");
        assert_eq!(
            value.callback_root,
            "https://cma.example/api/v1/bridge-callbacks"
        );
        for endpoint in [
            "http://nyx.example",
            "https://user:secret@nyx.example",
            "https://nyx.example/?token=secret",
        ] {
            assert!(matches!(
                deployment(&mode, Some(endpoint)),
                Err(CallbackError::KeysUnavailable)
            ));
        }
    }

    #[test]
    fn configured_jwks_uses_provider_discovery_path_on_pinned_public_origin() {
        // NyxID discovery advertises /.well-known/jwks.json. Its frontend
        // proxies /.well-known; /jwks.json falls through to the HTML app.
        let provider_jwks =
            reqwest::Url::parse("https://nyx.example/.well-known/jwks.json").unwrap();
        let mode = provider("https://nyx.example", "https://cma.example");
        for public_origin in ["https://public.nyx.example", "https://public.nyx.example/"] {
            let configured = deployment(&mode, Some(public_origin)).unwrap().unwrap();
            let endpoint = reqwest::Url::parse(configured.keys.endpoint()).unwrap();
            assert_eq!(endpoint.path(), provider_jwks.path());
            assert_eq!(
                endpoint.origin().ascii_serialization(),
                "https://public.nyx.example"
            );
            assert_eq!(configured.policy.issuer, "https://nyx.example");
        }
    }
}

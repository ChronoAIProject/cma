//! An opt-in immutable image proof for receiver-less native runtimes.
use super::*;
use crate::agent_sandbox::ReceiverIncarnation;
use crate::hosted::reply::valid_sha256;

// This reviewed router predates every manager-ownership route. Deployment
// supplies proof of its image build separately from application source.
const RECEIVERLESS_SOURCE: &str = "bc8cd8bf0f585446a37d3bd50a8e3df2d6d06e6e";

#[derive(Clone, PartialEq, Eq)]
pub struct LegacyReceiverImage {
    manifest: String,
    config_digest: String,
}

impl LegacyReceiverImage {
    pub fn from_reviewed_mapping(
        manifest: String,
        config_digest: String,
        source_revision: &str,
    ) -> crate::Result<Self> {
        let valid_manifest = manifest
            .strip_prefix("ghcr.io/")
            .and_then(|image| image.split_once("@sha256:"))
            .is_some_and(|(repository, digest)| {
                manifest.len() <= 2048
                    && repository.contains('/')
                    && repository.split('/').all(|part| {
                        !part.is_empty()
                            && part.bytes().all(|byte| {
                                byte.is_ascii_lowercase()
                                    || byte.is_ascii_digit()
                                    || matches!(byte, b'.' | b'_' | b'-')
                            })
                    })
                    && valid_sha256(digest)
                    && config_digest.strip_prefix("sha256:") != Some(digest)
            });
        if source_revision != RECEIVERLESS_SOURCE
            || !valid_manifest
            || !config_digest
                .strip_prefix("sha256:")
                .is_some_and(valid_sha256)
        {
            return Err(AdapterError::Rejected(
                "invalid reviewed receiver-less image mapping".into(),
            ));
        }
        Ok(Self {
            manifest,
            config_digest,
        })
    }

    fn matches(&self, running_image: &str) -> bool {
        crate::runtime_startup_policy::running_image_matches(
            running_image,
            &self.manifest,
            &self.config_digest,
        )
    }
}

impl ProviderHostedControlTransport {
    pub fn with_legacy_receiver_image(mut self, image: Option<LegacyReceiverImage>) -> Self {
        self.legacy_receiver_image = image;
        self
    }

    pub(super) async fn legacy_receiver_incarnation(
        &self,
        endpoint: &RuntimeEndpoint,
    ) -> crate::Result<Option<ReceiverIncarnation>> {
        let (
            Some(proof),
            RuntimeDiscovery::AgentSandbox(client),
            Some((handle, binding)),
            Some(identity),
        ) = (
            self.legacy_receiver_image.as_ref(),
            &self.client,
            endpoint.native_binding(),
            endpoint.native_identity(),
        )
        else {
            return Ok(None);
        };
        if self.native_transport == NativeRuntimeTransport::Direct
            || endpoint
                .control_token()
                .is_none_or(|token| token.is_empty())
            || self.native_endpoint_state(handle).await.0 != *identity
        {
            return Ok(None);
        }
        let current = client.receiver_incarnation(handle, binding).await?;
        if !proof.matches(&current.image_id)
            || self.native_endpoint_state(handle).await.0 != *identity
        {
            return Ok(None);
        }
        Ok(Some(current))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_reference_and_config_id_match_only_their_reviewed_digests() {
        let manifest_digest = format!("sha256:{}", "1".repeat(64));
        let manifest = format!("ghcr.io/fixture/runtime@{manifest_digest}");
        let config = format!("sha256:{}", "2".repeat(64));
        let image = LegacyReceiverImage::from_reviewed_mapping(
            manifest.clone(),
            config.clone(),
            RECEIVERLESS_SOURCE,
        )
        .unwrap();

        assert!(image.matches(&manifest));
        assert!(image.matches(&format!("docker-pullable://{manifest}")));
        assert!(image.matches(&config));
        assert!(image.matches(&format!("containerd://{config}")));
        assert!(!image.matches(&manifest_digest));
        assert!(!image.matches(&format!("ghcr.io/fixture/runtime@{config}")));
        assert!(!image.matches("ghcr.io/fixture/runtime:legacy"));
        assert!(LegacyReceiverImage::from_reviewed_mapping(
            manifest.clone(),
            manifest_digest,
            RECEIVERLESS_SOURCE,
        )
        .is_err());
        assert!(LegacyReceiverImage::from_reviewed_mapping(manifest, config, "unknown").is_err());
    }
}
